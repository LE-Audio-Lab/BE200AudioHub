//! L3 — Platform / Device Backend.
//!
//! Owns OS/hardware-specific raw transfer primitives and BE200 bootstrap quirks.
//! It does not own HCI packet semantics.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformBackend {
    WindowsBe200,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerBootstrapState {
    Detached,
    Present,
    Operational,
    Faulted,
}
