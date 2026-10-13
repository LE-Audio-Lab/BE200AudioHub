# BE200 Quick Settings: Rust native N2 experiment

Branch: `experiment/win11-quick-settings` (separate from active CIS/R5 development).

## Scope

**Pure frontend**, one synthetic `断开 B3P` tile injected directly into the native Windows 11 Quick Settings `RootGridView`/`ItemsWrapGrid`. Click only logs an event, no WinUSB, Bluetooth state, BAP, CIS, IPC, or AudioHub disconnect.

This is the **invasive proof-of-concept** requested for Windows 11 build 26200. The native `ItemsWrapGrid` is OS-managed and can reject, recycle, or crash when its generated children are edited. No claim of successful Windows HIL validation is made at source-commit time.

## Architecture

- `be200hub.exe ui quick-settings inject`: standalone Rust CLI route; calls `InitializeXamlDiagnosticsEx` for a verified `ShellHost.exe`/`ControlCenterWindow`.
- `be200_qs_tap.dll`: Rust `cdylib` implementing `DllGetClassObject`, COM `IClassFactory`, `IObjectWithSite` and XAML Diagnostics `IVisualTreeServiceCallback`.
- XAML callback builds a parent map; requires `TogglesGroup -> RootGridView -> ItemsWrapGrid` (avoids `SlidersGroup`'s `RootGridView`).
- A `Windows.UI.Xaml.Markup.XamlReader` factory creates one complete XAML `GridViewItem`, including Bluetooth icon, label and Button.
- An `IFrameworkElement::FindName` + `IButtonBase::add_Click` call connects a native `Windows.UI.Xaml.RoutedEventHandler` which writes a **UI-only log**.
- The native `Children` collection is obtained either by `IVisualTreeService2::GetPropertyIndex/GetProperty` or by `IXamlDiagnostics::GetIInspectableFromHandle` -> `IPanel::Children` -> `GetHandleFromIInspectable`.
- `IVisualTreeService::AddChild(collection_handle, grid_item_handle, 2)` performs the direct injection, not a QuickActionViewModel data-source registration.

## Build on Windows

Use a **separate clone/worktree** to avoid touching the active R5/CIS branch and its dirty `Cargo.lock`.

```powershell
cd C:\STanJK\Development\LEAudioLab\BE200
# Skip clone if BE200QuickSettings already exists and uses the experiment branch.
git clone -b experiment/win11-quick-settings https://github.com/STanJK/BE200AudioHub.git BE200QuickSettings
cd .\BE200QuickSettings
cargo build
cargo test -p be200-quick-settings-tap
```

Expect `target\debug\be200hub.exe` and `target\debug\be200_qs_tap.dll`.

## HIL smoke test

Open `Win+A` before executing (a `ControlCenterWindow` must exist), then:

```powershell
.\target\debug\be200hub.exe ui quick-settings inject
Get-Content "$env:LOCALAPPDATA\BE200AudioHub\logs\qs-tap.log" -Tail 60
```

A nonnegative HRESULT from the launcher **only indicates that Windows accepted the TAP attach request**. Confirm independently:

1. `[BE200-QS/TAP] SetSite received.`
2. `AdviseVisualTreeChange HRESULT=0x00000000`
3. `[N2] Children collection found` or `[N2] IPanel::Children native fallback`
4. `[N2] XamlReader.Load created a native GridViewItem`
5. `[N2] Button.Click registered`
6. `[N2] INSERT OK index=2` **and** the tile is actually visible
7. Click it; `[CLICK] disconnect_b3p (UI ONLY; no backend)`

If a stage fails, preserve its HRESULT, Windows event logs and the TAP log. Do **not** immediately retry in a crashing ShellHost process.

## N2.2: rebuild when ShellHost locks the old TAP DLL

Once Windows has loaded `be200_qs_tap.dll`, Cargo cannot overwrite that
DLL in `target\\debug`; Windows returns `os error 5` (access denied).
This does **not** mean the new Rust code failed to compile.

The recommended development loop avoids the occupied output path:

```powershell
# In the isolated BE200QuickSettings worktree
git pull --ff-only
cargo test -p be200-quick-settings-tap
cargo build -p be200-quick-settings-tap --target-dir target-qs-n22
```

The new DLL is `target-qs-n22\\debug\\be200_qs_tap.dll`.
The unchanged launcher can point directly to it:

```powershell
# Save work; sign out and back in first to exit the OLD ShellHost session.
# Then open Win+A and inject only once:
.\\target\\debug\\be200hub.exe ui quick-settings inject --tap ".\\target-qs-n22\\debug\\be200_qs_tap.dll"
```

Use a fresh target directory for later iterations while an older DLL is
loaded. **Do not inject again in the same old ShellHost process**: the
existing TAP is process-lifetime pinned. Restarting Explorer alone does
not guarantee the ShellHost DLL is unloaded.

N2.1 regression fix: `is_quick_settings_wrap` previously rejected a
complete `RootGridView -> TogglesGroup` chain if an even higher ancestor
had not arrived. Now missing outer ancestors do not invalidate anchors
already confirmed along the same parent chain. The updated test also
ensures a missing `TogglesGroup` is rejected.

## Unload and limitations

This N2 experimental build is deliberately process-lifetime pinned; there is **no clean detach or reinstall command** yet. To remove the test tile, restart the `ShellHost.exe` process or sign out and back in; it will not reinject automatically. `DllCanUnloadNow` returns S_FALSE. Do not leave this experiment enabled for daily use until lifecycle handling is implemented.

This mod does not write Quick Actions registry layout, install Windows drivers, pair Bluetooth devices, or start BE200 firmware. The previous `integrations/windhawk` variant is **not loaded or required**.

## ABI references

- Microsoft `xamlom.idl`: IVisualTreeService3, IXamlDiagnostics, callback structs and GUIDs.
- Windows.UI.Xaml.Markup.IXamlReaderStatics: `Load` slot 6.
- Windows.UI.Xaml.IFrameworkElement: `FindName` slot 51.
- Windows.UI.Xaml.Controls.Primitives.IButtonBase: `add_Click` slot 14.
- Windows.UI.Xaml.Controls.IPanel: `Children` slot 6.
- Windows.UI.Xaml.RoutedEventHandler: delegate IID `a856e674-b0b6-4bc3-bba8-1ba06e40d4b5`.

Sources: https://mingw.googlesource.com/mingw-w64/+/refs/heads/v12.x/mingw-w64-headers/include/xamlom.idl ; https://learn.microsoft.com/windows/win32/api/xamlom/nf-xamlom-ivisualtreeservice-addchild