//! Windows CLI probe lifetime: balanced timer request and cooperative Ctrl+C.
use std::sync::atomic::{AtomicBool, Ordering};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);

pub struct ProbeGuard { #[cfg(windows)] timer_armed: bool }
impl ProbeGuard {
    pub fn install() -> Result<Self, String> {
        if ACTIVE.swap(true, Ordering::SeqCst) { return Err("an ISO probe is already active".into()); }
        CANCELLED.store(false, Ordering::SeqCst);
        #[cfg(windows)] {
            // SAFETY: callback has the required ABI, lives for the process lifetime,
            // and only stores an atomic flag. It never calls USB/GATT/logging APIs.
            if unsafe { windows::SetConsoleCtrlHandler(Some(windows::console_handler), 1) } == 0 {
                ACTIVE.store(false, Ordering::SeqCst);
                return Err(format!("cannot install probe Ctrl+C handler: {}", std::io::Error::last_os_error()));
            }
            // Paired in Drop; this is not a hard realtime scheduling guarantee.
            let timer_armed = unsafe { windows::timeBeginPeriod(1) } == 0;
            if !timer_armed { eprintln!("[R5.3/TIMER] 1 ms timer request unavailable; pacing deadlines remain enforced"); }
            Ok(Self { timer_armed })
        }
        #[cfg(not(windows))] { Ok(Self {}) }
    }
    pub fn check_cancelled() -> Result<(), String> {
        if CANCELLED.load(Ordering::SeqCst) { Err("probe interrupted; performing ordered teardown".into()) } else { Ok(()) }
    }
}
impl Drop for ProbeGuard {
    fn drop(&mut self) {
        #[cfg(windows)] unsafe {
            windows::SetConsoleCtrlHandler(Some(windows::console_handler), 0);
            if self.timer_armed { windows::timeEndPeriod(1); }
        }
        ACTIVE.store(false, Ordering::SeqCst);
    }
}
#[cfg(windows)]
mod windows {
    use super::{CANCELLED, Ordering};
    pub unsafe extern "system" fn console_handler(kind: u32) -> i32 {
        if kind <= 1 { CANCELLED.store(true, Ordering::SeqCst); 1 } else { 0 }
    }
    #[link(name="kernel32")]
    extern "system" {
        pub fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32)->i32>, add: i32) -> i32;
    }
    #[link(name="winmm")]
    extern "system" {
        pub fn timeBeginPeriod(period: u32) -> u32;
        pub fn timeEndPeriod(period: u32) -> u32;
    }
}
