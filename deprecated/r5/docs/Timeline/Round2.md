# BE200AudioHub — Round 2 Engineering Record

**Project:** BE200AudioHub / LE Audio Relay  
**Round:** Round 2 — Bumble-RS integration, real LE ACL, pairing/bond architecture  
**Branch:** `r2/stack-integration-bootstrap`  
**Current implementation HEAD at the end of this round:** `e883f70315f17a605f266e90e7e5618b5a7ea02d`  
**Primary target:** Intel BE200 (`8087:0036`) + Galaxy Buds3 Pro  
**Host OS:** Windows 11, BE200 bound to WinUSB for user-space HCI ownership

---

# 0. Executive summary

Round 2 moved BE200AudioHub from “we can boot Intel firmware and issue raw HCI commands” into a real user-space Bluetooth Host stack running against physical hardware.

The important progression was:

```text
Intel Bootloader
    ↓
R1.5b Windows OEM firmware loader
    ↓
BE200 Operational
    ↓
Bumble-RS takes ownership without HCI Reset
    ↓
Bumble sees real BLE advertisements
    ↓
Bumble establishes a real LE ACL to Buds3 Pro
    ↓
stable 2 s ACL
    ↓
controlled disconnect
```

The following milestones are now hardware-validated:

```text
R1.5b  Bootloader → Operational firmware                   PASS
R1.6   Raw HCI capabilities + BLE scan                    PASS
R2.1   Bumble attach to already-Operational BE200         PASS
R2.2   Bumble passive Extended Scan → real B3P ads        PASS
R2.3   Same-process scan → real LE ACL → disconnect       PASS
```

The next major block is **R2.4: persistent SMP bonding and encrypted reconnect**, with the explicit design goal:

> Each physical Buds3 Pro member should be truly paired only once.

The desired steady state is:

```text
member A pairing count = 1
member B pairing count = 1

all later development:
scan / RPA resolve
→ reconnect
→ use stored LTK
→ encrypted ACL
→ no new SMP Pairing Request
```

---

# 1. Round 2 starting point

Before Round 2, the project had already proven the low-level Intel path:

- BE200 could be manually powered independently.
- USB interface 0 was controlled via WinUSB/libusb.
- Windows Intel cold boot was captured.
- Intel Secure Send firmware loading had been reconstructed.
- The exact Windows OEM firmware path could be reproduced independently.
- R1.6 had proven that the Operational BE200 exposed modern LE features including CIS/ISO support.
- Raw BLE Extended Advertising scanning could already see Galaxy Buds3 Pro.

The strategic change for Round 2 was:

> Stop building a complete Bluetooth Host from scratch.  
> Use Bumble-RS as the Host substrate, but keep BE200-specific ownership, lifecycle, policy, LE Audio orchestration, scheduler, recovery, and audio bridge in our own code.

---

# 2. Upstream audit and architectural decisions

## 2.1 Bumble-RS selected as the main Host substrate

Pinned upstream:

```text
chaitanyarahalkar/bumble-rs
bbac2a6803b8cab0920ab725a23aa408fc4fed85
```

Bumble-RS was found to already contain substantially more than initially expected:

- USB HCI transport
- HCI command/event parsing
- ACL
- L2CAP
- ATT
- GATT
- SMP
- PairingManager
- persistent key stores
- RPA resolution
- PACS
- ASCS
- BAP-related types
- CIG/CIS
- ISO Host primitives
- LC3 codec support

The missing piece for our product is not “Bluetooth LE Audio support” in general.

The important custom layer remains:

```text
BE200 runtime ownership / handoff
BAP Unicast Client coordinator
Buds-specific topology / QoS policy
audio bridge
ISO pacing / drift / backpressure
recovery supervisor
Windows audio capture
```

---

## 2.2 Other upstream projects and how they are used

### Google Bumble

Use as an independent behavioral oracle:

- Python user-space Bluetooth Host
- Intel controller support
- BE200 recognition
- PACS / ASCS
- CIS / ISO
- confirms HCI ISO over USB Bulk OUT behavior

### Linux `btusb`

Use as a transport behavior oracle.

Important confirmation:

```text
HCI Command  → USB Control
HCI Event    ← USB Interrupt
ACL          ↔ USB Bulk
HCI ISO      ↔ USB Bulk
```

This strongly supports the decision that LE Audio CIS HCI ISO TX uses interface 0 Bulk OUT, not the SCO-style USB isochronous interface.

### Zephyr

Use as the **BAP happy-path oracle**:

```text
codec config
→ group
→ QoS
→ enable
→ start
→ stream TX
```

### AOSP Fluoride

Use as the **failure / recovery oracle**:

- CIG creation
- CIS establishment
- ISO Data Path setup
- teardown
- retry / recovery state machine

### BlueZ / `le-audio-receiver`

Use as a lifecycle / real-world interoperability reference, not as product code.

---

# 3. R2 branch bootstrap and project layout

The Round 2 branch was created as:

```text
r2/stack-integration-bootstrap
```

Important bootstrap commits before the hardware milestones:

