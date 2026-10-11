# R1.5b — BE200 Windows 24.60.0.4 independent cold boot

## Status

- R1.4 independent HCI Read Version: **validated on user's BE200**.
- R1.5a offline SFI/DDC structure: **validated**.
- R1.5b Windows wire reconstruction: **3960 out of 3960 HCI 0xFC09 packets match** `003-ColdBoot.pcapng`, including ECDSA headers and DATA, using an independent offline Python comparison.
- R1.5b Rust loader, offline wire-hash oracle, read-only preflight and guarded boot: **implemented, local cargo tests still required**.
- **Independent Rust cold boot on hardware has NOT been tested.**

## Image provenance and compatibility

This first-loader experiment exclusively accepts the Windows 24.60.0.4 **SFI-format Intel-signed image**, extracted from the operator's actual Intel `ibtusb.sys` (original OEM driver `oem31.inf`). The reference capture establishes the same exact ECDSA signature + DATA bytes were previously accepted by this same BE200 in Windows.

- Local filename assigned by the extraction tool: `be200_intel_windows_24.60.0.4_usb_verified.sfi`
- Size: **980840 bytes**
- SFI SHA256: `939a9b32ad11d8962abfd10cb24d5bd90d7848218af4ea438cf6490243a618d4`
- ECDSA Secure Send plan: 3 header packets, 3957 DATA packets
- Concatenated 3960 packets in HCI-wire format: **996036 bytes**, SHA256 `55200bef2efadd869500baf58909623fbf23c8d26e6218458cbf7a1fd8923531`
- Embedded HCI commands: **3967**
- Embedded boot address: `0x00100800`
- SFI build metadata: build **176**, week **26**, year **2026**
- DDC file: `ibt-0291-0291.ddc`, 4 bytes `03 28 01 18`, SHA256 `e0fb10fe29f60607d6507ced3f697492d9611f62c4765feaee59c4bd4101d3e7`
- Hardware expected: `USB\VID_8087&PID_0036`, CNVi/CNVr TOP `0x02001910`, variant `0x1C`, Bootloader image `0x01`, Secure Boot `0x01`, SBE `0x01` ECDSA.
- Windows WinUSB driver binding observed: `oem17.inf`, service `WinUSB`. Original Intel backup `oem31.inf` available.

The Windows SFI **must NOT be checked into the repository**. The 4-byte Linux-named DDC is also present in the Windows hardware trace; do not conflate that common config record with all OEM Windows-specific initialization commands. File hash verification does **not** independently verify Intel's public-key firmware signature.

## On-wire sequence (from Windows 003)

1. Read Intel version `0xFC05 FF` over EP0, ensure Bootloader, ECDSA SBE and matching hardware.
2. Preboot Intel DDC `0xFC8B 03 86 01 01` (observed frame 1957, success).
3. ECDSA INIT 128 B, PKEY 96 B, SIGN 96 B; then 3957 aligned DATA fragments via `0xFC09` on Bulk OUT EP02. Wire sequence matches the 3960 captured packets **exactly**.
4. Wait Intel vendor Firmware Download Complete `0xFF/0x06` and require success.
5. Intel Boot `0xFC01` to the embedded address `0x00100800`, wait Intel `0xFF/0x02`.
6. Send Intel DSBR-default command `0xFC0A 00 00` (Windows frame 9917).
7. Re-read `0xFC05`, require image `0x03` Operational.
8. Postboot DDC `0xFC8B 03 28 01 18`; confirm Command Complete.

Windows later sends additional platform-specific vendor commands, including telemetry and OEM configuration. These **are not part of the minimal firmware boot** and are not indiscriminately replayed. Finishing steps 1–8 proves controller boot only, not fully functioning LE Audio Host.

## Stage A: validation while already Operational (NO power operation)

Use unprivileged VS Code PowerShell:

