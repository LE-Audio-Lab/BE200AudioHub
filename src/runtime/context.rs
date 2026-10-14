use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
};

use crate::architecture::{
    execution::ExecutionDomain,
    types::Generation,
};

use super::{
    generation::GenerationGate,
    supervisor::{ContextFault, ContextFaultKind, FaultSink},
};

type Job<S> = Box<dyn FnOnce(&mut S) + Send + 'static>;

enum ContextMessage<S> {
    Run {
        generation: Generation,
        job: Job<S>,
    },
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostError {
    Full,
    StaleGeneration,
    Stopped,
}

pub struct ContextHandle<S> {
    tx: SyncSender<ContextMessage<S>>,
    accepting: Arc<AtomicBool>,
    generation: GenerationGate,
}

pub struct ContextJoin<S> {
    tx: SyncSender<ContextMessage<S>>,
    accepting: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

pub fn spawn_serial_context<S>(
    domain: ExecutionDomain,
    capacity: usize,
    initial_state: S,
    generation: GenerationGate,
    faults: FaultSink,
) -> (ContextHandle<S>, ContextJoin<S>)
where
    S: Send + 'static,
{
    assert!(capacity > 0, "context mailbox capacity must be nonzero");
    let (tx, rx) = mpsc::sync_channel::<ContextMessage<S>>(capacity);
    let accepting = Arc::new(AtomicBool::new(true));
    let thread_accepting = accepting.clone();
    let thread_generation = generation.clone();

    let join = thread::Builder::new()
        .name(format!("{domain:?}"))
        .spawn(move || {
            let mut state = initial_state;
            while thread_accepting.load(Ordering::Acquire) {
                let message = match rx.recv() {
                    Ok(message) => message,
                    Err(_) => {
                        let _ = faults.report(ContextFault {
                            domain,
                            kind: ContextFaultKind::InboxDisconnected,
                        });
                        break;
                    }
                };

                match message {
                    ContextMessage::Run { generation, job } => {
                        if !thread_generation.accepts(generation) {
                            continue;
                        }
                        if catch_unwind(AssertUnwindSafe(|| job(&mut state))).is_err() {
                            let _ = faults.report(ContextFault {
                                domain,
                                kind: ContextFaultKind::HandlerPanicked,
                            });
                            break;
                        }
                    }
                    ContextMessage::Stop => break,
                }
            }
            thread_accepting.store(false, Ordering::Release);
        })
        .expect("failed to spawn execution context");

    (
        ContextHandle {
            tx: tx.clone(),
            accepting: accepting.clone(),
            generation,
        },
        ContextJoin {
            tx,
            accepting,
            join: Some(join),
        },
    )
}

impl<S> Clone for ContextHandle<S> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            accepting: self.accepting.clone(),
            generation: self.generation.clone(),
        }
    }
}

impl<S> ContextHandle<S> {
    pub fn try_post<F>(&self, generation: Generation, job: F) -> Result<(), PostError>
    where
        F: FnOnce(&mut S) + Send + 'static,
    {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(PostError::Stopped);
        }
        if !self.generation.accepts(generation) {
            return Err(PostError::StaleGeneration);
        }

        self.tx
            .try_send(ContextMessage::Run {
                generation,
                job: Box::new(job),
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => PostError::Full,
                TrySendError::Disconnected(_) => PostError::Stopped,
            })
    }

    pub fn current_generation(&self) -> Generation {
        self.generation.current()
    }
}

impl<S> ContextJoin<S> {
    pub fn request_stop(&self) {
        self.accepting.store(false, Ordering::Release);
        // If the inbox is empty this wakes recv(). If it is full the worker is
        // already runnable; after processing one queued item it observes
        // accepting=false and exits on the next loop condition.
        let _ = self.tx.try_send(ContextMessage::Stop);
    }

    pub fn join(mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        time::Duration,
    };

    use super::*;
    use crate::runtime::supervisor::{fault_channel, ContextFaultKind};

    #[test]
    fn serial_context_preserves_order_and_single_owner_state() {
        let generation = GenerationGate::new(Generation(1));
        let (faults, _fault_inbox) = fault_channel(4);
        let (handle, join) = spawn_serial_context(
            ExecutionDomain::Core,
            8,
            Vec::<u8>::new(),
            generation,
            faults,
        );

        handle.try_post(Generation(1), |state| state.push(1)).unwrap();
        handle.try_post(Generation(1), |state| state.push(2)).unwrap();

        let (tx, rx) = mpsc::channel();
        handle
            .try_post(Generation(1), move |state| {
                tx.send(state.clone()).unwrap();
            })
            .unwrap();

        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), vec![1, 2]);
        drop(handle);
        join.join();
    }

    #[test]
    fn stale_generation_is_rejected_before_enqueue() {
        let generation = GenerationGate::new(Generation(3));
        let (faults, _fault_inbox) = fault_channel(4);
        let (handle, join) = spawn_serial_context(
            ExecutionDomain::Core,
            2,
            (),
            generation.clone(),
            faults,
        );
        generation.advance();
        assert_eq!(
            handle.try_post(Generation(3), |_| {}),
            Err(PostError::StaleGeneration)
        );
        drop(handle);
        join.join();
    }

    #[test]
    fn request_stop_wakes_idle_context_even_if_handle_is_still_alive() {
        let generation = GenerationGate::new(Generation(5));
        let (faults, _fault_inbox) = fault_channel(4);
        let (handle, join) = spawn_serial_context(
            ExecutionDomain::Core,
            2,
            (),
            generation,
            faults,
        );

        join.request_stop();
        join.join();

        assert_eq!(
            handle.try_post(Generation(5), |_| {}),
            Err(PostError::Stopped)
        );
    }

    #[test]
    fn handler_panic_is_reported_and_context_stops() {
        let generation = GenerationGate::new(Generation(9));
        let (faults, fault_inbox) = fault_channel(4);
        let (handle, join) = spawn_serial_context(
            ExecutionDomain::HciHost,
            2,
            (),
            generation,
            faults,
        );
        handle
            .try_post(Generation(9), |_| panic!("intentional test panic"))
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let fault = loop {
            if let Ok(fault) = fault_inbox.try_recv() {
                break fault;
            }
            assert!(std::time::Instant::now() < deadline, "fault was not reported");
            std::thread::yield_now();
        };
        assert_eq!(fault.domain, ExecutionDomain::HciHost);
        assert_eq!(fault.kind, ContextFaultKind::HandlerPanicked);

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while handle.try_post(Generation(9), |_| {}) != Err(PostError::Stopped) {
            assert!(std::time::Instant::now() < deadline, "context did not stop");
            std::thread::yield_now();
        }

        drop(handle);
        join.join();
    }
}
