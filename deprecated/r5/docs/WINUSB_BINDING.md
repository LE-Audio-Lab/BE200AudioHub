# R1.3b — Bind BE200 to WinUSB (manual, reversible)

**Current implementation:** No automatic driver installer or uninstaller. WinUSB installation is a privileged, manual operator action. The CLI can verify the result and safely open/claim/release interface 0. **It does not yet send HCI commands.**

## 0. Evidence-based guard

The verified device, as of 2026-10-11:

- Intel BE200 Bluetooth: **USB VID:PID 8087:0036**, PnP service **BTHUSB**, native Intel OEM INF **oem31.inf**, driver 24.60.0.4
- Original parent port path: `PCIROOT(0)#PCI(1400)#USBROOT(0)#USB(14)`
- MediaTek MT7925: **a DIFFERENT adapter** under a DIFFERENT USB port. Do not install WinUSB on MediaTek.
- The two radios occupy separate logical USB ports of the same manually modified front-panel connector. **Do not disconnect this combined header live.**

## 1. Prerequisites — no changes until all are true

1. Intel native package already exported successfully by `driver backup`; preserve its exported .inf, .sys, and accompanying files.
2. Keep the backup directory, e.g. `C:\STanJK\Development\LEAudioLab\BE200\backups\BE200-20261011-201450-27432`, outside the Git repository.
3. Verify the original instance ID, port path, native service/driver and hardware status with `cargo run -- driver status`.
4. Confirm a second functioning keyboard/mouse path; do not make hub/controller changes or disable Windows driver signature enforcement.
5. Close any program actively trying to use BE200. Leave MT7925 alone.

## 2. Manual WinUSB binding (administrator, one-time)

Download **Zadig** from its official site: https://zadig.akeo.ie/

1. Launch Zadig elevated; `Options → List All Devices`.
2. Select **Intel(R) Wireless Bluetooth(R)** with **USB ID 8087 / 0036**.
3. Verify these IDs *in Zadig* before continuing. If the selected item is a `USB Root Hub`, `USB Composite Device`, `MediaTek Bluetooth Adapter`, `0E8D:...`, or an unexpected `MI_..` interface, **STOP**.
4. Select **WinUSB** as the new driver, not libusb-win32 or libusbK.
5. Use `Replace Driver` on this exact BE200 entry and respond to the UAC prompt. Do not select `List Hubs` or replace an xHCI controller.
6. If Windows reports a signature error, install failure or requires reboot, stop and record exactly what happened. Do **not** use TESTSIGNING, disable Secure Boot, or blindly retry on other devices.

The Bluetooth radio may disappear from Bluetooth-class Device Manager and reappear under USB devices; this is expected when WinUSB replaces BTHUSB. Windows should no longer own this controller.

**Important:** Zadig's WinUSB driver replacement persists as a Windows PnP/driver-store choice. A mere USB replug or PC restart does not guarantee automatic Intel restoration.

## 3. Validate using an ordinary VS Code PowerShell

```powershell
cd C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub
git pull --ff-only
cargo test
cargo run -- driver status
cargo run -- driver verify
cargo run -- usb probe
```

- `driver verify` requires **exactly one present** 8087:0036 target, PnP OK and its current Windows service `WinUSB`; otherwise it exits without attempting to access the device.
- `usb probe` repeats this guard, checks the USB class E0/01/01, opens the device via libusb, claims **interface 0**, and releases it. No HCI traffic, USB reset, firmware download or power cycle occurs.
- If `driver verify` succeeds but `usb probe` returns libusb `Access` or `NotFound`, STOP. Preserve raw output plus `pnputil /enum-devices /instanceid '<exact device id>' /services /stack /drivers` for diagnosis. Do not touch MT7925.
- Record which adapter Windows actually uses if MT7925 is enabled/disabled; do not assume either radio active solely from Settings UI.

## 4. Manual restoration of Intel Bluetooth if needed

Use the recorded original BE200 instance and the exported driver package. **Do not issue generic** `pnputil /delete-driver /uninstall /force`, `/disable-device /class Bluetooth`, or all-device driver updates.

1. Stop BE200AudioHub or any WinUSB user process; do not hot-unplug the custom USB2 header.
2. In Device Manager find **the exact 8087:0036 instance** (possibly under "Universal Serial Bus devices"), verify its Hardware IDs and Instance Path.
3. `Update driver → Browse my computer → Let me pick → Have Disk`, point to the `*.inf` from the saved **Intel `exported-driver`** directory.
4. Select the compatible Intel Bluetooth driver for this device, follow Windows' prompts, and restart Windows if necessary.
5. Verify `cargo run -- driver status` returns service **BTHUSB**, Intel provider / native driver and no PnP problem. Confirm relevant Intel filter-stack behavior and MT7925's untouched driver/availability.

If the Intel INF cannot be selected or Windows insists on the WinUSB INF, stop and inspect the device-specific matching drivers/rank via:

```powershell
pnputil /enum-devices /instanceid 'USB\VID_8087&PID_0036\5&47B74AE&0&14' /drivers
```

**No guarantee:** An exported OEM package is not a complete Windows restore point. Microsoft documents that `pnputil /add-driver /install` does *not* force a lower-ranked package. Do not assume automatic rollback is possible. Restore via Device Manager or repair the original Intel package under controlled conditions.

## 5. Exit criteria

R1.3b is complete only once:

- Intel OEM driver backup exists and is accessible.
- BE200 binds to WinUSB, verified by `driver verify`.
- `usb probe` succeeds: open / interface 0 claim / release.
- MT7925 and other USB peripherals remain unaffected.
- Restore path is documented and can be exercised later.

References:

- https://github.com/libusb/libusb/wiki/Windows
- https://zadig.akeo.ie/
- https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/pnputil-command-syntax
- https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation
