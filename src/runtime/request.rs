use std::collections::HashMap;

use crate::architecture::mailbox::RequestId;

type ReplyHandler<R> = Box<dyn FnOnce(R) + Send + 'static>;

pub struct RequestTracker<R> {
    next_id: u64,
    pending: HashMap<RequestId, ReplyHandler<R>>,
}

impl<R> Default for RequestTracker<R> {
    fn default() -> Self {
        Self {
            next_id: 1,
            pending: HashMap::new(),
        }
    }
}

impl<R> RequestTracker<R> {
    pub fn begin<F>(&mut self, on_reply: F) -> RequestId
    where
        F: FnOnce(R) + Send + 'static,
    {
        let id = RequestId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("request id space exhausted");
        let previous = self.pending.insert(id, Box::new(on_reply));
        debug_assert!(previous.is_none(), "request id unexpectedly reused");
        id
    }

    pub fn complete(&mut self, id: RequestId, reply: R) -> bool {
        let Some(handler) = self.pending.remove(&id) else {
            return false;
        };
        handler(reply);
        true
    }

    pub fn cancel(&mut self, id: RequestId) -> bool {
        self.pending.remove(&id).is_some()
    }

    pub fn contains(&self, id: RequestId) -> bool {
        self.pending.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn replies_can_complete_out_of_order_without_blocking() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut tracker = RequestTracker::default();

        let first_seen = seen.clone();
        let first = tracker.begin(move |value: u8| {
            first_seen.lock().unwrap().push((1, value));
        });
        let second_seen = seen.clone();
        let second = tracker.begin(move |value: u8| {
            second_seen.lock().unwrap().push((2, value));
        });

        assert!(tracker.complete(second, 22));
        assert!(tracker.complete(first, 11));
        assert_eq!(*seen.lock().unwrap(), vec![(2, 22), (1, 11)]);
        assert!(tracker.is_empty());
    }

    #[test]
    fn unknown_or_duplicate_reply_is_rejected() {
        let mut tracker = RequestTracker::<u8>::default();
        let id = tracker.begin(|_| {});
        assert!(tracker.complete(id, 1));
        assert!(!tracker.complete(id, 2));
        assert!(!tracker.complete(RequestId(999), 3));
    }

    #[test]
    fn cancellation_removes_pending_handler() {
        let mut tracker = RequestTracker::<u8>::default();
        let id = tracker.begin(|_| panic!("cancelled request must not run"));
        assert!(tracker.contains(id));
        assert!(tracker.cancel(id));
        assert!(!tracker.contains(id));
        assert!(!tracker.complete(id, 1));
    }
}
