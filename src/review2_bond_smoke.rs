use std::{
    path::PathBuf,
    time::Duration,
};

use crate::{
    hci_host::{
        connection::{
            ConnectionToken, DisconnectState, FalConnectRequest,
            FalConnectionState, LeConnectionSetSnapshot,
            LeInitiationParameters, LePeerIdentity,
        },
        engine::{
            ControllerIdentity, ControllerIdentityFailure,
            OperationalAttachState,
        },
        mailbox::HciReport,
        privacy::{
            ControllerPrivacyState, ResolvingListConfig, ResolvingPeer,
            DEFAULT_RPA_TIMEOUT_SECONDS,
        },
        security::{
            StoredEncryptionRequest, StoredEncryptionState,
            StoredLeKeyMaterial,
        },
    },
    review2_bond::{ReviewBondMaterial, ReviewBondMember},
};

const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondSmokeConfig {
    pub sfi: PathBuf,
    pub ddc: PathBuf,
    pub bond_root: PathBuf,
    pub target_name: String,
    pub connect_timeout: Duration,
}

pub fn parse_bond_smoke_args(
    args: &[String],
) -> Result<BondSmokeConfig, String> {
    let mut sfi = None;
    let mut ddc = None;
    let mut bond_root = None;
    let mut target_name = None;
    let mut connect_timeout =
        Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS);
    let mut confirmed = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" => {
                let flag = args[index].clone();
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                match flag.as_str() {
                    "--sfi" => sfi = Some(PathBuf::from(value)),
                    "--ddc" => ddc = Some(PathBuf::from(value)),
                    "--bond-root" => {
                        bond_root = Some(PathBuf::from(value))
                    }
                    "--name" => {
                        if value.trim().is_empty() {
                            return Err("--name must not be empty".into());
                        }
                        target_name = Some(value.clone());
                    }
                    "--connect-seconds" => {
                        let seconds: u64 = value.parse().map_err(|_| {
                            "--connect-seconds must be an integer".to_string()
                        })?;
                        if !(3..=30).contains(&seconds) {
                            return Err(
                                "--connect-seconds must be in 3..=30".into()
                            );
                        }
                        connect_timeout = Duration::from_secs(seconds);
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--confirm-stored-bond-hil" => {
                confirmed = true;
                index += 1;
            }
            unknown => {
                return Err(format!(
                    "unknown review2 bond-smoke argument: {unknown}"
                ))
            }
        }
    }

    if !confirmed {
        return Err(
            "--confirm-stored-bond-hil is required".into(),
        );
    }

    Ok(BondSmokeConfig {
        sfi: sfi.ok_or_else(|| "--sfi is required".to_string())?,
        ddc: ddc.ok_or_else(|| "--ddc is required".to_string())?,
        bond_root: bond_root
            .ok_or_else(|| "--bond-root is required".to_string())?,
        target_name: target_name
            .ok_or_else(|| "--name is required".to_string())?,
        connect_timeout,
    })
}

#[derive(Clone, PartialEq, Eq)]
pub struct BondSmokePlan {
    pub controller_identity_le: [u8; 6],
    pub local_irk: [u8; 16],
    pub members: Vec<ReviewBondMember>,
}

impl BondSmokePlan {
    pub fn from_material(
        material: ReviewBondMaterial,
    ) -> Result<Self, String> {
        if material.members.len() != 2 {
            return Err(format!(
                "Review-2B dual-ACL HIL requires exactly 2 CSIS-verified members; found {}",
                material.members.len()
            ));
        }

        Ok(Self {
            controller_identity_le: material.controller_identity_le,
            local_irk: material.local_irk,
            members: material.members,
        })
    }

    pub fn privacy_config(&self) -> ResolvingListConfig {
        ResolvingListConfig {
            peers: self
                .members
                .iter()
                .map(|member| ResolvingPeer {
                    identity_address_le: member.identity_address_le,
                    identity_address_type: member.address_type,
                    peer_irk: member.peer_irk,
                })
                .collect(),
            local_irk: self.local_irk,
            rpa_timeout_seconds: DEFAULT_RPA_TIMEOUT_SECONDS,
        }
    }

    pub fn connect_request(&self, index: usize) -> FalConnectRequest {
        let member = &self.members[index];
        FalConnectRequest {
            peer: LePeerIdentity {
                address_le: member.identity_address_le,
                address_type: member.address_type,
            },
            parameters: LeInitiationParameters::default(),
        }
    }

    pub fn encryption_request(
        &self,
        index: usize,
        connection: ConnectionToken,
    ) -> StoredEncryptionRequest {
        let member = &self.members[index];
        StoredEncryptionRequest {
            connection,
            key: StoredLeKeyMaterial {
                long_term_key: member.long_term_key,
                encrypted_diversifier:
                    member.encrypted_diversifier,
                random_number: member.random_number,
            },
        }
    }

