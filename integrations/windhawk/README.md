# Windows 11 Quick Settings — BE200 experiment

Branch: `experiment/win11-quick-settings`
Base: `r2/stack-integration-bootstrap` at `c31c7f76588870a61c0ca76a10d76170817d53e8`.

## Boundary

This is a **frontend-only** proof of concept. Do **not** operate the BE200 radio, HCI, GATT, ACL, ASE, CIG or CIS from ShellHost. Do not modify the existing AudioHub Rust stack on this branch for the initial test.

Goal: add one native-looking **断开 B3P** action to Win+A. No device chooser, pairing, power toggle, radio enumeration, or automatic reconnect in this experiment.

Stage A (committed): Windhawk mod with ShellHost lifecycle logs and a disconnected backend stub. **Does not currently inject a button.**

Stage B (awaiting local XAML tree): locate stable insertion point on the target Windows 11 build, insert a XAML Button, attach Click handler; initially log only. Ensure event revocation and removal on unload, and guard duplicate injection when Win+A reopens.

Stage C (later): IPC from Shell helper to AudioHub's existing HCI owner. UI never touches WinUSB directly.

## Installation

1. Install Windhawk from https://windhawk.net/ .
2. Obtain UWPSpy release from https://github.com/m417z/UWPSpy/releases .
3. In Windhawk select "Create a new mod", paste the contents of `integrations/windhawk/be200-quick-settings.wh.cpp`, compile and enable.
4. Check logs for `[BE200-QS/BOOT]`. Open Win+A. **No extra button is expected at this stage.**
5. Use UWPSpy against the Shell process containing the Control Center (typically ShellHost.exe) while Win+A is open. Export/screenshot the XAML tree around Quick Actions, its GridView, Pager and containers.
6. Record `Get-ComputerInfo | Select WindowsProductName, OsVersion, OsBuildNumber` and `Get-Process ShellHost,ShellExperienceHost -ErrorAction SilentlyContinue | Select ProcessName,Id,Path`.

Avoid enabling multiple Shell UI customization mods while investigating the visual tree.

## API sketch (not implemented)

Action: `disconnect_b3p`, protocol version 1.

Proposed request: `{"version":1,"request_id":"<unique-id>","command":"disconnect_b3p"}`.
Proposed immediate response: `{"request_id":"<unique-id>","accepted":true}` or a structured error.
Proposed eventual status: `unknown | disconnected | connected | disconnecting | error`.

Acceptance is **not** successful disconnection. Backend should report actual completion independently. Prefer one-shot idempotent disconnect of B3P over toggle semantics, and avoid tearing down unrelated connections. No transport choice is committed; Windows named pipes are a candidate.

Do not reuse stale ACL/CIS handles. When the AudioHub protocol stack is ready, hand off the request to its live session owner, which performs state-appropriate teardown and publishes a confirmed result.

## Rollback

Disable/uninstall this mod in Windhawk. It does not persist changes to Windows Quick Actions registry layouts or Windows drivers. Shell modifications remain experimental and version-dependent.
