use std::{
    thread,
    time::{Duration, Instant},
};

use crate::{
    plane::l1_hci::acl::scan::ScanSummary,
    runtime::mailbox::RecvError,
};

use crate::{
    plane::l1_hci::interface::mailbox::{HciReport, HciReportInbox},
    plane::l1_hci::host::{
        engine::{OperationalAttachState, PassiveScanState},
        stack::{spawn_be200_hci_stack, HciStackConfig, HciStackExit},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmokeConfig {
    pub scan_duration: Duration,
    pub target_name: Option<String>,
}

impl Default for SmokeConfig {
    fn default() -> Self {
        Self {
            scan_duration: Duration::from_secs(8),
            target_name: None,
        }
    }
}

pub fn parse_smoke_args(args: &[String]) -> Result<SmokeConfig, String> {
    let mut config = SmokeConfig::default();
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--seconds" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --seconds".to_string())?;
                let seconds: u64 = value
                    .parse()
                    .map_err(|_| "--seconds must be an integer".to_string())?;
                if !(1..=60).contains(&seconds) {
                    return Err("--seconds must be in 1..=60".to_string());
                }
                config.scan_duration = Duration::from_secs(seconds);
                index += 2;
            }
            "--name" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --name".to_string())?;
                if value.is_empty() {
                    return Err("--name must not be empty".to_string());
                }
                config.target_name = Some(value.clone());
                index += 2;
            }
            unknown => {
                return Err(format!("unknown hci-smoke argument: {unknown}"));
            }
        }
    }

    Ok(config)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmokeResult {
    pub capabilities_seen: bool,
    pub scan: ScanSummary,
    pub target_seen: bool,
    pub exit: HciStackExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmokeError {
    AttachFailed,
    ScanFailed,
    Timeout,
    ReportDisconnected,
    ControlBackpressure,
}

#[cfg(windows)]
pub fn run_be200_smoke(config: SmokeConfig) -> Result<SmokeResult, SmokeError> {
    let stack = spawn_be200_hci_stack(HciStackConfig::default());

    let run = (|| {
        stack
            .control()
            .try_begin_operational_attach()
            .map_err(|_| SmokeError::ControlBackpressure)?;

        wait_until(&stack.reports(), Duration::from_secs(5), |report| {
            match report {
                HciReport::AttachState(OperationalAttachState::Failed(_)) => {
                    Err(SmokeError::AttachFailed)
                }
                HciReport::CapabilitiesReady(_) => Ok(Some(())),
                _ => Ok(None),
            }
        })?;

        stack
            .control()
            .try_start_passive_extended_scan()
            .map_err(|_| SmokeError::ControlBackpressure)?;

        wait_until(&stack.reports(), Duration::from_secs(5), |report| {
            match report {
                HciReport::PassiveScanState(PassiveScanState::Failed { .. }) => {
                    Err(SmokeError::ScanFailed)
                }
                HciReport::PassiveScanState(PassiveScanState::Scanning) => Ok(Some(())),
                _ => Ok(None),
            }
        })?;

        thread::sleep(config.scan_duration);

        stack
            .control()
            .try_stop_passive_extended_scan()
            .map_err(|_| SmokeError::ControlBackpressure)?;

        let scan = wait_until(
            &stack.reports(),
            Duration::from_secs(5),
            |report| match report {
                HciReport::PassiveScanState(PassiveScanState::Failed { .. }) => {
                    Err(SmokeError::ScanFailed)
                }
                HciReport::ScanComplete(summary) => Ok(Some(summary)),
                _ => Ok(None),
            },
        )?;

        let target_seen = config.target_name.as_ref().is_none_or(|target| {
            scan.observations
                .iter()
                .any(|observation| observation.name.as_deref() == Some(target.as_str()))
        });

        Ok((scan, target_seen))
    })();

    let exit = stack.shutdown();
    let (scan, target_seen) = run?;

    Ok(SmokeResult {
        capabilities_seen: true,
        scan,
        target_seen,
        exit,
    })
}

fn wait_until<T>(
    reports: &HciReportInbox,
    timeout: Duration,
    mut on_report: impl FnMut(HciReport) -> Result<Option<T>, SmokeError>,
) -> Result<T, SmokeError> {
    let deadline = Instant::now() + timeout;

    loop {
        match reports.try_recv() {
            Ok(report) => {
                if let Some(value) = on_report(report)? {
                    return Ok(value);
                }
            }
            Err(RecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(SmokeError::Timeout);
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(RecvError::Disconnected) => {
                return Err(SmokeError::ReportDisconnected);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        plane::l1_hci::interface::mailbox::hci_mailbox,
        plane::l1_hci::host::engine::ControllerCapabilities,
    };

    fn empty_capabilities() -> ControllerCapabilities {
        ControllerCapabilities {
            supported_commands: [0; 64],
            hci_version: None,
            hci_subversion: None,
            manufacturer: None,
            local_le_features: Vec::new(),
            local_le_features_max_page: None,
            le_acl_pool: None,
            iso_pool: None,
            number_of_supported_advertising_sets: 0,
            maximum_advertising_data_length: 0,
        }
    }

    #[test]
    fn smoke_args_default_to_eight_seconds_without_target() {
        let config = parse_smoke_args(&[]).unwrap();
        assert_eq!(config.scan_duration, Duration::from_secs(8));
        assert_eq!(config.target_name, None);
    }

    #[test]
    fn smoke_args_parse_bounded_duration_and_target_name() {
        let args = vec![
            "--seconds".to_string(),
            "5".to_string(),
            "--name".to_string(),
            "Galaxy Buds3 Pro (2F23)".to_string(),
        ];
        let config = parse_smoke_args(&args).unwrap();
        assert_eq!(config.scan_duration, Duration::from_secs(5));
        assert_eq!(
            config.target_name.as_deref(),
            Some("Galaxy Buds3 Pro (2F23)")
        );
    }

    #[test]
    fn smoke_args_reject_unknown_or_unbounded_values() {
        assert!(parse_smoke_args(&["--seconds".into(), "0".into()]).is_err());
        assert!(parse_smoke_args(&["--seconds".into(), "61".into()]).is_err());
        assert!(parse_smoke_args(&["--wat".into()]).is_err());
    }

    #[test]
    fn wait_until_ignores_unrelated_reports_and_returns_match() {
        let (_control, _ingress, reports, ports) = hci_mailbox(2, 2, 4);
        ports
            .report_tx
            .try_send(HciReport::AttachState(OperationalAttachState::Discovering))
            .unwrap();
        ports
            .report_tx
            .try_send(HciReport::CapabilitiesReady(empty_capabilities()))
            .unwrap();

        let result = wait_until(&reports, Duration::from_millis(10), |report| {
            match report {
                HciReport::CapabilitiesReady(_) => Ok(Some(7u8)),
                _ => Ok(None),
            }
        });

        assert_eq!(result, Ok(7));
    }

    #[test]
    fn wait_until_propagates_terminal_report_error() {
        let (_control, _ingress, reports, ports) = hci_mailbox(2, 2, 2);
        ports
            .report_tx
            .try_send(HciReport::AttachState(OperationalAttachState::Failed(
                crate::plane::l1_hci::host::engine::OperationalAttachFailure::HostInitializationFailed,
            )))
            .unwrap();

        let result: Result<(), SmokeError> =
            wait_until(&reports, Duration::from_millis(10), |report| match report {
                HciReport::AttachState(OperationalAttachState::Failed(_)) => {
                    Err(SmokeError::AttachFailed)
                }
                _ => Ok(None),
            });

        assert_eq!(result, Err(SmokeError::AttachFailed));
    }
}