```text
ea0c06f9bdc9ae2256acb2ffa35abe823178a149
build(r2): scaffold stack integration boundaries

2f194aaadafc720282e2ae45d89233a1d323fd61
refactor(r2): import relay seams and pin bumble-rs

e65605768032f826a8f4782b318698c8ba609a38
fix(r2): preserve relay source verbatim

2dc70f8403d52071356c3687a203d0f27f66777c
refactor(r2): make Windows audio path Rust-only
```

The resulting workspace boundary is:

```text
crates/
├─ be200-runtime
├─ bumble-adapter
├─ audio-capture-win32
├─ audio-bridge
├─ bap-client
├─ device-policy
├─ iso-scheduler
└─ recovery-supervisor
```

The old C# LE-Audio-Relay code is reference material only:

```text
reference/le-audio-relay/
```

Final product direction:

> Rust-only runtime.  
> No mixed Rust + .NET production pipeline.

For First Sound, Windows audio capture is intentionally deferred:

```text
fixed WAV / sine
→ LC3
→ ISO
→ BE200
→ Buds
```

Only after this works do we connect WASAPI.

---

# 4. R2.1 — Bumble attach without HCI Reset

## 4.1 The critical upstream quirk

Bumble-RS normal external-controller initialization is:

```rust
ExternalHost::initialize_device()
```

The first action inside that function is:

```text
HCI Reset
```

That is unacceptable for our first handoff experiment.

R1.5b has already brought BE200 into Intel **Operational** firmware. The initial R2 requirement was:

> Attach a full Host to the already-running controller without disturbing controller firmware/lifecycle state.

Therefore we must not call normal `initialize_device()` unchanged.

---

## 4.2 Chosen solution

We deliberately did **not fork the entire Bumble Host**.

Instead, `bumble-adapter` mirrors the public command sequence of:

```text
ExternalHost::initialize_device()
```

but omits exactly:

```text
Command::Reset
```

Everything after Reset remains aligned with upstream:

```text
Read Local Supported Commands
Read Local Version
Read LE Features
Read LMP Extended Features
Set Event Mask
Set Event Mask Page 2
LE Set Event Mask
Read classic ACL buffer
Read LE ACL / ISO buffer
Read / update Suggested Default Data Length
Read advertising capacity
configure_controller_packet_pools()
```

This became:

```text
initialize_existing_operational()
```

Commit:

```text
c8ed60e1abd8f36a737c94c9acc3a0d48febdc5e
feat(r2.1): attach Bumble host without resetting BE200
```

---

## 4.3 Cold-power quirk discovered during testing

The user had physically removed power from BE200.

Running:

```powershell
be200hub hci version
```

returned:

```text
Image: Bootloader (0x01)
```

This is expected.

Important lifecycle rule:

```text
BE200 physical power loss
→ Intel Bootloader
→ user-space firmware loader required again
```

Because BE200 is bound to WinUSB, Windows is not automatically performing the Intel Bluetooth firmware boot for this experimental device.

Current development sequence after every real power loss is therefore:

```text
firmware boot
→ hci version / Operational confirmation
→ R2 attach / scan / connect
```

Long-term product behavior should automate this:

```text
open controller
→ check image
→ Bootloader ? load firmware : attach immediately
```

But this remains deliberately separated during R2 to avoid debugging loader and Host attach simultaneously.

---

## 4.4 Verified firmware used

Use the recovered Windows OEM firmware:

```text
be200_intel_windows_24.60.0.4_usb_verified.sfi
size: 980,840 bytes
```

Not the other SFI:

```text
ibt-0291-0291.sfi
size: 978,976 bytes
```

Known DDC:

```text
ibt-0291-0291.ddc
size: 4 bytes
```

The validated firmware boot sequence again produced:

```text
Windows preboot DDC 0x0186 accepted
Secure Send 3960 / 3960
Intel 0xFF/0x06 firmware result accepted
Boot address 0x00100800
Intel 0xFF/0x02 Operational event
Intel DSBR defaults 0xFC0A
Operational confirmed by Intel 0xFC05
Intel 0xFC8B DDC record applied
```

---

## 4.5 R2.1 HIL result

Command:

```powershell
.\target\debug\be200hub.exe r2 attach-probe
```

Result:

```text
Attach: PASS
Bumble upstream:
bbac2a6803b8cab0920ab725a23aa408fc4fed85

Host state: Running

Controller version:
HCI          = 0x0D
subversion   = 0x44B0
manufacturer = 2

LE ACL buffer:
251 B × 6

ISO buffer:
248 B × 8

Max extended advertising data:
200 B
```

This matched the independently measured R1.6 controller capabilities.

Therefore R2.1 proved:

```text
R1 loader releases USB interface 0
→ Bumble reclaims the same live controller
→ no firmware reload
→ no HCI Reset
→ correct Host capability state
→ correct ACL / ISO flow-control pools
```

---

# 5. R2.2 — Bumble passive Extended Scan

## 5.1 Goal

Prove that after R2.1 attach, actual asynchronous radio events can flow through:

```text
BE200
→ Bumble USB transport
→ ExternalHost
→ Device event path
→ Bumble advertisement journal
```

