# R1.5b — Independent Intel BE200 Cold Boot

## Status / safety contract

- Source-level loader and offline tests: **implemented**.
- Real BE200 independent cold boot: **NOT YET VERIFIED**. Only the maintainer's Windows rig can validate it.
- This is a one-off experimental boot of the controller's **RAM-loaded Intel-signed operational firmware**, not an arbitrary flash erase/write tool.
- The supplied SFI is pinned to SHA256 `ab04144f4b05dbd7baa2e3b0def81b87e27652accc35193582468f570f606018`, DDC to `e0fb10fe29f60607d6507ced3f697492d9611f62c4765feaee59c4bd4101d3e7`.
- Hash matching is file identity, **not independent Intel public-key signature verification**. The controller's hardware Secure Boot Engine authorizes or rejects the signed firmware.

This experiment's BE200 hardware has an externally controlled USB 5V→3.3V supply, with an independently routed USB2 data pair, attached to a custom shared front-panel header with the MT7925. The lab has once successfully performed a hardware power cycle without damage. That past outcome does **not** establish hotplug/power-sequence compliance. Do not manipulate live flywires or the shared header; power stability, common ground and output isolation remain operator responsibilities.

## Protocol evidence (do not confuse Windows firmware payload with Linux firmware image)

Reference capture `003-ColdBoot.pcapng`:
- HCI Read Version `0xFC05` on EP0, parameter `FF`.
- Bootloader TLVs: image 0x01, CNVi/CNVr TOP 0x02001910, CNVi BT 0x001C3700 → variant 0x1C, secure_boot=1, SBE_TYPE (TLV **0x2F**) =1, ECDSA.
- Secure Send HCI opcode 0xFC09 routed over **Bulk OUT 0x02**, not EP0.
- ECDSA fragment sequence: INIT type 0x00 (128 bytes), PKEY 0x03 (96 bytes), SIGN 0x02 (96 bytes), then DATA type 0x01 (up to 252 bytes each). The payload is assembled from embedded HCI commands using 4-byte alignment boundaries, as in Linux `btintel_download_firmware_payload`.
- Intel download confirmation: Interrupt IN 0x81, `FF 05 06 00 00 00 00` = vendor subtype 0x06, success.
- Boot/Reset command: EP0, opcode 0xFC01, 8 params `00 01 00 01 00 08 10 00`. The Intel bootloader does **not** send a regular Command Complete for this reset.
- Boot completion event: `FF 07 02 00 01 01 02 FF 01`, vendor subtype 0x02.
- After boot, Read Version reports image type 0x03 = Operational; `0xFC8B` writes DDC `03 28 01 18`.

Caveat: 003 Windows capture contained **3,960** 0xFC09 packets. The current pinned Linux SFI produces **3,953** planned sends: 3 ECDSA signature/header sends + 3,950 DATA sends. Counts differ because firmware builds/contents may differ. This loader never blindly replays the old PCAP.

## First, test offline and under normal Operational state

Run these from the unprivileged VS Code PowerShell, while BE200 is powered and bound to WinUSB:

```powershell
cd C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub
git pull --ff-only
cargo test
$fw = 'C:\STanJK\Development\LEAudioLab\BE200\firmware'
cargo run -- firmware inspect "$fw\ibt-0291-0291.sfi" "$fw\ibt-0291-0291.ddc"
cargo run -- firmware plan --sfi "$fw\ibt-0291-0291.sfi" --ddc "$fw\ibt-0291-0291.ddc"
cargo run -- driver verify
cargo run -- hci version
```

Verify `hci version` reports Operational 0x03, expected TOP and SBE details. It is valid for the new boot command to **refuse** to start while the controller is operational; there is deliberately no reset-to-bootloader function.

## Physical cold boot: operator only, with explicit decision

Only proceed if the maintainer has inspected the hotwire rig, decided its power cycling is acceptable, verified the supply's **3.3V**, persistent common GND, isolation and stable output, and has access to disconnect power safely without touching the shared data header. Controller power OFF/ON is **not controlled by be200hub**.

1. Ensure firmware files are verified offline and no previous HCI/USB process holds BE200. The BE200 is already WinUSB-bound; do not reinstall Intel's Bluetooth driver.
2. Start optional USBPcap1 capture *before* the power cycle if an independent trace is desired; don't publish full hub captures (HID/keyboard privacy).
3. Switch the **existing** BE200 3.3 V control box OFF once, without touching the 2-pin USB data cable, flywires or modified dual-port front connector. Ensure GND stays connected, measure BE200-side rail only using preattached insulated probes if possible. Stop if rail is held up by USB back-power or an electrical anomaly occurs.
4. Only once safely discharged and ready, switch ON once. Verify supply stable, no abnormal current/heat, and WinUSB `8087:0036` enumerated.
5. Run `cargo run -- hci version`. **Expected image 0x01 Bootloader** and SBE 0x01. If image 0x03 or the USB device does not appear, **stop** and capture the output; do not retry power cycling.
6. When the operator has confirmed the Bootloader is healthy, execute the single test below. It refuses to write on a mismatched USB/firmware/controller security state.

```powershell
cargo run -- firmware boot --sfi "$fw\ibt-0291-0291.sfi" --ddc "$fw\ibt-0291-0291.ddc" --confirm-cold-boot
```

This is **the first point at which the BE200AudioHub program will intentionally send firmware / boot commands to hardware**.

After success:
```powershell
cargo run -- hci version
```

Expected image type 0x03. If there is a timeout, USB error, unexpected state, failed Secure Send, failure to boot or abnormal power, **stop**, keep logs, and do not re-run the command. The program does not attempt an automatic recovery/reset; the operator must decide what to do after diagnosing.

## Guard rails actually enforced by the code

1. Pin exact SHA256 of reference SFI and DDC; validate firmware structure and boot address before opening USB.
2. Verify a **single** present BE200 `8087:0036` and Windows service `WinUSB`.
3. Query version `0xFC05` and require image=0x01, CNVi/CNVr TOP 0x02001910, hw variant=0x1C, Secure Boot=1, SBE=0x01 (ECDSA).
4. Only then send one sequential 0xFC09 plan over EP02. Bound fragment size to 252 bytes, check every USB completion/length, global deadline, and wait for vendor download result.
5. Send 0xFC01 **once** using the known embedded boot address; wait for vendor event; require new Intel Read Version image=0x03.
6. Apply single DDC via 0xFC8B, check response; release interface 0 and exit.
7. No arbitrary opcodes or unbounded fallback/retry.

Source references: original private 003 PCAP and analysis; `shared_resource/btintel.c`, `shared_resource/btintel.h`, `shared_resource/btusb.c` in the LEAudioLab quarry. The public Linux driver is an algorithm reference, **not code copied verbatim**.

## If the first boot fails

Do not attempt to unbind WinUSB, toggle raw power repeatedly, or force an Intel firmware rollback mid-command. Save the console output and `%LOCALAPPDATA%\BE200AudioHub\logs\be200hub.log`, any BE200-only trace (with personal data removed), plus PnP status. Physical recovery strategy must be decided after inspecting which stage failed. The exported native Intel `oem31.inf` remains the manual fallback for a later controlled driver restoration.
