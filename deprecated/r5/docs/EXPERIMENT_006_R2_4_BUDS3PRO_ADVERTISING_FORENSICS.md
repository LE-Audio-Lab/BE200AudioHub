# Experiment 006 — R2.4 Galaxy Buds3 Pro advertising / identity forensics

Date: 2026-10-12
Branch: `r2/stack-integration-bootstrap`
Target: Galaxy Buds3 Pro `Galaxy Buds3 Pro (2F23)`
Controller path: Intel BE200 `8087:0036`, Bumble-RS external HCI transport, passive LE 1M extended scan

## Scope and evidence handling

This document archives the six-state passive advertising capture used to determine which Galaxy Buds3 Pro advertising surface should be used for R2.4 ACL/SMP onboarding. The capture command explicitly performed **no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO**.

The operator reused previous PowerShell commands for some runs, so filenames are not authoritative for runs 03 and 04. The state labels below are reconstructed from **execution order**, per the operator's explicit instruction. The complete pasted terminal log is preserved verbatim in Appendix A.

## Execution-order matrix

| Run | Physical state | Scan result |
|---|---|---:|
| 00 | case closed; both earbuds in case | 0 reports / 0 retained |
| 01 | case open; both earbuds still in case; not pairing | 1 reports / 1 retained |
| 02 | formal pairing mode; both earbuds in case | 3 reports / 3 retained |
| 03 | left earbud out (execution-order label; filename was reused) | 5 reports / 5 retained |
| 04 | right earbud out (execution-order label; filename was reused) | 5 reports / 5 retained |
| 05 | both earbuds out | 3 reports / 3 retained |

## High-level evidence

1. **Case closed:** no advertisements were retained.
2. **Case open, not pairing:** one unnamed, zero-payload, type-0/public-class report appeared.
3. **Formal pairing mode:** three advertisers appeared: one pairing-only RPA with Microsoft company ID `0x0006`, plus two full LE Audio advertisements using public addresses.
4. **After leaving formal pairing and taking earbuds out:** the two LE Audio advertisements were observed using RPAs instead of the two pairing-mode public addresses.
5. The public/RPA forms can be paired into two stable member families by exact RSI equality.

## Member-family mapping by RSI

| Family | Formal-pairing address | Post-pairing/current RPA | RSI | Local name | Key advertised LE Audio IDs |
|---|---|---|---|---|---|
| A | `E8:C9:13:EF:2F:23/P` | `41:95:2A:3E:63:B2` | `5C:06:D1:6A:01:41` | Galaxy Buds3 Pro (2F23) | `0x1853` CAS, `0x184E` ASCS, `0x1850` PACS, `0x1844` VCS, `0x184F`, `0x184D` |
| B | `E8:C9:13:F6:5C:E0/P` | `42:AD:FE:E2:A5:43` | `32:3C:A2:A6:02:79` | Galaxy Buds3 Pro (2F23) | same service set |

The RSI correlation is direct capture evidence: family A keeps `5C:06:D1:6A:01:41` across its public and RPA forms; family B keeps `32:3C:A2:A6:02:79` across its public and RPA forms.

Additional payload differences worth retaining for later analysis:

- Family A formal-pairing public advertisement: Flags `0x19`; CAS service-data payload `00`; ASCS service-data begins `00:6F:00:6B:00:00`; proprietary 128-bit service-data tail ends in `...:2F:08`.
- Family B formal-pairing public advertisement: Flags `0x04`; CAS payload `00`; ASCS begins `00:6F:00:6B:00:00`; proprietary 128-bit service-data tail ends in `...:2F:09`.
- Family A RPA advertisement: Flags `0x18`; CAS payload `01`; ASCS begins `01:6F:00:6B:00:00`; proprietary tail ends in `...:2F:0C`.
- Family B RPA advertisement: Flags `0x04`; CAS payload `01`; ASCS begins `01:6F:00:6B:00:00`; proprietary tail ends in `...:2F:0D`.
- The 128-bit service-data UUID bytes are `3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7`; canonicalized from BLE little-endian UUID byte order this is `a7a473e9-19c6-491b-aea6-7ea92b8f043a`. No semantic assignment is asserted here.