The acceptance target was the real:

```text
Galaxy Buds3 Pro (2F23)
```

---

## 5.2 Scan design

The R2.2 path intentionally kept the R1.6 scan policy:

```text
own address type     = public
filter policy        = accept all
PHY                  = LE 1M
scan type            = passive
scan interval        = 0x0060
scan window          = 0x0030
```

The scan uses Extended Advertising.

Important decision:

> Use Bumble’s merged `take_advertisements()` journal rather than manually combining raw legacy and extended report fragments.

This avoids duplicating Bumble’s advertisement assembly logic.

---

## 5.3 R2.2 commits

```text
ddb46d518eaa5ca34a2b8c49011851ac3ed48e66
feat(r2.2): scan through Bumble host event path

9f20aedbb17b47217628159f037ce569b820d37d
fix(r2.2): render qualified Bumble addresses
```

The second commit fixed address rendering.

Bumble address strings can carry a type qualifier for public addresses:

```text
AA:BB:CC:DD:EE:FF/P
```

Random addresses do not get the `/P` qualifier.

Later, for R2.3, string rendering was judged insufficient and `address_type` was preserved explicitly in `ScanObservation`.

---

## 5.4 R2.2 HIL result

Command:

```powershell
.\target\debug\be200hub.exe r2 scan --seconds 10 --show-addresses
```

Result:

```text
Scan: PASS
8 Bumble advertisements
8 unique advertisers
```

Two named Buds3 Pro advertisers appeared:

```text
Galaxy Buds3 Pro (2F23)
addr=4F:3C:58:3E:C4:AF
RSSI=-59 dBm
PHY=1/2
SID=3
legacy=false
connectable=true
scannable=false
data=135 B

Galaxy Buds3 Pro (2F23)
addr=7D:70:34:6B:E4:22
RSSI=-59 dBm
PHY=1/2
SID=3
legacy=false
connectable=true
scannable=false
data=135 B
```

Several additional unnamed connectable advertisements were visible with zero assembled payload.

---

## 5.5 Important interpretation rule

Do **not** call the two B3P addresses “left” and “right” yet.

At this point they are only:

```text
member A
member B
```

The addresses are random (`address_type = 1`) and change between runs.

Later R2.3 observed completely different B3P addresses:

```text
50:C1:13:44:E0:45
58:D6:96:86:1F:DB
```

This confirms that raw advertisement address strings are not stable identities.

Final left/right assignment should come later from:

```text
PACS Audio Locations
```

CSIS rank is not equivalent to audio channel side.

---

# 6. R2 hot-path latency / blue PowerShell progress bug

## 6.1 Symptom

Every R2 command initially appeared to “hang” for roughly 10–15 seconds before doing useful work.

PowerShell showed flashing blue progress bars.

This happened even before the requested scan duration.

---

## 6.2 Root cause

R2 commands still called the old R1 safety guard:

```rust
driver::verify()?;
```

`driver::verify()` launched:

```text
powershell.exe
```

and ran:

```text
Get-PnpDevice -PresentOnly
→ enumerate machine-wide PnP devices
→ find USB\VID_8087&PID_0036
→ query many DEVPKEY properties
→ confirm WinUSB binding
```

The PowerShell PnP/CIM path emitted progress records through the console host, causing the blue progress UI.

The delay was therefore not:

- Bumble initialization
- Bluetooth scan
- USB latency
- HCI processing

It was a heavyweight Windows PnP preflight inherited from R1.

---

## 6.3 Fix

Keep the heavyweight commands available explicitly:

```text
driver status
driver verify
driver backup
```

but remove them from every R2 hot path.

New R2 fast preflight:

```text
pure Rust / rusb
→ enumerate USB
→ exactly one 8087:0036
→ verify Bluetooth HCI class e0/01/01
→ do not open handle
→ do not claim
→ do not send HCI
```

Then Bumble itself proves whether WinUSB can actually open/claim the interface.

Commit:

```text
bb8331f9e4358ef656299ae73a54bf8b5e4b0625
perf(r2): replace PowerShell preflight with Rust USB check
```

User retested and confirmed the long pre-delay / blue progress behavior was gone.

---

# 7. R2.3 — real LE ACL connect / disconnect

## 7.1 Goal

Cross the boundary from:

```text
advertising-only world
```

into:

```text
connected LE world
```

without adding any higher-level protocol.

R2.3 explicitly excludes:

```text
SMP
pairing
GATT
PACS
ASCS
CIG
CIS
ISO
```

---

## 7.2 Same-process target selection decision

A critical design rule was established:

> Scan and connect must happen in the same process / same Host lifetime.

Do not:

```text
process A scans
→ user copies RPA
→ process exits
→ process B connects later
```

Because the peer is advertising with RPAs.

Instead:

```text
same process
→ scan
→ preserve address + address type
→ select candidate
→ immediately connect
```

If multiple B3P advertisers are seen, the CLI asks the user to select a temporary candidate index.

---

## 7.3 Bumble convenience API quirk

Bumble’s high-level:

```text
Device::connect_le_extended()
```

hardcodes:

```text
own_address_type = random
```

This is unsafe for our no-reset path.

Our current R2 controller path had not run Bumble `power_on()`, therefore it had not necessarily programmed a Bumble random address.

R2.1 / R2.2 had consistently used the controller public address.

Decision:

> Do not use the convenience function for the first physical ACL experiment.

R2.3 directly sends:

```text
LE Extended Create Connection
```

through `ExternalHost` with:

```text
own_address_type = public
```

while still letting Bumble `Device::poll()` consume:

```text
LE Enhanced Connection Complete
```

and maintain connection state.

---

## 7.4 R2.3 implementation

Commit:

```text
e883f70315f17a605f266e90e7e5618b5a7ea02d
feat(r2.3): probe real LE ACL connect and disconnect
```

CLI:

```powershell
.\target\debug\be200hub.exe r2 connect-probe `
  --seconds 5 `
  --name "Galaxy Buds3 Pro (2F23)"
```

Flow:

```text
attach
→ passive scan
→ filter exact name + connectable
→ display candidates
→ user selects index
→ LE Extended Create Connection
→ wait Enhanced Connection Complete
→ obtain ACL handle
→ pump events for 2 seconds
→ HCI Disconnect
→ wait Disconnection Complete
```

---

## 7.5 R2.3 HIL result

Candidates:

```text
[0]
50:C1:13:44:E0:45
type=1
RSSI=-63 dBm

[1]
58:D6:96:86:1F:DB
type=1
RSSI=-49 dBm
```

User selected:

```text
[1]
```

Connection result:

```text
ACL: PASS

handle = 0x0800
role   = central
peer   = 58:D6:96:86:1F:DB

connection interval = 36
                    = 45 ms

peripheral latency = 0

supervision timeout = 42
                    = 420 ms
```

The ACL remained established while the Host pumped HCI for:

```text
2 seconds
```

Then a controlled disconnect was submitted.

Requested HCI Disconnect reason:

```text
0x13
Remote User Terminated Connection
```

Observed local Disconnection Complete reason:

```text
0x16
Connection Terminated by Local Host
```

This is acceptable and consistent with a host-initiated local teardown.

Final result:

```text
same-process scan
→ real LE ACL
→ stable hold
→ controlled disconnect
PASS
```

---

# 8. Meaning of the first real ACL handle

The first validated real handle was:

```text
0x0800
```

This is the anchor for everything above HCI:

```text
LE ACL handle
│
├─ L2CAP
│   ├─ SMP
│   └─ ATT
│       └─ GATT
│           ├─ CSIS
│           ├─ PACS
│           └─ ASCS
│
└─ later:
    CIS association / LE Audio control
```

R2.3 is therefore the point where the project stopped being a scanner and became a real connected Bluetooth Host.

---

# 9. R2.4 design goal — minimize pairing count

The user explicitly does not want development to devolve into:

```text
delete Buds
re-enter pairing mode
pair again
repeat forever
```

The target invariant is:

```text
member A: one true SMP bond
member B: one true SMP bond

normal later development:
zero new pairings
```

Ideally the user places Buds3 Pro into pairing mode only once while both physical members are onboarded sequentially.

---

# 10. Why both members need their own bond

Do not confuse:

```text
LTK / IRK
```

with:

```text
SIRK
```

Per-member security state:

```text
member A
├─ identity address A
├─ IRK A
└─ LTK A

member B
├─ identity address B
├─ IRK B
└─ LTK B
```

Set membership:

```text
SIRK X
```

Both devices can belong to the same Coordinated Set:

```text
A ─┐
   ├─ SIRK X
B ─┘
```

but SIRK is not a replacement for either member’s LTK.

Therefore the safe assumption is:

> Two physical LE members require two SMP bonding transactions.

The optimization target is not “one SMP transaction total”.

It is:

> One pairing transaction per physical member, ever.

---

# 11. Planned dual-member onboarding transaction

Ideal first onboarding:

```text
Buds enters pairing/discoverable mode once
        ↓
scan
        ↓
discover member A + member B
        ↓
connect member A
        ↓
SMP Secure Connections + bonding
        ↓
persist A immediately
        ↓
encrypted ACL
        ↓
minimal GATT
        ↓
read CSIS:
SIRK
set size
rank
        ↓
disconnect A
        ↓
scan again in same onboarding flow
        ↓
identify member B
        ↓
connect B
        ↓
SMP Secure Connections + bonding
        ↓
persist B immediately
        ↓
read CSIS
        ↓
verify:
same SIRK
expected set size
different rank
        ↓
disconnect B
        ↓
BondSet = COMPLETE
```

---

# 12. Partial-success rule

If member A succeeds and member B fails:

```text
A = BONDED
B = MISSING / INCOMPLETE
```

Next run must:

```text
reuse A bond
→ do not pair A again
→ continue B onboarding
```

Never restart the entire set onboarding just because the second member failed.

This is central to minimizing pair count.

---

# 13. Bond deletion policy

Strong invariant:

```text
PAIRING FAILURE != DELETE BOND
```

The program must not automatically remove stored keys because of:

