//! Stable Control Plane boundary contracts.
//!
//! Control has two distinct input classes:
//! - upper intents from L0/Core orchestration;
//! - lower semantic reports from the HCI Host.
//!
//! Runtime observation is deliberately separate from semantic Control input.
//! Media/Transport/HCI health snapshots are reduced into RuntimeState, but they
//! do not directly drive the LE Audio ControlMachine.

use crate::{
    architecture::{
        control::{ControlSnapshot, StreamBinding},
        hci_host::HciHostSnapshot,
        lifecycle::LifecycleSnapshot,
        media::MediaSnapshot,
        transport::TransportSnapshot,
        types::{SessionId, StreamId},
    },
    hci_host::mailbox::HciReport,
};

use super::machine::{ControlAction, ControlTimer};

/// L0/Core -> Control semantic intents.
///
/// This intentionally contains only operations already supported by the
/// Control semantic state machine. Lifecycle stop/recovery policy stays in L0
/// until that contract is frozen; it must not be guessed into Control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlIntent {
    Start,
    BeginStreamHandoff {
        session_id: SessionId,
        stream_id: StreamId,
    },
    DiscoveryWindowElapsed,
    TimerExpired(ControlTimer),
}

/// Lower semantic input accepted by Control.
///
/// Raw HCI packets, handles and transport callbacks never cross this boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlLowerReport {
    Hci(HciReport),
}

/// Observation reports consumed by the single-writer RuntimeState reducer.
///
/// These are projections/snapshots of plane-owned state. They are not handles
/// to the mutable objects that implement those planes.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeObservation {
    Lifecycle(LifecycleSnapshot),
    Control(ControlSnapshot),
    Media(MediaSnapshot),
    Hci(HciHostSnapshot),
    Transport(TransportSnapshot),
}

/// One semantic Control transition exposed to the future Core composition.
///
/// Actions are still implementation-neutral ControlAction values. The existing
/// dispatcher classifies them into reliable HCI submissions or Core timer
/// directives without letting the ControlMachine perform I/O or wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlStep {
    pub actions: Vec<ControlAction>,
    pub snapshot: ControlSnapshot,
    pub stream_binding: Option<StreamBinding>,
}
