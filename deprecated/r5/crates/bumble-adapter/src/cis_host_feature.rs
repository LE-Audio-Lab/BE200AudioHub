//! R5.2-B controller pre-ACL gate for CIS (Host Support).
//!
//! Bluetooth Core 5.2+ Vol 4 Part E 7.8.115: HCI LE Set Host Feature
//! controls bit 32, LE Connected Isochronous Stream (Host Support).
//! It must be set *before* establishing any LE ACL, or the controller
//! returns Command Disallowed (0x0C). Create CIS may also return 0x0C
//! while this feature is unset.
//!
//! Linux net/bluetooth/hci_sync.c does exactly this during controller
//! initialization (hci_le_set_host_feature_sync, bit=32,value=1).
//! We scope it to the explicit R5.2-B / future ISO path so R3/R4 and
//! the R5.1/R5.2-A probes remain unchanged.

use std::time::Duration;

use bumble_hci::{metadata::supported_command_names, Command, ReturnParameters};
use bumble_transport::CommandResponse;

use crate::AttachedController;

const CIS_HOST_SUPPORT_BIT: u8 = 32;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SET_HOST_FEATURE_COMMAND: &str = "HCI_LE_SET_HOST_FEATURE_COMMAND";
const READ_LE_FEATURES_COMMAND: &str = "HCI_LE_READ_LOCAL_SUPPORTED_FEATURES_COMMAND";
const READ_ALL_LE_FEATURES_COMMAND: &str = "HCI_LE_READ_ALL_LOCAL_SUPPORTED_FEATURES_COMMAND";

/// Two-stage read-only verification before and after the one-time host write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CisHostFeatureReport {
    pub initially_enabled: bool,
    pub enabled_after: bool,
    pub set_command_sent: bool,
}

pub(crate) fn cis_host_feature_enabled(features: &[u8]) -> bool {
    features.get(usize::from(CIS_HOST_SUPPORT_BIT / 8))
        .is_some_and(|octet| octet & (1u8 << (CIS_HOST_SUPPORT_BIT % 8)) != 0)
}

fn decoded_features(parameters: &ReturnParameters) -> Result<Vec<u8>, String> {
    match parameters {
        ReturnParameters::LeReadLocalSupportedFeatures { status: 0, le_features } =>
            Ok(le_features.to_vec()),
        ReturnParameters::LeReadAllLocalSupportedFeatures { status: 0, le_features, .. } =>
            Ok(le_features.to_vec()),
        unexpected => Err(format!(
            "LE features readback must be a successful typed response: {unexpected:?}"
        )),
    }
}