- connect failure
- encryption failure
- RPA resolution failure
- GATT failure
- PACS failure
- ASCS failure
- CIS failure
- application crash

Bond deletion must be an explicit user operation.

Planned surfaces:

```text
bond list
bond forget <member>
bond forget-set <set>
```

`forget-set` should require explicit confirmation.

---

# 14. Host identity decision

The current V1 design should use:

```text
BE200 public BD_ADDR
```

as the stable Host identity.

Startup invariant:

```text
Read BD_ADDR
→ compare to bond database controller_identity
```

If equal:

```text
load and reuse bonds
```

If different:

```text
refuse to silently reuse those bonds
```

Do not silently create a new logical Host identity, otherwise the Buds may accumulate stale “ghost host” bonds.

Current Host privacy is intentionally deferred.

V1 first goal:

```text
Host:
stable public identity

Peer:
RPA + IRK resolution
```

This keeps privacy complexity off the critical path while still allowing B3P RPA rotation.

---

# 15. Important R2.4 source-audit findings

The following findings were confirmed directly from the uploaded Bumble-RS source.

---

## 15.1 `PairingConfig::default()` has a dangerous default for our use case

Bumble-RS defaults:

```text
secure_connections = true
bonding            = true
mitm               = true
```

`PairingCapabilities::default()` is:

```text
NoInputNoOutput
```

For Buds3 Pro, blindly using default pairing config would therefore request MITM while claiming no useful I/O.

Decision:

Explicitly configure:

```text
secure_connections = true
bonding            = true
mitm               = false
io                 = NoInputNoOutput
identity           = public Host identity
```

Do not rely on the default `mitm=true`.

---

## 15.2 Default key distribution is exactly what we want

Bumble-RS default SMP key distribution:

```text
ENCRYPTION_KEY
|
IDENTITY_KEY
```

Therefore the initial negotiation is already designed to obtain:

```text
LTK
IRK
peer identity address
```

when the peer supports/distributes them.

This is the desired basis for persistent reconnect and RPA resolution.

---

## 15.3 Secure Connections bond representation

For LE Secure Connections, Bumble stores the common pairing LTK in:

```text
PairingKeys.ltk
```

Legacy role-specific fields also exist:

```text
ltk_central
ltk_peripheral
```

but for our desired SC path the normal field is:

```text
ltk
```

---

## 15.4 `store_bond()` uses peer identity when available

Bumble key distribution keeps:

```text
peer_identity_address
```

If the peer distributes Identity Address Information:

```text
store key name = peer identity address
```

If no identity address was obtained:

```text
fallback = current connection peer address
```

This is an important quirk.

If the fallback address is an RPA, it is not a desirable permanent bond key.

Planned policy:

> A B3P bond is not considered fully stable unless identity information / IRK is available.

Do not quietly claim “persistent identity complete” if we only stored a transient RPA.

---

## 15.5 JsonKeyStore is already atomic enough for the experiment

Bumble-RS `JsonKeyStore` save path:

```text
serialize DB
→ write <file>.tmp
→ rename temp to final
```

This gives us a useful crash boundary.

Decision:

Persist each member immediately after successful pairing.

Do not wait until both members are complete before saving anything.

---

## 15.6 Key store merging behavior

`JsonKeyStore::update()` merges newer pairing material into an existing peer entry instead of blindly replacing the entire record.

This is useful for:

- additional keys learned later
- Classic/LE/CTKD expansion
- incremental metadata

---

## 15.7 Key store namespaces

Bumble key store supports namespaces.

Device-side namespace selection prefers:

```text
public_address if known and non-zero
else random_address
```

This reinforces the need to plumb the real BE200 public BD_ADDR into R2.4 before trusting automatic keystore configuration.

---

# 16. Critical no-reset identity quirk discovered before R2.4 implementation

Normal Bumble `Device::power_on()` performs Host identity initialization.

Our R2 path intentionally bypasses `power_on()`.

Therefore several automatic Device assumptions must not be taken for granted.

Most importantly:

```text
Device public_address
Device random_address
```

may not reflect the actual local address state in the same way as normal Bumble startup.

Before SMP, R2.4 must explicitly obtain:

```text
HCI Read BD_ADDR
```

and use the real BE200 public address as the pairing local identity.

Do not pass an arbitrary Bumble random/static config address into SMP identity calculations.

---

# 17. Device-integrated pairing manager quirk

Bumble `Device` contains an internal `pairing_manager`.

When an LE connection is registered with that manager, the current upstream code uses:

```text
self.random_address
```

as the local pairing address.

That is not automatically safe for our no-reset / public-own-address R2 path.

Therefore the apparently convenient approach:

```text
turn on Device pairing manager and let it do everything
```

may bind SMP identity semantics to the wrong local address unless we first fully repair Device identity initialization.

Current preferred R2.4 direction:

> Use `LePairingSession` explicitly with the real BE200 public address for the first implementation.

This keeps local identity under our control.

---

# 18. Device KeyStore integration — useful capabilities found

Even though the first pairing session may remain external, Bumble `Device` already has a strong persistent-key integration layer.

