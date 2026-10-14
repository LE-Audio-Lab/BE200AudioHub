//! Single pre-R2 controller gate.
//!
//! The gate performs one read-only Intel Read Version query. Operational
//! firmware is left untouched. A matching BE200 bootloader is initialized once
//! through the existing guarded boot path using the pinned files under
//! `./firmware`. There is no retry, USB reset, reset-to-bootloader, or fallback
//! firmware search.

use crate::{boot, error::{AppError, Result}, intel, usb};
use rusb::{DeviceHandle, GlobalContext};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const INTEL_READ_VERSION: u16 = 0xFC05;
const READ_VERSION_COMMAND: [u8; 4] = [0x05, 0xFC, 0x01, 0xFF];
const CONTROL_TIMEOUT: Duration = Duration::from_millis(1500);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_SLICE_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_EVENT_STREAM_BYTES: usize = 1024;

const FIRMWARE_DIRECTORY: &str = "firmware";
const SFI_FILENAME: &str = "be200_intel_windows_24.60.0.4_usb_verified.sfi";
const DDC_FILENAME: &str = "ibt-0291-0291.ddc";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapOutcome {
    AlreadyOperational,
    Booted,
}

#[derive(Default)]
struct EventAssembler {
    pending: Vec<u8>,
}

impl EventAssembler {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > MAX_EVENT_STREAM_BYTES {
            return Err(AppError::Protocol(format!(
                "controller bootstrap HCI event stream exceeded {MAX_EVENT_STREAM_BYTES} bytes"
            )));
        }

        let mut events = Vec::new();
        while self.pending.len() >= 2 {
            let required = 2 + usize::from(self.pending[1]);
            if self.pending.len() < required {
                break;
            }
            events.push(self.pending.drain(..required).collect());
        }
        Ok(events)
    }
}

/// Ensure the external BE200 is running Operational firmware before any R2
/// Host-stack command opens Bumble.
pub fn ensure_operational() -> Result<BootstrapOutcome> {
    println!("[R2/BOOTSTRAP] probing Intel controller image state...");
    let version = read_version_once()?;

    match version.image_type {
        Some(0x03) => {
            println!(
                "[R2/BOOTSTRAP] controller already Operational (image=0x03); firmware transfer skipped"
            );
            Ok(BootstrapOutcome::AlreadyOperational)
        }
        Some(0x01) => {
            let (sfi, ddc) = default_firmware_paths()?;
            println!(
                "[R2/BOOTSTRAP] Bootloader detected (image=0x01); starting one guarded initialization"
            );
            println!("[R2/BOOTSTRAP] SFI: {}", sfi.display());
            println!("[R2/BOOTSTRAP] DDC: {}", ddc.display());
            println!("[R2/BOOTSTRAP] policy: pinned hashes, one attempt, no reset and no retry");

            let sfi = path_text(&sfi, "SFI")?;
            let ddc = path_text(&ddc, "DDC")?;
            boot::boot_once(&sfi, &ddc)?;
            println!("[R2/BOOTSTRAP] controller initialization: PASS");
            Ok(BootstrapOutcome::Booted)
        }
        other => Err(AppError::Protocol(format!(
            "R2 controller bootstrap requires Intel image Bootloader 0x01 or Operational 0x03; got {other:?}"
        ))),
    }
}

fn default_firmware_paths() -> Result<(PathBuf, PathBuf)> {
    let root = std::env::current_dir().map_err(|error| {
        AppError::Protocol(format!(
            "cannot resolve current working directory for .\\{FIRMWARE_DIRECTORY}: {error}"
        ))
    })?;
    Ok(firmware_paths_from(&root))
}

fn firmware_paths_from(root: &Path) -> (PathBuf, PathBuf) {
    let directory = root.join(FIRMWARE_DIRECTORY);
    (directory.join(SFI_FILENAME), directory.join(DDC_FILENAME))
}

fn path_text(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Protocol(format!(
            "{label} path is not valid Unicode: {}",
            path.display()
        )))
}

fn read_version_once() -> Result<intel::Version> {
    let handle = usb::open_be200_claimed()?;
    let result = read_version_from_claimed(&handle);
    let release = handle.release_interface(0);

    match (result, release) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(version), Ok(())) => {
            println!("[R2/BOOTSTRAP] release interface 0: OK");
            Ok(version)
        }
    }
}

fn read_version_from_claimed(handle: &DeviceHandle<GlobalContext>) -> Result<intel::Version> {
    let written = handle.write_control(
        0x20,
        0x00,
        0,
        0,
        &READ_VERSION_COMMAND,
        CONTROL_TIMEOUT,
    )?;
    if written != READ_VERSION_COMMAND.len() {
        return Err(AppError::Protocol(format!(
            "short Intel Read Version control write: {written}/{} bytes",
            READ_VERSION_COMMAND.len()
        )));
    }

    let deadline = Instant::now() + EVENT_TIMEOUT;
    let mut assembler = EventAssembler::default();
    let mut packet = [0u8; 260];
    let mut ignored = 0usize;

    while Instant::now() < deadline {
        let received = match handle.read_interrupt(0x81, &mut packet, READ_SLICE_TIMEOUT) {
            Ok(count) => count,
            Err(rusb::Error::Timeout) => continue,
            Err(error) => return Err(error.into()),
        };
        if received == 0 {
            continue;
        }

        for event in assembler.push(&packet[..received])? {
            if event.len() >= 6 && event[0] == 0x0E {
                let opcode = u16::from_le_bytes([event[3], event[4]]);
                if opcode == INTEL_READ_VERSION {
                    let status = event[5];
                    if status != 0 {
                        return Err(AppError::Protocol(format!(
                            "Intel Read Version returned HCI status 0x{status:02X}"
                        )));
                    }
                    let version = intel::parse_read_version_tlv(&event[6..])?;
                    println!(
                        "[R2/BOOTSTRAP] Intel image={} ({:?}); CNVi={:?} CNVr={:?} variant={:?}",
                        version
                            .image_type
                            .map(intel::image_description)
                            .unwrap_or("unknown"),
                        version.image_type,
                        version.cnvi_top,
                        version.cnvr_top,
                        version.hardware_variant()
                    );
                    return Ok(version);
                }
            }
            ignored = ignored.saturating_add(1);
        }
    }

    Err(AppError::Protocol(format!(
        "timed out waiting for Intel Read Version during controller bootstrap; ignored {ignored} unrelated event(s)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_paths_are_fixed_under_working_directory() {
        let root = Path::new("C:/repo/BE200AudioHub");
        let (sfi, ddc) = firmware_paths_from(root);
        assert_eq!(
            sfi,
            root.join("firmware")
                .join("be200_intel_windows_24.60.0.4_usb_verified.sfi")
        );
        assert_eq!(ddc, root.join("firmware").join("ibt-0291-0291.ddc"));
    }

    #[test]
    fn event_assembler_handles_split_and_concatenated_events() {
        let first = [0x0E, 0x04, 0x01, 0x05, 0xFC, 0x00];
        let second = [0xFF, 0x01, 0x02];
        let mut assembler = EventAssembler::default();
        assert!(assembler.push(&first[..3]).unwrap().is_empty());
        let mut rest = first[3..].to_vec();
        rest.extend_from_slice(&second);
        assert_eq!(assembler.push(&rest).unwrap(), vec![first.to_vec(), second.to_vec()]);
    }
}
