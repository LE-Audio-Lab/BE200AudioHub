//! Explicit Media <-> HCI boundary.
//!
//! First implementation target: interval-group scheduling inspired by PipeWire
//! and Espressif, backed by Zephyr/Bumble-style Host queues.
//!
//! Responsibilities:
//! - map FrameId to one authoritative StreamClock interval;
//! - submit left/right as one logical group;
//! - keep backlog bounded;
//! - drop stale frames rather than accumulating latency;
//! - use pre-encoded codec-agnostic fallback SDUs when a frame misses its slot.
//!
//! The Bridge is intentionally not assigned to either Media or HCI ownership at
//! this architecture stage.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoTxBridgeState {
    Idle,
    Primed,
    Running,
    Draining,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoTxPolicy {
    IntervalGroup,
    ControllerAnchored,
}