Confirmed APIs include:

```text
set_key_store(...)
bonds()
bond(peer)
delete_bond(peer)
delete_all_bonds()

refresh_address_resolver()
refresh_resolving_list()

enable_encryption_with_bond()
enable_encryption_with_bond_on_handle()
```

This means we do not need to invent key database semantics from scratch.

---

# 19. Stored-bond encryption behavior

Bumble can reconstruct the stored encryption parameters:

```text
LTK
EDIV
RAND
```

from the key store and call:

```text
LE Enable Encryption
```

For Secure Connections:

```text
EDIV = 0
RAND = 0
```

as expected.

The useful high-level API is:

```text
enable_encryption_with_bond_on_handle()
```

for a central connection.

---

# 20. Automatic LTK request handling

Inside `Device::poll()`, when the controller emits:

```text
LE Long Term Key Request
```

Bumble checks:

```text
1. current pairing-manager key
2. stored KeyStore key
```

If a key is found, it can automatically send:

```text
LE Long Term Key Request Reply
```

If no key exists, the request is surfaced to the application.

This confirms that the Host substrate already supports persistent-key reuse.

---

# 21. RPA resolver support

`KeyStore::get_resolving_keys()` extracts:

```text
(IRK, identity address)
```

for every stored bond that has an IRK.

Bumble provides:

```text
AddressResolver
```

and Device APIs:

```text
refresh_address_resolver()
find_peer_by_identity_address()
```

This is exactly the required basis for:

```text
stored identity
→ observe current RPA
→ resolve current advertiser
→ reconnect without pairing
```

---

# 22. Important RPA reconnect quirk still to resolve

There is a subtle upstream interaction we must handle carefully.

`stored_encryption_parameters()` looks up the bond using:

```text
connection.peer_address.to_string(false)
```

But a new physical connection may have been established to a current RPA.

If the connection metadata still contains the raw RPA while the key store is keyed by stable identity address:

```text
lookup by RPA
→ bond not found
```

Host-side address resolution by itself does not automatically rewrite the connection metadata in the code path inspected.

There are two viable approaches:

### Option A — controller resolving-list offload

Program the BE200 resolving list with:

```text
peer IRK
peer identity
local IRK
```

and let the controller report identity-resolved connection semantics.

### Option B — adapter-level manual resolution

For the first R2.4 implementation:

```text
current RPA
→ host AddressResolver
→ stable identity
→ fetch PairingKeys by identity
→ explicitly use stored LTK
```

This avoids depending on controller privacy offload during the first bond experiment.

Current preference:

> Keep the first implementation explicit and adapter-controlled; add controller offload after persistence is proven.

---

# 23. CSIS findings

Bumble-RS already includes:

```text
CoordinatedSetIdentificationProxy
```

It can discover:

```text
CSIS service
SIRK characteristic
Coordinated Set Size characteristic
Set Member Lock
Set Member Rank
```

It includes a helper:

```text
read_set_identity_resolving_key()
```

SIRK can be:

```text
plain
or
encrypted
```

If encrypted, the helper requires a 16-byte encryption key and performs the CSIS SIRK decryption operation.

For set size and rank, the exposed characteristic proxies can be read through `GattClient::read_value()`.

Therefore no custom CSIS wire parser is required.

---

# 24. GATT transport findings

Bumble-RS already provides:

```text
ExternalAttTransport
```

over a real `ExternalHost` + `Device` + LE ACL handle.

It:

```text
sends ATT over the selected ACL
pumps Device
waits for response
preserves unsolicited ATT PDUs
fails if the ACL disappears
```

This is sufficient for the first CSIS/PACS discovery path.

EATT support also exists, but R2 does not need EATT for the first CSIS proof.

---

# 25. Proposed persistent file layout

Experiment-stage persistence should live under:

```text
%LOCALAPPDATA%\BE200AudioHub\bonding\
```

Suggested split:

```text
pairing-keys.json
buds-sets.json
```

`pairing-keys.json`:

```text
cryptographic peer bond material
LTK
IRK
address type
etc.
```

`buds-sets.json`:

```text
our product-level grouping metadata:
controller_identity
SIRK
set size
member identities
rank
later audio location
onboarding state
```

Do not overload the upstream key store with all product semantics.

---

# 26. Proposed Round 2.4 state machine

The earlier R2.4 plan was refined into:

```text
R2.4a
single-member Secure Connections pairing
+ immediate persistent bond

R2.4b
process restart / reconnect
+ stored LTK encryption
+ absolutely no new pairing

R2.4c
minimal encrypted GATT
+ CSIS discovery
+ SIRK / size / rank

R2.4d
second-member onboarding
+ pair exactly once
+ immediate save
+ verify same SIRK

R2.4e
dual-member bonded reconnect
+ two encrypted ACLs
+ zero new pairings
```

Only after this is stable:

```text
R2.5
PACS / Audio Locations
```

Then later:

```text
ASCS
CIG
CIS
ISO
First Sound
```

---

# 27. Why we should verify reconnect before pairing the second member

The user wants to minimize pairing count.

Therefore we should not immediately pair both members before proving persistence works.

