//! Controller bring-up, discovery admission, privacy and ACL/security flow.

use super::*;

impl ControlMachine {
    pub fn start(&mut self) -> Result<Vec<ControlAction>, ControlFailure> {
        if self.phase != ControlPhase::Idle {
            return Err(ControlFailure::InvalidStartState(self.phase));
        }
        self.plan.validate().map_err(ControlFailure::Plan)?;
        self.phase = ControlPhase::Attaching;
        Ok(vec![
            ControlAction::BeginOperationalAttach,
            ControlAction::ArmTimer(ControlTimer::Attach),
        ])
    }

    pub(super) fn begin_member_connection(
        &mut self,
        member_index: usize,
    ) -> Result<Vec<ControlAction>, ControlFailure> {
        let request = self
            .plan
            .connect_request(member_index)
            .map_err(ControlFailure::Plan)?;
        self.phase = ControlPhase::ConnectingMember {
            member_index,
        };
        Ok(vec![
            ControlAction::BeginFalConnection {
                member_index,
                request,
            },
            ControlAction::ArmTimer(
                ControlTimer::Connect { member_index },
            ),
        ])
    }

    pub(super) fn remove_known_connection(
        &mut self,
        connection: ConnectionToken,
    ) {
        for slot in &mut self.confirmed.connections {
            if *slot == Some(connection) {
                *slot = None;
            }
        }
    }

    pub(super) fn connection_set_matches(
        &self,
        snapshot: &LeConnectionSetSnapshot,
    ) -> bool {
        if snapshot.connections.len() != self.plan.members.len() {
            return false;
        }

        self.confirmed.connections.iter().all(|expected| {
            let Some(expected) = expected else {
                return false;
            };
            snapshot.connections.iter().any(|actual| {
                actual.token() == *expected
                    && actual.encryption_enabled != 0
            })
        })
    }
}