## Pairing-only 0x0006 advertiser

During formal pairing mode only, the capture contained:

- Address: `5B:C5:06:09:94:D8`
- HCI address type: `1`, resolvable private address (RPA)
- RSSI: `-39 dBm`
- PHY: `1/2`
- SID: `1`
- Connectable: `true`
- Scannable: `false`
- Local name AD: absent
- RSI: absent
- Raw advertising length: `37 B`
- Manufacturer specific data: company ID bytes `06:00` = `0x0006`; payload begins `03:02:80:04:44:24` and contains the UTF-8 display name `Galaxy Buds3 Pro (2F23)`
- Appearance: `0x0941`
- Capture-time decoder printed company `0x0006` as `unknown-company`; the decoder was corrected after this capture to label `0x0006` as Microsoft.

Working label for subsequent experiments: **pairing-only Microsoft/Swift-Pair candidate**. This label is a working interpretation; the raw bytes above are the primary evidence.

## Unnamed public-address-class reports

These reports are intentionally **not** dismissed as a Bumble-RS artifact. The operator reports independently observing the same class of unnamed/public-looking behavior on Fedora with an Intel AX210. That prior Fedora observation is not part of this capture file and remains an operator-supplied cross-platform observation.

Common properties in this capture:

- HCI address type reported as `0` / public device address.
- Extended advertising SID `3`, PHY `1/2`.
- `connectable=true`, `scannable=false`.
- No local name, no RSI, no retained AD structures, `raw=0B`.
- Addresses change between runs and several share suffix `:18:46`.
- They occur in case-open / single-ear-out / both-out states, but none appeared in the formal-pairing capture.

### Complete unnamed-address list

| Address | Runs | RSSI observations | Type | SID | Payload |
|---|---|---|---|---:|---|
| `1D:1B:3C:93:18:46/P` | 01 | -38 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `1D:12:31:88:18:46/P` | 03 | -52 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `27:0F:31:88:18:46/P` | 03 | -51 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `27:10:31:88:18:46/P` | 03 | -51 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `13:14:39:43:18:46/P` | 04 | -44 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `1D:0B:39:43:18:46/P` | 04 | -54 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `27:11:33:4C:18:46/P` | 04 | -44 dBm | 0/public | 3 | 0 B; unnamed; no RSI |
| `1D:0A:30:8E:18:46/P` | 05 | -61 dBm | 0/public | 3 | 0 B; unnamed; no RSI |

Status: **unresolved**. Candidate explanations (Samsung OEM behavior, controller/firmware report semantics, extended-advertising report composition, or host parsing) are deliberately left open for the next analysis round.

## Complete address / ID catalog