Safer sequence:

```text
pair A once
→ save
→ disconnect
→ process exits
→ new process
→ resolve / reconnect A
→ encrypt from stored bond
→ prove no Pairing Request occurred
```

Only after that passes:

```text
pair B once
```

If persistence is broken, this catches it after only one bond instead of corrupting two members and requiring two repairs.

---

# 28. ToolHub / Connector dogfood findings during this round

A separate local ToolHub connector was used as a bounded build executor.

Exposed MCP-level operations:

```text
list_projects
list_operations
run(project, operation)
```

Configured AudioHub operations included:

```text
git-status
git-pull
cargo-check
cargo-build
frontend-npm-build
```

This proved that ChatGPT could:

```text
Connector
→ Secure MCP Tunnel
→ local ToolHub
→ project-scoped allowlist
→ real local git / cargo operation
```

without exposing a general shell.

---

# 29. ToolHub stale operation lock bug

## Symptom

After:

```text
frontend-npm-build
```

failed with:

```text
spawn EINVAL
```

all later BE200AudioHub ToolHub operations returned:

```text
Project BE200AudioHub already has a running operation
```

## Root cause

The per-project in-flight operation lock was not released on a spawn failure path.

Conceptual broken shape:

```text
acquire project lock
→ spawn()
→ spawn returns EINVAL
→ early error return
→ lock state remains "busy"
```

## Fix decision

Use RAII/finally semantics:

```text
ProjectOperationGuard::acquire()
→ any exit path drops guard
```

Must release on:

```text
normal exit
spawn error
timeout
child crash
other execution failure
```

User fixed this and retested successfully.

The underlying `frontend-npm-build` `spawn EINVAL` is a separate Windows process-launch issue and should not be conflated with the stale-lock bug.

---

# 30. Connector approval-layer observation

An intentionally invalid operation:

```text
definitely-not-allowlisted
```

triggered a user approval UI.

The user manually denied it.

This demonstrated that the local ToolHub allowlist and the platform approval layer are separate protection boundaries.

For the AudioHub workflow, however, a stronger operational rule was adopted:

> The assistant does not directly invoke hardware HIL operations through Connector.

---

# 31. Final Assistant / HIL responsibility boundary

User decision:

```text
Assistant:
- code changes
- git sync
- cargo check
- cargo build
- static/source analysis

User:
- firmware boot
- attach-probe hardware execution
- BLE scan HIL
- LE connect HIL
- pairing
- all future physical-radio experiments
```

This keeps hardware ownership and irreversible Bluetooth state under the user’s direct control.

---

# 32. Build-status reporting rule

The user requested that Connector operations not run “blind”.

New workflow rule:

Before every local build/check:

```text
准备执行：cargo check --workspace
```

After:

```text
cargo check: PASS / FAIL
duration
error count / relevant compiler output
```

Likewise for:

```text
cargo build
git pull
git status
```

This is now part of the working protocol for this project.

---

# 33. Source-access workflow mistake and correction

During R2.2/R2.4 investigation, upstream Bumble-RS source was initially queried repeatedly through the GitHub connector.

This was unnecessary because the project already contained:

```text
shared_resources_Round2.zip
```

with the complete source archives.

Problems with repeated connector archaeology:

- slower
- noisy
- more tool calls
- caused platform “thinking” interruptions
- duplicated already-available local source

Corrected source priority:

```text
1. user/project uploaded original source
2. current repository source
3. GitHub connector only for:
   - our own repo writes
   - branch/HEAD checks
   - genuinely missing upstream/current data
```

R2.4 source auditing was then moved to the local extracted Bumble-RS source.

---

# 34. GitHub write-response quirk

During the R2.2 write, the GitHub tool returned a transient GraphQL error.

However, checking branch HEAD showed that the commit had actually succeeded.

Important workflow rule:

> After a remote write tool returns an ambiguous/transient error, check branch HEAD before retrying.

Otherwise a retry can duplicate changes.

Observed sequence:

```text
tool reports GraphQL failure
→ branch HEAD had already advanced
→ verify commit contents
→ do not repeat write
```

---

# 35. Current local Git state quirk

Throughout the round, local status repeatedly showed:

```text
M Cargo.lock
```

This originated from the Bumble-RS git dependency resolution / workspace bootstrap.

It is not a R2.3 HIL side effect.

Do not confuse it with runtime-generated files.

---

# 36. R2 milestone commits

Relevant branch progression:

```text
ea0c06f9bdc9ae2256acb2ffa35abe823178a149
build(r2): scaffold stack integration boundaries

2f194aaadafc720282e2ae45d89233a1d323fd61
refactor(r2): import relay seams and pin bumble-rs

e65605768032f826a8f4782b318698c8ba609a38
fix(r2): preserve relay source verbatim

2dc70f8403d52071356c3687a203d0f27f66777c
refactor(r2): make Windows audio path Rust-only

c8ed60e1abd8f36a737c94c9acc3a0d48febdc5e
feat(r2.1): attach Bumble host without resetting BE200

ddb46d518eaa5ca34a2b8c49011851ac3ed48e66
feat(r2.2): scan through Bumble host event path

9f20aedbb17b47217628159f037ce569b820d37d
fix(r2.2): render qualified Bumble addresses

bb8331f9e4358ef656299ae73a54bf8b5e4b0625
perf(r2): replace PowerShell preflight with Rust USB check

e883f70315f17a605f266e90e7e5618b5a7ea02d
feat(r2.3): probe real LE ACL connect and disconnect
```

