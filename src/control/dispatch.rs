//! Thin Control -> HCI / Core-timer action adapter.
//!
//! This module deliberately owns no Control state machine and performs no
//! waiting. Reliable HCI actions are submitted nonblocking to HciControl;
//! timer actions are returned to the Core timer owner as directives.

use crate::{
    plane::l1_hci::interface::command::HciControl,
    runtime::mailbox::SendError,
};

use super::machine::{ControlAction, ControlTimer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlDispatchEffect {
    HciSubmitted,
    ArmTimer(ControlTimer),
    DisarmTimer(ControlTimer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlDispatchError {
    pub action: ControlAction,
    pub error: SendError,
}

pub fn dispatch_control_action(
    hci: &HciControl,
    action: ControlAction,
) -> Result<ControlDispatchEffect, ControlDispatchError> {
    let original = action.clone();

    let result = match action {
        ControlAction::BeginOperationalAttach => {
            hci.try_begin_operational_attach()
        }
        ControlAction::ReadControllerIdentity => {
            hci.try_read_controller_identity()
        }
        ControlAction::EnsureCisHostFeature => {
            hci.try_ensure_cis_host_feature()
        }
        ControlAction::StartPassiveExtendedScan => {
            hci.try_start_passive_extended_scan()
        }
        ControlAction::StopPassiveExtendedScan => {
            hci.try_stop_passive_extended_scan()
        }
        ControlAction::ConfigureControllerPrivacy(config) => {
            hci.try_configure_controller_privacy(config)
        }
        ControlAction::BeginFalConnection {
            member_index: _,
            request,
        } => hci.try_begin_fal_connection(request),
        ControlAction::BeginStoredEncryption {
            member_index: _,
            request,
        } => hci.try_begin_stored_encryption(request),
        ControlAction::CancelFalConnection => {
            hci.try_cancel_fal_connection()
        }
        ControlAction::DisconnectAcl { connection } => {
            hci.try_disconnect(connection)
        }
        ControlAction::ClearControllerPrivacy => {
            hci.try_clear_controller_privacy()
        }
        ControlAction::ConfigureCig(config) => {
            hci.try_configure_cig(config)
        }
        ControlAction::CreateCis(request) => {
            hci.try_create_cis(request)
        }
        ControlAction::SetupIsoInputPathByRef { cis } => {
            hci.try_setup_iso_input_path_by_ref(cis)
        }
        ControlAction::RemoveIsoPathByRef {
            cis,
            directions,
        } => hci.try_remove_iso_path_by_ref(cis, directions),
        ControlAction::DisconnectCisByRef { cis } => {
            hci.try_disconnect_cis_by_ref(cis)
        }
        ControlAction::RemoveCig { cig_id } => {
            hci.try_remove_cig(cig_id)
        }
        ControlAction::BindStreamRoute(binding) => {
            hci.try_bind_stream_route(binding)
        }
        ControlAction::UnbindStreamRoute {
            session_id,
            stream_id,
        } => hci.try_unbind_stream_route(session_id, stream_id),
        ControlAction::BeginGatt(request) => {
            hci.try_begin_gatt(request)
        }
        ControlAction::CancelGatt {
            connection,
            request_id,
        } => hci.try_cancel_gatt(connection, request_id),
        ControlAction::ArmTimer(timer) => {
            return Ok(ControlDispatchEffect::ArmTimer(timer));
        }
        ControlAction::DisarmTimer(timer) => {
            return Ok(ControlDispatchEffect::DisarmTimer(timer));
        }
    };

    result
        .map(|()| ControlDispatchEffect::HciSubmitted)
        .map_err(|error| ControlDispatchError {
            action: original,
            error,
        })
}

#[cfg(test)]
mod tests {
    use crate::{
        architecture::types::{SessionId, StreamId},
        plane::l1_hci::interface::{
            command::HciCommand,
            mailbox::hci_mailbox_with_iso,
        },
        plane::l1_hci::iso::{
            cig_cis::CisRef,
            route::StereoStreamRouteBinding,
        },
    };

    use super::*;

    #[test]
    fn timer_directives_never_touch_hci_control_lane() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 1, 1, 1);

        assert_eq!(
            dispatch_control_action(
                &control,
                ControlAction::ArmTimer(ControlTimer::Attach),
            ),
            Ok(ControlDispatchEffect::ArmTimer(
                ControlTimer::Attach,
            ))
        );
        assert_eq!(
            dispatch_control_action(
                &control,
                ControlAction::DisarmTimer(ControlTimer::Attach),
            ),
            Ok(ControlDispatchEffect::DisarmTimer(
                ControlTimer::Attach,
            ))
        );
        assert!(ports.control_rx.try_recv().is_err());
    }

    #[test]
    fn route_and_semantic_iso_actions_map_to_reliable_hci_commands() {
        let (control, _ingress, _reports, ports) =
            hci_mailbox_with_iso(1, 4, 1, 1);
        let binding = StereoStreamRouteBinding {
            session_id: SessionId(7),
            stream_id: StreamId(11),
            left: CisRef {
                cig_id: 1,
                cis_id: 1,
            },
            right: CisRef {
                cig_id: 1,
                cis_id: 2,
            },
        };

        assert_eq!(
            dispatch_control_action(
                &control,
                ControlAction::BindStreamRoute(binding),
            ),
            Ok(ControlDispatchEffect::HciSubmitted)
        );
        assert_eq!(
            dispatch_control_action(
                &control,
                ControlAction::SetupIsoInputPathByRef {
                    cis: binding.left,
                },
            ),
            Ok(ControlDispatchEffect::HciSubmitted)
        );
        assert_eq!(
            dispatch_control_action(
                &control,
                ControlAction::UnbindStreamRoute {
                    session_id: binding.session_id,
                    stream_id: binding.stream_id,
                },
            ),
            Ok(ControlDispatchEffect::HciSubmitted)
        );

        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::BindStreamRoute(binding)
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::SetupIsoInputPathByRef {
                cis: binding.left,
            }
        );
        assert_eq!(
            ports.control_rx.try_recv().unwrap(),
            HciCommand::UnbindStreamRoute {
                session_id: binding.session_id,
                stream_id: binding.stream_id,
            }
        );
    }

    #[test]
    fn reliable_control_backpressure_is_explicit_and_preserves_action() {
        let (control, _ingress, _reports, _ports) =
            hci_mailbox_with_iso(1, 1, 1, 1);

        dispatch_control_action(
            &control,
            ControlAction::BeginOperationalAttach,
        )
        .unwrap();

        let action = ControlAction::ReadControllerIdentity;
        assert_eq!(
            dispatch_control_action(&control, action.clone()),
            Err(ControlDispatchError {
                action,
                error: SendError::Full,
            })
        );
    }
}
