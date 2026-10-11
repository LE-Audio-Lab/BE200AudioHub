# R2 — Minimal BE200/Buds3 Pro LE connection path

## Objective

Move from validated BE200 passive BLE discovery to a **controlled LE ACL connection** using a normal Windows userspace Rust Host, then add L2CAP/ATT/GATT and pairing. This is the narrowest practical prerequisite for Unicast LE Audio PACS/ASCS + CIG/CIS.

## Freeze now

- Keep Intel signed Windows Build 176 SFI loader unchanged (R1.5b PASS). The controller should already be Operational; do not power-cycle just to test R2.
- WinUSB claims only BE200 interface 0. The separate MT7925 daily Windows Bluetooth controller is never re-bound.
- Preserve a single HCI session and a central event pump. The R1.6 transport is a first seam, not yet a concurrent Host.
- Prefer a replaceable BLE Host adapter interface. Evaluate `trouble-host` for Central, L2CAP, ATT/GATT, SMP/bonding before adding a bespoke stack. BTstack is a useful feature/reference comparison subject to its license.

## R2a — controlled LE ACL: four bounded slices

1. **R2a.1 Session/transport:** make HCI event decoder support asynchronous LE Meta events, Command Status for connection initiation and proper cancellation; add USB Bulk IN `0x82` and Bulk OUT `0x02` ACL framing (including packet boundary flags, handles and fragmentation). Do not confuse these with Bootloader `0xFC09` Bulk OUT.
2. **R2a.2 Scan target selection:** scan only connectable advertisements, present scan-local target index and metadata (address type, SID, RSSI, AD name). Avoid leaking raw addresses by default. Selecting a target must happen in the *same process/session* so rotating RPA addresses are not copied across independent scan runs. Never auto-connect just because the AD name resembles Buds3 Pro.
3. **R2a.3 LE connection:** user explicitly confirms one selected target; issue LE Extended Create Connection (after capability/command support check), handle LE Enhanced Connection Complete including peer address/address type/role, use bounded timeout and LE Create Connection Cancel on failure; otherwise Disconnect cleanly. Test the transport with known peer that permits pairing if Buds3 Pro refuses.
4. **R2a.4 ACL link evidence:** confirm a stable LE handle and successful orderly disconnect, collect failure reasons (status/authentication/policy). Avoid storing raw advertising addresses or bond keys in plain-text logs.

## R2b — host services

- L2CAP LE fixed channels, ATT MTU exchange and GATT service discovery.
- SMP secure pairing, encrypted LE ACL and durable bonding, with privacy address resolution.
- Only after authentication can we verify PACS/ASCS/CAS and whether Samsung Buds3 Pro exposes compatible Unicast Server services. Coordinated set or multi-ASE topology is **unknown**, not frozen.

## R3/R4 — audio

- Unicast Client: discover PACS/ASCS, configure Sink ASE(s), create CIG/CIS.
- Validate how this specific BE200 USB controller presents HCI ISO over Bulk IN/OUT and buffer management. Interface 1 SCO isochronous descriptors do not directly establish the CIS data path.
- Encode negotiated LC3 frames at controller/earbud-supported frame duration and size; send paced ISO SDUs before reusing Windows Process Loopback/Ring audio pipeline.
- Objective remains one-way PC → Buds3 Pro playback; independent mic input; minimize latency and CIS lifecycle faults from the earlier Windows Relay experiments.

## Safety and next acceptance criteria

**First R2 milestone:** controller reports a real LE connection handle followed by a controlled disconnect, while USB/HCI remains healthy and MT7925 untouched. It is **not** necessary to implement audio, SMP, GATT, or produce actual Buds3 Pro audio during this stage.

No automated firmware reload, random-address brute forcing, indefinite reconnection, repeated power-cycling or arbitrary HCI vendor command injection.