```powershell
cd C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub
git pull --ff-only
cargo test

$fw = 'C:\STanJK\Development\LEAudioLab\BE200\firmware'
$windowsSfi = Join-Path $fw 'be200_intel_windows_24.60.0.4_usb_verified.sfi'
$ddc = Join-Path $fw 'ibt-0291-0291.ddc'

Get-FileHash -Algorithm SHA256 $windowsSfi
Get-FileHash -Algorithm SHA256 $ddc

cargo run -- firmware inspect "$windowsSfi" "$ddc"
cargo run -- firmware plan --sfi "$windowsSfi" --ddc "$ddc"
cargo run -- firmware preflight --sfi "$windowsSfi" --ddc "$ddc"
```

The recovered Windows SFI can be found in the private reconstruction archive from this project. Copy it into `BE200\firmware` using the exact assigned filename above. Do not rename Linux `ibt-0291-0291.sfi` and pretend it is the Windows firmware.

Expected Stage A behavior: `cargo test` passes, `firmware plan` prints 3960 sends / 996036 wire bytes and `EXACT MATCH`, and read-only `firmware preflight` reports **ALREADY OPERATIONAL**. The preflight only sends Intel HCI Read Version `0xFC05`, **not** firmware writes.

**DO NOT power cycle until Stage A console output has been reviewed.**

## Stage B: first cold boot (operator-controlled power)

This applies only to the user's custom externally powered BE200 experiment. BE200 is powered by a separately controlled 3.3V supply and USB2 D+/D− are connected to a modified front-panel connector shared with the MT7925 on a separate USB port. That arrangement has electrical back-power, grounding, accidental short and system USB-hub risks. A previous working power cycle does not prove universal safety.

Only after Stage A passes, and after the operator independently confirms a stable 3.3V output, uninterrupted shared ground, insulation, USB back-power isolation, and a safe cutoff method **without manipulating live flywires or the shared data header**:

1. Ensure no application holds BE200 and native Intel drivers are not reinstalled.
2. Optional: start a USBPcap1 trace, ideally filtered to the BE200 device. Do not publicly share unfiltered HID capture.
3. Switch BE200 external 3.3V OFF **once** through the existing controlled supply; do not touch the USB data lines. Ensure the power rail is truly down, with no evidence of USB data-line back-power. Stop immediately if this is uncertain.
4. Switch the supply ON **once** and wait for WinUSB target `8087:0036` to enumerate.
5. Run read-only preflight:
   ```powershell
   cargo run -- firmware preflight --sfi "$windowsSfi" --ddc "$ddc"
   ```
   Expected: `BOOTLOADER READY`, image `0x01`, variant 0x1C, SBE 0x01. If not ready, **stop**, do not automatically retry/power-cycle.
6. When the operator has reviewed this outcome, run the **one shot** hardware loader:
   ```powershell
   cargo run -- firmware boot --sfi "$windowsSfi" --ddc "$ddc" --confirm-cold-boot
   ```
7. On success confirm `cargo run -- hci version` reports Operational image `0x03`.

Any HCI, driver, USB or firmware error: **stop** and preserve output/logs. Do not retry transfer, manually power cycle repeatedly, reset device, or blindly reinstall drivers. Contact the development session with log/USBPcap evidence first.

## Implementation limits

- Fails closed on **exact-image SHA256**, PCAP **wire SHA256**, hardware variant, Bootloader image type and ECDSA SBE type.
- No in-app supply control, forced reset, loops/retries, driver installation, unsafe generic opcode injector or MT7925 modifications.
- No independent RSA/ECDSA cryptographic signing check.
- Does not reproduce Windows host's entire SAR/telemetry/identity initialization. This is firmware bring-up, not Windows Bluetooth stack compatibility.
- The controller can stay Operational after the loader exits as long as its internal power domain/firmware state persists. Only an actual loss of controller power is expected to require reloading; a normal exit from the Rust program does **not** require power-cycling. A USB port-cycle or warm host reset may or may not preserve firmware depending on power state.

References: the private 003-ColdBoot PCAP (BE200-only parsed commands), original Intel 24.60.0.4 exported driver, independent firmware extraction package, and Linux btintel.c/btusb.c as an algorithm reference (no Linux GPL code copied).