---

# 37. Current validated end-to-end control path

As of the end of this round:

```text
BE200 power-on
    ↓
Bootloader
    ↓
R1.5b Intel Secure Send
    ↓
Operational firmware
    ↓
release interface 0
    ↓
R2 fast Rust USB preflight
    ↓
Bumble USB transport claim
    ↓
no-reset capability initialization
    ↓
passive Extended Scan
    ↓
B3P advertisement
    ↓
select current RPA
    ↓
LE Extended Create Connection
    ↓
LE Enhanced Connection Complete
    ↓
ACL handle
    ↓
stable connected state
    ↓
controlled HCI Disconnect
```

Everything below SMP has now been proven on real hardware.

---

# 38. What is NOT yet proven

Do not accidentally overclaim current state.

Still unvalidated on the new Bumble path:

```text
SMP pairing with real B3P
Secure Connections result
persistent LTK / IRK save
process-restart encrypted reconnect
RPA → identity bond lookup
CSIS read from real B3P
second-member bonding
dual-member simultaneous encrypted ACL
PACS
Audio Locations
ASCS
CIG
CIS
ISO TX
LC3 playback
First Sound
```

---

# 39. Immediate next implementation tasks

Before the first real pairing attempt:

### 39.1 Local Host identity

Add:

```text
HCI Read BD_ADDR
```

and establish:

```text
controller_identity = BE200 public BD_ADDR
```

Use it consistently for:

- SMP local address
- key store namespace
- bond DB ownership check

### 39.2 BondStore wrapper

Create a thin product wrapper around Bumble `JsonKeyStore`.

Requirements:

```text
explicit Windows path
stable namespace
no auto-delete
list / lookup
IRK resolver extraction
atomic updates
```

### 39.3 Pairing policy

Explicit:

```text
Secure Connections = true
Bonding            = true
MITM               = false
I/O                = NoInputNoOutput
Identity           = Public
```

### 39.4 First member pair

Reuse R2.3 same-process scan / target selection.

Then:

```text
ACL
→ LePairingSession
→ encrypted
→ pairing complete
→ inspect PairingKeys
→ require stable identity/IRK
→ atomic store
```

### 39.5 Reconnect proof

Before pairing the second member:

```text
new process
→ load store
→ build resolver
→ identify current RPA
→ connect
→ use stored LTK
→ encrypted
→ confirm zero pairing
```

### 39.6 Minimal CSIS

On encrypted ACL:

```text
ExternalAttTransport
→ GattClient
→ CoordinatedSetIdentificationProxy::discover()
→ SIRK
→ Set Size
→ Rank
```

Then member B can be onboarded with a verified set relationship.

---

# 40. Round 2 north-star invariants

These decisions should survive future refactors.

```text
1. Never HCI Reset an already Operational BE200 without a dedicated experiment.

2. Never silently reload firmware during normal R2 Host attach.

3. Keep Intel firmware loading and Host protocol logic separable.

4. Scan + RPA target selection + connection happen in one Host lifetime.

5. Preserve address type; a rendered address string is not enough.

6. Use stable BE200 public identity for initial Host identity.

7. Do not rely on PairingConfig defaults; explicitly disable MITM for B3P NoInputNoOutput.

8. Pair each physical member at most once unless its bond is genuinely lost.

9. Persist each successful member immediately.

10. Pairing failure never implies bond deletion.

11. Do not call a coordinated-set member Left/Right until Audio Locations proves it.

12. CSIS SIRK groups members; it does not replace per-member LTK / IRK.

13. Stored bond reconnect must be proven before onboarding the second member.

14. HIL remains user-controlled; assistant performs code/build/static work only.

15. Prefer uploaded source archives over repeated external-source connector reads.

16. Keep R2 hot path pure Rust; heavy PowerShell PnP inspection is explicit diagnostics only.
```

---

# 41. Recommended Round 3 handoff starting point

If a new chat starts from this file, resume at:

```text
R2.4 implementation — persistent pairing foundation
```

Do not redo:

```text
R2.1 attach
R2.2 scan
R2.3 ACL connect
```

except as regression probes.

The first code task should be:

```text
Read real BE200 public BD_ADDR
+ persistent BondStore wrapper
+ explicit B3P PairingConfig
```

Then proceed to the first real SMP pairing only after those invariants are in place.

---

# 42. One-line state

At the end of Round 2:

> **BE200AudioHub now owns a real Intel BE200 from Windows user space, hands an already-Operational controller to Bumble-RS without Reset, discovers real Buds3 Pro advertisements, and creates/tears down a stable LE ACL; the next boundary is persistent two-member SMP bonding with a hard target of exactly one pairing per physical Buds member.**