    pub fn identity_matches(&self, identity: ControllerIdentity) -> bool {
        identity.public_address_le == self.controller_identity_le
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BondSmokeTracker {
    pub attach: Option<OperationalAttachState>,
    pub controller_identity:
        Option<Result<ControllerIdentity, ControllerIdentityFailure>>,
    pub privacy: Option<ControllerPrivacyState>,
    pub fal: Option<FalConnectionState>,
    pub disconnect: Option<DisconnectState>,
    pub encryption: Option<StoredEncryptionState>,
    pub connection_set: LeConnectionSetSnapshot,
}

impl BondSmokeTracker {
    pub fn observe(&mut self, report: &HciReport) {
        match report {
            HciReport::AttachState(state) => self.attach = Some(*state),
            HciReport::CapabilitiesReady(_) => {}
            HciReport::ControllerIdentity(identity) => {
                self.controller_identity = Some(*identity)
            }
            HciReport::PassiveScanState(_) | HciReport::ScanComplete(_) => {}
            HciReport::ControllerPrivacyState(state) => {
                self.privacy = Some(state.clone())
            }
            HciReport::FalConnectionState(state) => {
                self.fal = Some(state.clone())
            }
            HciReport::DisconnectState(state) => {
                self.disconnect = Some(state.clone())
            }
            HciReport::StoredEncryptionState(state) => {
                self.encryption = Some(*state)
            }
            HciReport::ConnectionSet(snapshot) => {
                self.connection_set = snapshot.clone()
            }
        }
    }

    pub fn contains_connection(&self, token: ConnectionToken) -> bool {
        self.connection_set
            .connections
            .iter()
            .any(|connection| connection.token() == token)
    }

    pub fn is_encrypted(&self, token: ConnectionToken) -> bool {
        self.connection_set.connections.iter().any(|connection| {
            connection.token() == token && connection.encryption_enabled != 0
        })
    }
}

pub fn format_address_le(address: [u8; 6]) -> String {
    address
        .iter()
        .rev()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(identity_last: u8, key_seed: u8) -> ReviewBondMember {
        ReviewBondMember {
            identity_address: format!(
                "E8:C9:13:EF:2F:{identity_last:02X}"
            ),
            identity_address_le: [
                identity_last, 0x2f, 0xef, 0x13, 0xc9, 0xe8,
            ],
            address_type: 0,
            peer_irk: [key_seed; 16],
            long_term_key: [key_seed.wrapping_add(1); 16],
            encrypted_diversifier: 0,
            random_number: [0; 8],
        }
    }

    fn material() -> ReviewBondMaterial {
        ReviewBondMaterial {
            controller_identity: "44:A3:BB:36:94:1A".into(),
            controller_identity_le: [
                0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44,
            ],
            local_irk: [0x44; 16],
            members: vec![member(0x23, 0x11), member(0xe0, 0x33)],
        }
    }

    #[test]
    fn parser_requires_explicit_hil_consent_and_paths() {
        assert!(parse_bond_smoke_args(&[]).is_err());

        let args = vec![
            "--sfi".into(), "a.sfi".into(),
            "--ddc".into(), "a.ddc".into(),
            "--bond-root".into(), "bonding".into(),
            "--name".into(), "Galaxy Buds3 Pro (2F23)".into(),
            "--confirm-stored-bond-hil".into(),
        ];
        let config = parse_bond_smoke_args(&args).unwrap();
        assert_eq!(
            config.connect_timeout,
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn plan_requires_exactly_two_verified_members() {
        let mut value = material();
        value.members.pop();
        assert!(BondSmokePlan::from_material(value).is_err());

        let plan = BondSmokePlan::from_material(material()).unwrap();
        assert_eq!(plan.members.len(), 2);
    }

    #[test]
    fn plan_maps_bond_material_into_typed_privacy_and_security_requests() {
        let plan = BondSmokePlan::from_material(material()).unwrap();
        let privacy = plan.privacy_config();
        assert_eq!(privacy.peers.len(), 2);
        assert_eq!(privacy.peers[0].identity_address_le[0], 0x23);
        assert_eq!(privacy.local_irk, [0x44; 16]);

        let token = ConnectionToken {
            connection_handle: 0x0800,
            generation: 7,
        };
        let encryption = plan.encryption_request(0, token);
        assert_eq!(encryption.connection, token);
        assert_eq!(encryption.key.long_term_key, [0x12; 16]);
    }

    #[test]
    fn tracker_projects_connection_set_without_key_material() {
        let token = ConnectionToken {
            connection_handle: 0x0800,
            generation: 3,
        };
        let mut tracker = BondSmokeTracker::default();
        tracker.observe(&HciReport::ConnectionSet(
            LeConnectionSetSnapshot {
                connections: vec![
                    crate::hci_host::connection::LeConnectionSnapshot {
                        connection_handle: 0x0800,
                        generation: 3,
                        role: 0,
                        peer_address_le: [1, 2, 3, 4, 5, 6],
                        peer_address_type: 1,
                        connection_interval: 24,
                        peripheral_latency: 0,
                        supervision_timeout: 42,
                        encryption_enabled: 1,
                        encryption_key_size: 16,
                    },
                ],
            },
        ));
        assert!(tracker.contains_connection(token));
        assert!(tracker.is_encrypted(token));
    }

    #[test]
    fn address_formatter_uses_human_order() {
        assert_eq!(
            format_address_le([
                0x1a, 0x94, 0x36, 0xbb, 0xa3, 0x44,
            ]),
            "44:A3:BB:36:94:1A"
        );
    }
}
