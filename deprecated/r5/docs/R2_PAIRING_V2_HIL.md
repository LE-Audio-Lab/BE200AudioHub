# R2 pairing-v2 HIL

Status: transactional primary + coordinated-set member path compiles; first end-to-end HIL pending.

## Active path

```text
pairing-v2-onboard
  -> primary Microsoft/Swift Pair discovery
  -> generic Filter Accept List direct connection
  -> LeBondManager provisional SMP transaction
  -> encrypted CSIS read using provisional LTK
  -> primary commit establishes expected SIRK / Set Size
  -> RSI search for uncommitted Set Member
  -> generic Filter Accept List direct connection
  -> LeBondManager provisional SMP transaction
  -> SetMemberVerifier reads/decrypts CSIS with provisional LTK
  -> exact SIRK + Set Size gate
     -> reject: disconnect and discard provisional keys
     -> verify: commit peer keys and verified member metadata
  -> refresh address resolver
  -> repeat until Set Size is reached
```

## Persistence rule

Neither primary nor later-member peer keys are written by `LeBondManager`.

The transaction sequence is:

```text
SMP complete + encrypted ACL
  -> MemoryKeyStore / ProvisionalBond
  -> encrypted CSIS read
  -> verification
  -> explicit commit function
```

The later-member commit function refuses writes unless the verifier report matches the stored SIRK and Set Size. A metadata write failure rolls the peer-key file back. An unverified metadata record may remain only after an I/O failure and is explicitly recoverable; it is never marked CSIS verified.

## One-time migration from deprecated R2.4 state

The currently recorded first-member bond was created by the deprecated monolithic path, before the v2 persistent local-identity and transaction boundaries existed. It cannot be safely adopted as the starting point for the first v2 end-to-end HIL.

Archive it once:

```powershell
.\target\debug\be200hub.exe r2 pairing-v2-reset --confirm
```

This moves project-managed files to:

```text
%LOCALAPPDATA%\BE200AudioHub\bonding\archive\reseed-<unix-seconds>\
```

and creates a fresh persistent `local-identity.json`. Windows pairing state is not modified.

## First end-to-end HIL

1. Put both Buds3 Pro earbuds in the case.
2. Open the case and enter formal pairing mode.
3. Run:

```powershell
.\target\debug\be200hub.exe r2 pairing-v2-onboard `
    --seconds 5 `
    --name "Galaxy Buds3 Pro (2F23)" `
    2>&1 | Tee-Object ".\r2-pairing-v2-e2e-1.log"
```

Expected checkpoints:

```text
Primary ExpectedDevice
[R2/CONN-MGR] ... filter_policy=1
Primary ACL: PASS
[R2/BOND-MGR] provisional transaction READY ... persistent store unchanged
Primary CSIS: set_size=2 rank=1
[R2/BOND-COMMIT] PRIMARY PASS

ExpectedSetMember ... RSI=...
[R2/CONN-MGR] ... filter_policy=1
Set-member ACL: PASS
[R2/BOND-MGR] provisional transaction READY ... persistent store unchanged
[R2/SET-VERIFY] PASS
[R2/BOND-COMMIT] PASS
R2 pairing-v2 COMPLETE: 2 CSIS-verified member(s) committed
```

## Failure semantics

- ACL failure: no SMP and no persistent mutation.
- SMP failure: no persistent mutation.
- CSIS read failure: provisional keys are dropped when the process exits; no commit.
- SIRK / Set Size rejection: disconnect and discard provisional keys; no commit.
- Commit I/O failure: peer-key file is rolled back when possible and the error explicitly reports any recoverable partial metadata state.

## Legacy path

`r2 bond-onboard` remains compiled only for comparison and recovery during migration. New HIL and further development must use `pairing-v2-onboard`; the legacy path is frozen and must not receive new transport or CSIS policy.