| Address | Classification | Runs | Type | RSSI(s) | SID | RSI | Name | Raw bytes |
|---|---|---|---|---|---:|---|---|---:|
| `1D:1B:3C:93:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 01 | 0: public device address | -38 dBm | 3 | `absent` | (unnamed) | 0 B |
| `5B:C5:06:09:94:D8` | Pairing-only RPA; Microsoft company-ID 0x0006 manufacturer payload; working label: Swift Pair candidate | 02 | 1: random device address / resolvable-private(RPA) | -39 dBm | 1 | `absent` | (unnamed) | 37 B |
| `E8:C9:13:EF:2F:23/P` | LE Audio member family A — public/identity-looking advertisement during formal pairing | 02 | 0: public device address | -43 dBm | 3 | `5C:06:D1:6A:01:41` | Galaxy Buds3 Pro (2F23) | 143 B |
| `E8:C9:13:F6:5C:E0/P` | LE Audio member family B — public/identity-looking advertisement during formal pairing | 02 | 0: public device address | -50 dBm | 3 | `32:3C:A2:A6:02:79` | Galaxy Buds3 Pro (2F23) | 143 B |
| `1D:12:31:88:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 03 | 0: public device address | -52 dBm | 3 | `absent` | (unnamed) | 0 B |
| `27:0F:31:88:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 03 | 0: public device address | -51 dBm | 3 | `absent` | (unnamed) | 0 B |
| `27:10:31:88:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 03 | 0: public device address | -51 dBm | 3 | `absent` | (unnamed) | 0 B |
| `41:95:2A:3E:63:B2` | LE Audio member family A — RPA advertisement outside formal pairing | 03, 04, 05 | 1: random device address / resolvable-private(RPA) | -55, -43, -55 dBm | 3 | `5C:06:D1:6A:01:41` | Galaxy Buds3 Pro (2F23) | 143 B |
| `42:AD:FE:E2:A5:43` | LE Audio member family B — RPA advertisement outside formal pairing | 03, 04, 05 | 1: random device address / resolvable-private(RPA) | -48, -48, -65 dBm | 3 | `32:3C:A2:A6:02:79` | Galaxy Buds3 Pro (2F23) | 143 B |
| `13:14:39:43:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 04 | 0: public device address | -44 dBm | 3 | `absent` | (unnamed) | 0 B |
| `1D:0B:39:43:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 04 | 0: public device address | -54 dBm | 3 | `absent` | (unnamed) | 0 B |
| `27:11:33:4C:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 04 | 0: public device address | -44 dBm | 3 | `absent` | (unnamed) | 0 B |
| `1D:0A:30:8E:18:46/P` | Unnamed public-address-class report; zero advertising payload in this capture; unresolved | 05 | 0: public device address | -61 dBm | 3 | `absent` | (unnamed) | 0 B |

## Per-address raw advertiser records

### `1D:1B:3C:93:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 01 — 01 — case open; both earbuds still in case; not pairing

