# R1.3a — BE200 Native Driver Preflight

This stage **does not bind WinUSB**. It does not touch the current PnP driver, Windows Bluetooth stack, USB power, or MT7925.

## Read-only status

```powershell
cargo run -- driver status
```

The tool locates **exactly one PRESENT** `USB\VID_8087&PID_0036\...` instance and emits a JSON record with service, bound INF, provider, version, parent and hardware IDs. Any 0/multiple matches fail closed. It uses Windows PowerShell's built-in `Get-PnpDevice` and `Get-PnpDeviceProperty`, not raw Intel HCI.

## Export the currently bound Intel OEM driver

In an administrator PowerShell:

```powershell
cargo run -- driver backup
```

Default output:

```text
%LOCALAPPDATA%\BE200AudioHub\driver-backups\BE200-<timestamp>-<pid>\
  be200-pnp.json
  all-properties.json
  bluetooth-inventory.json
  pnputil-device.txt
  pnputil-export.txt
  exported-driver\...
  sha256.csv
  RESTORE.md
```

Custom **parent** directory (each call still gets its own run subfolder):

```powershell
cargo run -- driver backup --out "C:\STanJK\Development\LEAudioLab\BE200\backups"
```

The backup refuses to proceed unless the selected device is unique, PnP OK, service `BTHUSB`, and associated INF is `oemNN.inf` from an Intel provider. `pnputil /export-driver` writes a package copy; it does **not** install/disable any driver.

`RESTORE.md` explains the manual recovery procedure. Installing an exported OEM driver is **not** guaranteed to overrule a future competing WinUSB INF; verify device binding explicitly during any rollback.

## Before any R1.3b WinUSB swap

1. Confirm backup and driver-store export succeeded, including SHA256.
2. Record the exact BE200 InstanceId and make sure MT7925 is **not** the target.
3. Ensure MT7925 Windows Bluetooth/LE Audio is still recoverable.
4. Develop separate **targeted** WinUSB bind and rollback implementation.
5. Switch **only** after another explicit confirmation. Do not manipulate the front USB2 header or power wiring during driver changes.

**Data warning:** The backup contains machine-specific PnP InstanceIds and configuration. Keep private; do not publish raw logs or captures.
