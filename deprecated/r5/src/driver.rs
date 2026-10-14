//! R1.3a: inspect and back up the *existing* BE200 driver binding.
//! All actual PnP/driver state changes belong to a later, separately reviewed milestone.
use crate::error::{AppError, Result};

#[cfg(windows)]
fn execute(action: &str, backup_root: Option<&str>) -> Result<()> {
    use std::process::Command;

    let mut command = Command::new("powershell.exe");
    command
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(include_str!("../scripts/driver-preflight.ps1"))
        .env("BE200_DRIVER_ACTION", action);

    if let Some(root) = backup_root {
        command.env("BE200_BACKUP_ROOT", root);
    }

    // PowerShell is allowed to *query* PnP and, for backup, export an OEM driver
    // package to disk. It is not allowed to install or switch a device driver.
    let output = command
        .output()
        .map_err(|e| AppError::Driver(format!("failed to launch Windows PowerShell: {e}")))?;

    if !output.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        return Err(AppError::Driver(format!(
            "driver {action} failed (PowerShell exit code: {:?}); no driver binding was changed",
            output.status.code()
        )));
    }
    log::info!(target: "be200hub::driver", "R1.3a {action} completed");
    Ok(())
}

#[cfg(not(windows))]
fn execute(_action: &str, _backup_root: Option<&str>) -> Result<()> {
    Err(AppError::Driver(
        "driver inspection / backup requires Windows".into(),
    ))
}

pub fn status() -> Result<()> {
    execute("status", None)
}

pub fn verify() -> Result<()> {
    execute("verify", None)
}

pub fn backup(root: Option<&str>) -> Result<()> {
    if root.is_some_and(|path| path.trim().is_empty()) {
        return Err(AppError::Usage("--out requires a nonempty directory".into()));
    }
    execute("backup", root)
}