impl AttachedController {
    /// Must be called *before scanning / ACL establishment*, never from the
    /// Config Codec or Create CIS phases. Does not change pairing material.
    /// No HCI Reset, no firmware update, no SMP.
    pub fn ensure_cis_host_feature_pre_acl(
        &mut self,
    ) -> Result<CisHostFeatureReport, String> {
        if self.device.is_connected() {
            return Err(
                "LE Set Host Feature (CIS bit 32) requires zero connected ACLs; call before R3 scan/connect".into()
            );
        }
        let names = supported_command_names(&self.info.supported_commands);
        let previous = self.info.local_le_features.as_deref()
            .ok_or("LE feature cache missing; cannot verify CIS Host Support bit")?;
        if previous.len() < 5 {
            return Err(format!(
                "LE feature cache too short ({} bytes) for bit 32", previous.len()
            ));
        }
        let initially_enabled = cis_host_feature_enabled(previous);
        println!(
            "[R5.2/HOST] pre-ACL CIS Host Support bit32={} (cached LE features)",
            initially_enabled
        );

        if !initially_enabled {
            if !names.contains(&SET_HOST_FEATURE_COMMAND) {
                return Err(
                    "BE200 has CIS Central but does not advertise HCI LE Set Host Feature (0x2074); cannot enable bit32".into()
                );
            }
            // Set only the host-controlled ISO feature, leaving all other
            // controller state, bonding and firmware untouched.
            let response = self.host.send_command(
                Command::LeSetHostFeature {
                    bit_number: CIS_HOST_SUPPORT_BIT,
                    bit_value: 1,
                },
                COMMAND_TIMEOUT,
            ).map_err(|error| format!(
                "HCI LE Set Host Feature bit32=1 command failed: {error}"
            ))?;
            if response.status() != Some(0)
                || !matches!(response, CommandResponse::Complete { .. })
            {
                return Err(format!(
                    "HCI LE Set Host Feature bit32=1 rejected / incomplete: {response:?}"
                ));
            }
            println!(
                "[R5.2/HOST] HCI LE Set Host Feature (0x2074): bit_number=32 value=1 PASS"
            );
        } else {
            println!("[R5.2/HOST] bit32 already enabled; no HCI Set Host Feature write needed");
        }

        // The Host Feature is observable in the LE feature bitmap; treat
        // Command Complete alone as insufficient proof of support.
        let read_command = if names.contains(&READ_ALL_LE_FEATURES_COMMAND) {
            Command::LeReadAllLocalSupportedFeatures
        } else if names.contains(&READ_LE_FEATURES_COMMAND) {
            Command::LeReadLocalSupportedFeatures
        } else {
            return Err(
                "controller has no supported LE feature readback command".into()
            );
        };
        let response = self.host.send_command(read_command, COMMAND_TIMEOUT)
            .map_err(|error| format!("LE feature readback after Host Feature failed: {error}"))?;
        if response.status() != Some(0) {
            return Err(format!(
                "LE feature readback returned status {:?}",
                response.status()
            ));
        }
        let parameters = response.return_parameters()
            .ok_or("LE feature readback returned Command Status instead of Complete")?;
        let after = decoded_features(parameters)?;
        let enabled_after = cis_host_feature_enabled(&after);
        if !enabled_after {
            return Err(format!(
                "CIS Host Support bit32 still clear after pre-ACL initialization; LE bytes[0..8]={:02X?}",
                &after[..after.len().min(8)]
            ));
        }
        self.info.local_le_features = Some(after);
        println!(
            "[R5.2/HOST] CIS Host Support bit32 READBACK PASS (pre-ACL, no bonds changed)"
        );
        Ok(CisHostFeatureReport {
            initially_enabled,
            enabled_after,
            set_command_sent: !initially_enabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_le_feature_bit_32() {
        assert!(!cis_host_feature_enabled(&[]));
        assert!(!cis_host_feature_enabled(&[0; 8]));
        let mut features = [0u8; 8];
        features[4] = 1;
        assert!(cis_host_feature_enabled(&features));
        features[4] = 2;
        assert!(!cis_host_feature_enabled(&features));
        features[3] = 1 << 4;
        assert!(!cis_host_feature_enabled(&features));
    }

    #[test]
    fn validates_upstream_feature_response_and_rejects_status_only() {
        let mut features = [0u8; 8];
        features[4] = 1;
        let parsed = decoded_features(&ReturnParameters::LeReadLocalSupportedFeatures {
            status: 0,
            le_features: features,
        }).unwrap();
        assert!(cis_host_feature_enabled(&parsed));
        assert!(decoded_features(&ReturnParameters::Status { status: 0 }).is_err());
        assert!(decoded_features(&ReturnParameters::Status { status: 0x0C }).is_err());
    }

    #[test]
    fn host_feature_request_uses_pinned_hci_serializer() {
        let cmd = Command::LeSetHostFeature {
            bit_number: CIS_HOST_SUPPORT_BIT,
            bit_value: 1,
        };
        // H4: type, opcode (0x2074, little-endian), plen=2, bit=32, value=1.
        assert_eq!(cmd.to_bytes(), [0x01, 0x74, 0x20, 0x02, 0x20, 0x01]);
    }
}