```text
[ADV #001] addr=1D:1B:3C:93:18:46/P type=0 (public device address) RSSI=-38dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `5B:C5:06:09:94:D8`

Classification: Pairing-only RPA; Microsoft company-ID 0x0006 manufacturer payload; working label: Swift Pair candidate.

Run 02 — 02 — formal pairing mode; both earbuds in case

```text
[ADV #001] addr=5B:C5:06:09:94:D8 type=1 (random device address / resolvable-private(RPA)) RSSI=-39dBm PHY=1/2 SID=1 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=37B 20:FF:06:00:03:02:80:04:44:24:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29:03:19:41:09
  AD structures:
    [00] type=0xFF Manufacturer Specific Data     len=31 value=06:00:03:02:80:04:44:24:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => company=0x0006 (unknown-company) payload=03:02:80:04:44:24:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
  derived hints: random_kind=resolvable-private(RPA) RSI=no LE-Audio-service-hint=none-in-advertising
```

### `E8:C9:13:EF:2F:23/P`

Classification: LE Audio member family A — public/identity-looking advertisement during formal pairing.

Run 02 — 02 — formal pairing mode; both earbuds in case

```text
[ADV #002] addr=E8:C9:13:EF:2F:23/P type=0 (public device address) RSSI=-43dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:19:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:00:09:16:4E:18:00:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:08:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=19  => flags=0x19
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:00  => service_uuid=0x1853 (Common Audio Service) payload=00
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:00:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=00:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:08
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=n/a RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

### `E8:C9:13:F6:5C:E0/P`

Classification: LE Audio member family B — public/identity-looking advertisement during formal pairing.

Run 02 — 02 — formal pairing mode; both earbuds in case

```text
[ADV #003] addr=E8:C9:13:F6:5C:E0/P type=0 (public device address) RSSI=-50dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:00:09:16:4E:18:00:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:09:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:00  => service_uuid=0x1853 (Common Audio Service) payload=00
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:00:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=00:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:09
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=n/a RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

### `1D:12:31:88:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 03 — 03 — left earbud out (execution-order label; filename was reused)

```text
[ADV #001] addr=1D:12:31:88:18:46/P type=0 (public device address) RSSI=-52dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `27:0F:31:88:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 03 — 03 — left earbud out (execution-order label; filename was reused)

```text
[ADV #002] addr=27:0F:31:88:18:46/P type=0 (public device address) RSSI=-51dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `27:10:31:88:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 03 — 03 — left earbud out (execution-order label; filename was reused)

```text
[ADV #003] addr=27:10:31:88:18:46/P type=0 (public device address) RSSI=-51dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `41:95:2A:3E:63:B2`

Classification: LE Audio member family A — RPA advertisement outside formal pairing.

Run 03 — 03 — left earbud out (execution-order label; filename was reused)

```text
[ADV #004] addr=41:95:2A:3E:63:B2 type=1 (random device address / resolvable-private(RPA)) RSSI=-55dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:18:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=18  => flags=0x18
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

Run 04 — 04 — right earbud out (execution-order label; filename was reused)

```text
[ADV #004] addr=41:95:2A:3E:63:B2 type=1 (random device address / resolvable-private(RPA)) RSSI=-43dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:18:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=18  => flags=0x18
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

Run 05 — 05 — both earbuds out

```text
[ADV #002] addr=41:95:2A:3E:63:B2 type=1 (random device address / resolvable-private(RPA)) RSSI=-55dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:18:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=18  => flags=0x18
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

### `42:AD:FE:E2:A5:43`

Classification: LE Audio member family B — RPA advertisement outside formal pairing.

Run 03 — 03 — left earbud out (execution-order label; filename was reused)

```text
[ADV #005] addr=42:AD:FE:E2:A5:43 type=1 (random device address / resolvable-private(RPA)) RSSI=-48dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

Run 04 — 04 — right earbud out (execution-order label; filename was reused)

```text
[ADV #005] addr=42:AD:FE:E2:A5:43 type=1 (random device address / resolvable-private(RPA)) RSSI=-48dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

Run 05 — 05 — both earbuds out

```text
[ADV #003] addr=42:AD:FE:E2:A5:43 type=1 (random device address / resolvable-private(RPA)) RSSI=-65dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853
```

### `13:14:39:43:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 04 — 04 — right earbud out (execution-order label; filename was reused)

```text
[ADV #001] addr=13:14:39:43:18:46/P type=0 (public device address) RSSI=-44dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `1D:0B:39:43:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 04 — 04 — right earbud out (execution-order label; filename was reused)

```text
[ADV #002] addr=1D:0B:39:43:18:46/P type=0 (public device address) RSSI=-54dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `27:11:33:4C:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 04 — 04 — right earbud out (execution-order label; filename was reused)

```text
[ADV #003] addr=27:11:33:4C:18:46/P type=0 (public device address) RSSI=-44dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

### `1D:0A:30:8E:18:46/P`

Classification: Unnamed public-address-class report; zero advertising payload in this capture; unresolved.

Run 05 — 05 — both earbuds out

```text
[ADV #001] addr=1D:0A:30:8E:18:46/P type=0 (public device address) RSSI=-61dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising
```

## Cross-reference to earlier R2.4 ACL/SMP HIL

These observations predate the six-state forensic matrix but are relevant to address selection:

- `52:8E:99:8D:C3:F5`, type 1/random: LE ACL previously reached `ACL: PASS handle=0x0801`, then one-time SMP timed out. No RSI was captured in that run, so it is **not** mapped to family A or B in this document.
- `E8:C9:13:F6:5C:E0/P`, type 0/public: an earlier ACL attempt timed out waiting for LE Enhanced Connection Complete.
- `E8:C9:13:EF:2F:23/P`, type 0/public: later verbose ACL trace showed HCI `0x2043 LE Extended Create Connection` Command Status success in ~1.95 ms, followed by 10 s with no LE Enhanced Connection Complete; best-effort cancel returned status `0x0C`.
- These HIL results are consistent with, but do not by themselves prove, the hypothesis that the public member advertisements and the actual initial pairing transport are distinct surfaces.

## What this capture proves vs. does not prove

### Proven by this capture

- There are two stable LE Audio member families identifiable by RSI.
- During formal pairing those families advertise on two public addresses; outside formal pairing they advertise on two RPAs.
- A separate pairing-only RPA with company ID `0x0006` appears during formal pairing and carries the product display name in manufacturer data.
- Unnamed public-address-class, zero-payload reports are repeatable within this Windows/BE200 capture and were independently reported by the operator on Fedora/AX210.

### Not yet proven

- Which family is physically left vs. right. This should be assigned later from PACS Audio Locations or another direct characteristic, not RSSI.
- Whether the unnamed public-address-class reports belong to Samsung OEM logic, the charging case, controller/firmware behavior, or another advertising/reporting mechanism.
- Whether the pairing-only `0x0006` RPA is the exact ACL/SMP transport endpoint. The new `r2 swift-pair-probe` exists specifically to test this without consuming a bond.
- Whether the two formal-pairing public addresses are resolvable identity addresses, independent advertising identities, or another Samsung-specific surface.

## Next controlled test

```powershell
.\target\debug\be200hub.exe r2 swift-pair-probe `
    --seconds 5 `
    --name "Galaxy Buds3 Pro (2F23)" `
    2>&1 | Tee-Object ".\r2-swift-pair-acl-1.log"
```

This probe is ACL-only: detect the pairing-only `0x0006` beacon, connect, linger, and disconnect. It does not perform SMP, GATT, or bonding.

## Appendix A — complete original terminal capture

The following is preserved from the operator-provided paste. Mojibake such as `ΓÇö` is retained because this appendix is evidence, not a cleaned transcription.

```text
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> .\target\debug\be200hub.exe r2 scan-forensics --seconds 5 `
>>   2>&1 | Tee-Object ".\forensics-00-case-closed.log"
R2 forensic passive advertising capture
Target: Intel BE200 8087:0036; LE 1M; 5s
Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.
Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.
R2 fast USB preflight: 8087:0036 bus=1 addr=9 ports=14 ΓÇö Bluetooth HCI descriptor OK
Attach: PASS
Forensic scan: PASS ΓÇö 0 reports, 0 retained advertiser(s)
NOTE: address/RPA classification below is transport evidence, not proof of physical identity.

LE Extended Scan disabled: OK
R2 forensic capture complete. No advertiser was connected or paired.
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> .\target\debug\be200hub.exe r2 scan-forensics --seconds 5 `
>>   2>&1 | Tee-Object ".\forensics-01-case-open.log"
>> 
R2 forensic passive advertising capture
Target: Intel BE200 8087:0036; LE 1M; 5s
Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.
Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.
R2 fast USB preflight: 8087:0036 bus=1 addr=9 ports=14 ΓÇö Bluetooth HCI descriptor OK
Attach: PASS
Forensic scan: PASS ΓÇö 1 reports, 1 retained advertiser(s)
NOTE: address/RPA classification below is transport evidence, not proof of physical identity.

[ADV #001] addr=1D:1B:3C:93:18:46/P type=0 (public device address) RSSI=-38dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

LE Extended Scan disabled: OK
R2 forensic capture complete. No advertiser was connected or paired.
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> # 2. 两耳在盒内，进入正式 pairing mode
>> .\target\debug\be200hub.exe r2 scan-forensics --seconds 5 `
>>   2>&1 | Tee-Object ".\forensics-02-pairing.log"
R2 forensic passive advertising capture
Target: Intel BE200 8087:0036; LE 1M; 5s
Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.
Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.
R2 fast USB preflight: 8087:0036 bus=1 addr=9 ports=14 ΓÇö Bluetooth HCI descriptor OK
Attach: PASS
Forensic scan: PASS ΓÇö 3 reports, 3 retained advertiser(s)
NOTE: address/RPA classification below is transport evidence, not proof of physical identity.

[ADV #001] addr=5B:C5:06:09:94:D8 type=1 (random device address / resolvable-private(RPA)) RSSI=-39dBm PHY=1/2 SID=1 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=37B 20:FF:06:00:03:02:80:04:44:24:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29:03:19:41:09
  AD structures:
    [00] type=0xFF Manufacturer Specific Data     len=31 value=06:00:03:02:80:04:44:24:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => company=0x0006 (unknown-company) payload=03:02:80:04:44:24:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
  derived hints: random_kind=resolvable-private(RPA) RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #002] addr=E8:C9:13:EF:2F:23/P type=0 (public device address) RSSI=-43dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:19:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:00:09:16:4E:18:00:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:08:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=19  => flags=0x19
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:00  => service_uuid=0x1853 (Common Audio Service) payload=00
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:00:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=00:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:08
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=n/a RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

[ADV #003] addr=E8:C9:13:F6:5C:E0/P type=0 (public device address) RSSI=-50dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:00:09:16:4E:18:00:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:09:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:00  => service_uuid=0x1853 (Common Audio Service) payload=00
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:00:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=00:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:09
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=n/a RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

LE Extended Scan disabled: OK
R2 forensic capture complete. No advertiser was connected or paired.
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> .\target\debug\be200hub.exe r2 scan-forensics --seconds 5 `
>>   2>&1 | Tee-Object ".\forensics-01-case-open.log"         
>>                                                 
R2 forensic passive advertising capture
Target: Intel BE200 8087:0036; LE 1M; 5s
Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.
Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.
R2 fast USB preflight: 8087:0036 bus=1 addr=9 ports=14 ΓÇö Bluetooth HCI descriptor OK
Attach: PASS
Forensic scan: PASS ΓÇö 5 reports, 5 retained advertiser(s)
NOTE: address/RPA classification below is transport evidence, not proof of physical identity.

[ADV #001] addr=1D:12:31:88:18:46/P type=0 (public device address) RSSI=-52dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #002] addr=27:0F:31:88:18:46/P type=0 (public device address) RSSI=-51dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #003] addr=27:10:31:88:18:46/P type=0 (public device address) RSSI=-51dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #004] addr=41:95:2A:3E:63:B2 type=1 (random device address / resolvable-private(RPA)) RSSI=-55dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:18:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=18  => flags=0x18
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

[ADV #005] addr=42:AD:FE:E2:A5:43 type=1 (random device address / resolvable-private(RPA)) RSSI=-48dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

LE Extended Scan disabled: OK
R2 forensic capture complete. No advertiser was connected or paired.
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> .\target\debug\be200hub.exe r2 scan-forensics --seconds 5 `
>>   2>&1 | Tee-Object ".\forensics-01-case-open.log"
>> 
R2 forensic passive advertising capture
Target: Intel BE200 8087:0036; LE 1M; 5s
Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.
Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.
R2 fast USB preflight: 8087:0036 bus=1 addr=9 ports=14 ΓÇö Bluetooth HCI descriptor OK
Attach: PASS
Forensic scan: PASS ΓÇö 5 reports, 5 retained advertiser(s)
NOTE: address/RPA classification below is transport evidence, not proof of physical identity.

[ADV #001] addr=13:14:39:43:18:46/P type=0 (public device address) RSSI=-44dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #002] addr=1D:0B:39:43:18:46/P type=0 (public device address) RSSI=-54dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #003] addr=27:11:33:4C:18:46/P type=0 (public device address) RSSI=-44dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #004] addr=41:95:2A:3E:63:B2 type=1 (random device address / resolvable-private(RPA)) RSSI=-43dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:18:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=18  => flags=0x18
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

[ADV #005] addr=42:AD:FE:E2:A5:43 type=1 (random device address / resolvable-private(RPA)) RSSI=-48dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

LE Extended Scan disabled: OK
R2 forensic capture complete. No advertiser was connected or paired.
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> # 5. 两耳都取出
>> .\target\debug\be200hub.exe r2 scan-forensics --seconds 5 `
>>   2>&1 | Tee-Object ".\forensics-05-both-out.log"
R2 forensic passive advertising capture
Target: Intel BE200 8087:0036; LE 1M; 5s
Policy: passive scan only; no connection, pairing, SCAN_REQ, GATT, CIG/CIS, or ISO.
Purpose: fingerprint TWS members / RPA / identity-looking advertisers / case / vendor beacons.
R2 fast USB preflight: 8087:0036 bus=1 addr=9 ports=14 ΓÇö Bluetooth HCI descriptor OK
Attach: PASS
Forensic scan: PASS ΓÇö 3 reports, 3 retained advertiser(s)
NOTE: address/RPA classification below is transport evidence, not proof of physical identity.

[ADV #001] addr=1D:0A:30:8E:18:46/P type=0 (public device address) RSSI=-61dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=(unnamed)
  RSI=absent raw=0B -
  AD structures: (none)
  derived hints: random_kind=n/a RSI=no LE-Audio-service-hint=none-in-advertising

[ADV #002] addr=41:95:2A:3E:63:B2 type=1 (random device address / resolvable-private(RPA)) RSSI=-55dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=5C:06:D1:6A:01:41 raw=143B 02:01:18:03:19:41:09:05:16:51:2B:2A:00:07:2E:5C:06:D1:6A:01:41:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=18  => flags=0x18
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=5C:06:D1:6A:01:41  => RSI=5C:06:D1:6A:01:41
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0C
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

[ADV #003] addr=42:AD:FE:E2:A5:43 type=1 (random device address / resolvable-private(RPA)) RSSI=-65dBm PHY=1/2 SID=3 legacy=false connectable=true scannable=false name=Galaxy Buds3 Pro (2F23)
  RSI=32:3C:A2:A6:02:79 raw=143B 02:01:04:03:19:41:09:05:16:51:2B:2A:00:07:2E:32:3C:A2:A6:02:79:04:16:53:18:01:09:16:4E:18:01:6F:00:6B:00:00:0B:02:4E:18:4F:18:50:18:44:18:4D:18:2E:FF:75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20:16:21:3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D:18:09:47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29
  AD structures:
    [00] type=0x01 Flags                          len=1 value=04  => flags=0x04
    [01] type=0x19 Appearance                     len=2 value=41:09  => appearance=0x0941
    [02] type=0x16 Service Data - 16-bit UUID     len=4 value=51:2B:2A:00  => service_uuid=0x2B51 payload=2A:00
    [03] type=0x2E Resolvable Set Identifier      len=6 value=32:3C:A2:A6:02:79  => RSI=32:3C:A2:A6:02:79
    [04] type=0x16 Service Data - 16-bit UUID     len=3 value=53:18:01  => service_uuid=0x1853 (Common Audio Service) payload=01
    [05] type=0x16 Service Data - 16-bit UUID     len=8 value=4E:18:01:6F:00:6B:00:00  => service_uuid=0x184E (Audio Stream Control Service) payload=01:6F:00:6B:00:00
    [06] type=0x02 Incomplete 16-bit UUIDs        len=10 value=4E:18:4F:18:50:18:44:18:4D:18  => 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1844(Volume Control Service), 0x184D
    [07] type=0xFF Manufacturer Specific Data     len=45 value=75:00:02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20  => company=0x0075 (Samsung Electronics) payload=02:09:01:00:00:00:15:03:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:03:01:54:01:5B:48:65:61:64:70:68:6F:6E:65:5D:20
    [08] type=0x21 Service Data - 128-bit UUID    len=21 value=3A:04:8F:2B:A9:7E:A6:AE:1B:49:C6:19:E9:73:A4:A7:01:54:23:2F:0D
    [09] type=0x09 Complete Local Name            len=23 value=47:61:6C:61:78:79:20:42:75:64:73:33:20:50:72:6F:20:28:32:46:32:33:29  => name="Galaxy Buds3 Pro (2F23)"
  derived 16-bit UUIDs: 0x1844(Volume Control Service), 0x184D, 0x184E(Audio Stream Control Service), 0x184F(Broadcast Audio Scan Service), 0x1850(Published Audio Capabilities Service), 0x1853(Common Audio Service), 0x2B51
  derived hints: random_kind=resolvable-private(RPA) RSI=yes LE-Audio-service-hint=0x184E,0x1850,0x1853

LE Extended Scan disabled: OK
R2 forensic capture complete. No advertiser was connected or paired.
PS C:\STanJK\Development\LEAudioLab\BE200\BE200AudioHub> 
```
