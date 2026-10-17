use std::collections::BTreeSet;

pub const MAX_HCI_CONNECTION_HANDLE: u16 = 0x0EFF;
pub const MAX_ISO_SDU_LENGTH: usize = 0x0FFF;
/// Review's first product stream is one stereo interval group: at most one SDU
/// for each of the two CIS members. A one-member group remains valid for a
/// temporarily inactive/shadow peer.
pub const MAX_ISO_SDU_GROUP_MEMBERS: usize = 2;
/// HCI ISO first fragments carry PSN + SDU length/status metadata inside the
/// controller-advertised ISO data packet length.
pub const ISO_SDU_INFO_LENGTH: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSduRequest {
    pub connection_handle: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoSduConfigError {
    InvalidConnectionHandle(u16),
    SduTooLarge { length: usize },
}

impl IsoSduRequest {
    pub fn validate(&self) -> Result<(), IsoSduConfigError> {
        if self.connection_handle > MAX_HCI_CONNECTION_HANDLE {
            return Err(IsoSduConfigError::InvalidConnectionHandle(
                self.connection_handle,
            ));
        }
        if self.payload.len() > MAX_ISO_SDU_LENGTH {
            return Err(IsoSduConfigError::SduTooLarge {
                length: self.payload.len(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSduGroupRequest {
    pub sdus: Vec<IsoSduRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoSubmitOutcome {
    Accepted,
    Backpressured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoSduGroupConfigError {
    Empty,
    TooManyMembers { count: usize },
    DuplicateConnectionHandle(u16),
    Member {
        index: usize,
        error: IsoSduConfigError,
    },
}

impl IsoSduGroupRequest {
    pub fn single(sdu: IsoSduRequest) -> Self {
        Self { sdus: vec![sdu] }
    }

    pub fn validate(&self) -> Result<(), IsoSduGroupConfigError> {
        if self.sdus.is_empty() {
            return Err(IsoSduGroupConfigError::Empty);
        }
        if self.sdus.len() > MAX_ISO_SDU_GROUP_MEMBERS {
            return Err(IsoSduGroupConfigError::TooManyMembers {
                count: self.sdus.len(),
            });
        }

        let mut handles = BTreeSet::new();
        for (index, sdu) in self.sdus.iter().enumerate() {
            if let Err(error) = sdu.validate() {
                return Err(IsoSduGroupConfigError::Member { index, error });
            }
            if !handles.insert(sdu.connection_handle) {
                return Err(
                    IsoSduGroupConfigError::DuplicateConnectionHandle(
                        sdu.connection_handle,
                    ),
                );
            }
        }
        Ok(())
    }
}

/// Number of controller HCI ISO packets needed for one complete SDU using the
/// same first/continuation capacities as Bumble `Device::send_iso_sdu()`.
pub fn iso_packet_count(
    sdu_length: usize,
    packet_length: usize,
) -> Option<usize> {
    if packet_length <= ISO_SDU_INFO_LENGTH {
        return None;
    }

    let first_capacity = packet_length - ISO_SDU_INFO_LENGTH;
    if sdu_length <= first_capacity {
        return Some(1);
    }

    let remaining = sdu_length - first_capacity;
    Some(1 + remaining.div_ceil(packet_length))
}

pub fn iso_group_packet_count(
    group: &IsoSduGroupRequest,
    packet_length: usize,
) -> Option<usize> {
    group.sdus.iter().try_fold(0usize, |total, sdu| {
        iso_packet_count(sdu.payload.len(), packet_length)
            .and_then(|count| total.checked_add(count))
    })
}

/// Bound Bumble's internally-unbounded waiting queue without rejecting one
/// legal large SDU/group merely because it fragments beyond the controller's
/// instantaneous packet window.
///
/// Normal stereo traffic therefore cannot queue beyond the controller window.
/// A group larger than that window is admitted only when the previous group is
/// fully drained, so at most one such logical group can reside in Host waiting
/// state.
pub fn iso_group_fits(
    pending_packets: usize,
    controller_window: usize,
    required_packets: usize,
) -> bool {
    if controller_window == 0 || required_packets == 0 {
        return false;
    }
    let admission_limit = controller_window.max(required_packets);
    pending_packets
        .checked_add(required_packets)
        .is_some_and(|total| total <= admission_limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sdu(handle: u16, length: usize) -> IsoSduRequest {
        IsoSduRequest {
            connection_handle: handle,
            payload: vec![0; length],
        }
    }

    #[test]
    fn complete_sdu_boundary_accepts_generic_payload_sizes() {
        sdu(0x0900, 0).validate().unwrap();
        sdu(0x0900, MAX_ISO_SDU_LENGTH).validate().unwrap();
    }

    #[test]
    fn complete_sdu_boundary_rejects_invalid_handle_and_oversize() {
        assert_eq!(
            sdu(0x0F00, 1).validate(),
            Err(IsoSduConfigError::InvalidConnectionHandle(0x0F00))
        );
        assert_eq!(
            sdu(0x0900, MAX_ISO_SDU_LENGTH + 1).validate(),
            Err(IsoSduConfigError::SduTooLarge {
                length: MAX_ISO_SDU_LENGTH + 1,
            })
        );
    }

    #[test]
    fn interval_group_is_bounded_and_handle_unique() {
        IsoSduGroupRequest {
            sdus: vec![sdu(0x0900, 100), sdu(0x0901, 100)],
        }
        .validate()
        .unwrap();

        assert_eq!(
            IsoSduGroupRequest { sdus: vec![] }.validate(),
            Err(IsoSduGroupConfigError::Empty)
        );
        assert_eq!(
            IsoSduGroupRequest {
                sdus: vec![
                    sdu(0x0900, 1),
                    sdu(0x0901, 1),
                    sdu(0x0902, 1),
                ],
            }
            .validate(),
            Err(IsoSduGroupConfigError::TooManyMembers { count: 3 })
        );
        assert_eq!(
            IsoSduGroupRequest {
                sdus: vec![sdu(0x0900, 1), sdu(0x0900, 1)],
            }
            .validate(),
            Err(IsoSduGroupConfigError::DuplicateConnectionHandle(
                0x0900,
            ))
        );
    }

    #[test]
    fn packet_count_matches_bumble_first_fragment_overhead() {
        assert_eq!(iso_packet_count(0, 248), Some(1));
        assert_eq!(iso_packet_count(100, 248), Some(1));
        assert_eq!(iso_packet_count(244, 248), Some(1));
        assert_eq!(iso_packet_count(245, 248), Some(2));
        assert_eq!(iso_packet_count(492, 248), Some(2));
        assert_eq!(iso_packet_count(493, 248), Some(3));
        assert_eq!(iso_packet_count(1, ISO_SDU_INFO_LENGTH), None);
    }

    #[test]
    fn group_packet_count_is_sum_of_member_fragment_counts() {
        let group = IsoSduGroupRequest {
            sdus: vec![sdu(0x0900, 100), sdu(0x0901, 493)],
        };
        assert_eq!(iso_group_packet_count(&group, 248), Some(4));
    }

    #[test]
    fn admission_never_moves_normal_backlog_past_controller_window() {
        // BE200 R5 evidence: 8 ISO packet credits. Two 100-byte mono SDUs are
        // one HCI packet each, so four stereo intervals fit in the window.
        assert!(iso_group_fits(6, 8, 2));
        assert!(!iso_group_fits(7, 8, 2));
        assert!(!iso_group_fits(8, 8, 2));
    }

    #[test]
    fn one_large_group_may_exceed_window_only_from_drained_state() {
        assert!(iso_group_fits(0, 8, 17));
        assert!(!iso_group_fits(1, 8, 17));
        assert!(!iso_group_fits(0, 0, 17));
    }
}
