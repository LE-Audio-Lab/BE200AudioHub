# Native Windows Quick Settings (Rust-only / experimental)

## Current state

**Stage N1**: the Rust executable has `ui quick-settings inject`, and a Rust
`cdylib` implements a COM TAP factory, `IObjectWithSite`, and a visual-tree
callback. The launcher requests that Windows XAML Diagnostics load this DLL into
`ShellHost.exe` on Windows 11 build 26200. Neither executable opens BE200,
touches HCI/CIS, nor sends B3P disconnects.

**Not yet implemented**: inserting the synthetic GridViewItem into the XAML
`ItemsWrapGrid`, native Click handler, persistence, unload/reversal.
The command is currently an **attach/probe**, not a functioning new tile.
Both launcher and TAP are unverified on a real Windows host; this commit is
an implementation scaffold, NOT a passing Windows HIL result.

## Build (Windows)

```powershell
git fetch origin
# In a separate worktree/clone; do not switch the active CIS development checkout.
git switch experiment/win11-quick-settings
cargo build
```

This experimental branch adds the TAP as a default workspace member, so
both binaries should appear in `target\debug`:

- `be200hub.exe`
- `be200_qs_tap.dll`

## Native attach probe

Open Win+A to ensure the ControlCenterWindow exists, then:

```powershell
.\target\debug\be200hub.exe ui quick-settings inject
```

The CLI prints `ShellHost.exe` PID and HRESULT. A nonnegative HRESULT means
the Diagnostics injection request was accepted, **not** that a tile rendered.
The TAP logs in:

`%LOCALAPPDATA%\BE200AudioHub\logs\qs-tap.log`

Look for:

- `SetSite received. Rust native TAP is loaded inside ShellHost.`
- `AdviseVisualTreeChange HRESULT=0x00000000`
- `TARGET FOUND: ItemsWrapGrid ... direct tile insertion NOT YET ENABLED`

If TAP reports failure or ShellHost crashes, preserve the exact HRESULT/log
and restore the shell via sign-out/restart. This probe does not modify
drivers, Bluetooth controller state, or the Quick Actions registry layout.

## Architecture rationale

The external CLI cannot mutate foreign-process UWP XAML runtime objects.
`InitializeXamlDiagnosticsEx` loads our native Rust COM TAP into the ShellHost
process; `IObjectWithSite::SetSite` receives the diagnostics site. A separate
thread calls `AdviseVisualTreeChange` to avoid ShellHost deadlocks known in the
upstream Notification Center Styler implementation.

Next stage N2 will use the discovered ItemsWrapGrid handle and the real
`Windows.UI.Xaml.Controls.IPanel::Children` WinRT collection to insert a
native UI element. **Children is not an ordinary dependency property**,
so treating `IVisualTreeService::SetProperty` as an equivalent of
`Panel.Children().InsertAt` is not valid.

No Windhawk installation is needed by the Rust-native path. The older
`integrations/windhawk/` folder is archived research and is *not used*
by `be200hub ui quick-settings inject`.

### Source docs
- https://learn.microsoft.com/windows/win32/api/xamlom/nf-xamlom-initializexamldiagnosticsex
- https://learn.microsoft.com/windows/win32/api/xamlom/nf-xamlom-ivisualtreeservicecallback-onvisualtreechange
- https://mingw.googlesource.com/mingw-w64/+/refs/heads/v12.x/mingw-w64-headers/include/xamlom.idl
