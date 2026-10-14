# R1.5a — Intel firmware structural inspection (offline)

**No USB/HCI calls or firmware transfers are performed.** This is a precursor to R1.5b and MUST NOT be described as complete cold boot.

## Inputs

The private project reference assets, supplied by the maintainer:

- `ibt-0291-0291.sfi` — 978,976 bytes; SHA256 `ab04144f4b05dbd7baa2e3b0def81b87e27652accc35193582468f570f606018`
- `ibt-0291-0291.ddc` — 4 bytes; SHA256 `e0fb10fe29f60607d6507ced3f697492d9611f62c4765feaee59c4bd4101d3e7`

SHA256 values identify **these exact uploaded files**, not an upstream pinned release or proof of cryptographic Intel signing. Keep raw firmware out of GitHub; license restrictions remain applicable.

## Commands

```powershell
cargo test
cargo run -- firmware inspect "C:\STanJK\Development\LEAudioLab\BE200\references\firmware\intel\ibt-0291-0291.sfi" "C:\STanJK\Development\LEAudioLab\BE200\references\firmware\intel\ibt-0291-0291.ddc"
```

Use the files' real local paths. The sample location is not automatically created by this program.

## What it validates

- First CSS header: RSA version `0x00010000` or hybrid `0x00069700`.
- For RSA + ECDSA dual header, ECDSA CSS version `0x00020000` at `644 + 8`; embedded command buffer begins at offset `964`.
- Embedded command triplets: little-endian opcode `u16`, parameter length `u8`, then parameters.
- Linux `btintel_download_firmware_payload` 4-byte alignment grouping and proposed <=252-byte Secure Send DATA fragments. Each DATA fragment represents a **slice of SFI source bytes**, not a complete Intel USB command.
- Optional Intel Write Boot Parameters opcode `0xFC0E` and its boot address/build date.
- DDC records: one-byte record length, two-byte ID, optional value. `03 28 01 18` is valid and represents ID `0x0128` with one value byte.

For the uploaded reference image, an independent offline parse found:

| Property | Expected |
|---|---:|
| SFI size | 978,976 bytes |
| Header kind | RSA + ECDSA |
| Command-buffer start | 964 |
| Embedded HCI commands | 3,959 |
| 4-byte aligned data groups | 3,950 |
| Planned DATA Secure Send fragments | 3,950 |
| Boot address | `0x00100800` |
| DDC records | 1 |

**Not equivalent to the 3,960 Secure Sends counted in the previous Windows 003 capture.** The Windows-transmitted firmware may differ in build or version and header fragment selection; counts alone do not prove an exact byte match.

## R1.5b prerequisites

1. Read true BE200 cold-boot Intel Version TLVs, including actual `img_type`, SBE type and hardware variant.
2. Select an appropriate Intel-signed SFI build, source/license and exact hash.
3. Verify **which header signature sequence** applies (RSA, ECDSA, or hybrid), rather than blindly replaying the traffic.
4. Implement bounded, checked HCI Secure Send transport and acknowledgment/status handling; bootloader-specific Bulk OUT handling differs from standard EP0 HCI commands.
5. Wait for download and boot completion events; only apply DDC once operational.
6. Validate safe power and recovery before any new true cold-boot attempt.

Source authority: `shared_resource/btintel.c` from the project quarry, particularly `btintel_sfi_rsa_header_secure_send`, `btintel_sfi_ecdsa_header_secure_send`, `btintel_download_firmware_payload` and `btintel_load_ddc_config`. Linux code is a reference only; it has not been copied into this project.

## R1.5b follow-on

Run `firmware plan --sfi <path> --ddc <path>` with the recovered **Windows 24.60.0.4 SFI** and the 4-byte DDC to check the fully offline first-boot plan and the 003-PCAP-exact USB wire hash. The earlier Linux file continues to work with `firmware inspect`, but is deliberately rejected by `firmware plan`. No USB writes occur.

A read-only `firmware preflight --sfi <path> --ddc <path>` checks the Windows image, verifies WinUSB and queries the current controller image without firmware download. A separate `firmware boot --sfi <path> --ddc <path> --confirm-cold-boot` command is available but **NOT hardware-validated**. It fails closed unless the controller is in the expected BE200 Bootloader state. Read `FIRMWARE_BOOT.md` and pass offline/local tests first.

## Windows 24.60.0.4 live boot profile

The recommended first independent BE200 boot profile is now the **recovered Windows OEM signed image** `be200_intel_windows_24.60.0.4_usb_verified.sfi` (980840 bytes, SHA256 `939a9b32ad11d8962abfd10cb24d5bd90d7848218af4ea438cf6490243a618d4`), not the Linux Build 145 image. The Linux image remains useful for comparative offline `firmware inspect`, but is deliberately rejected by `firmware plan/preflight/boot`.

The Windows image yields **3967 embedded commands / 3957 DATA fragments / 3960 total Secure Sends**, 996036 HCI-wire bytes, wire SHA256 `55200bef2efadd869500baf58909623fbf23c8d26e6218458cbf7a1fd8923531`; all 3960 packets were independently compared byte-for-byte to the supplied USBPcap capture with zero mismatches. The existing four-byte `ibt-0291-0291.ddc` contains the same ID `0x0128` record transmitted by the Windows driver.
