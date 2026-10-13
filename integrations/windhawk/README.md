# BE200 Quick Settings — Direct Tile experiment

Branch: `experiment/win11-quick-settings` (base `c31c7f7`)

## Stage and isolation
Windows 11 build `26200` front-end-only PoC. **No Bluetooth operations, no WinUSB/HCI and no AudioHub IPC**. The tile click only emits `[BE200-QS/CLICK]` in Windhawk's log.

The mod directly inserts a synthetic `GridViewItem` at index 2 of the **native** `ItemsWrapGrid`, under `TogglesGroup > PaginatedGridView > GridView#RootGridView`. It does *not* create or register a `ControlCenter.QuickActionViewModel`.

This is an invasive and **not yet compiled or tested on the target Windows PC**. Windows owns the ItemsHost. The insertion may be rejected, silently removed, or cause the Control Center/ShellHost to crash. This build intentionally does not provide an alternative layout fallback.

## Installation / smoke test

1. Disable other mods targeting the Windows Notification Center / Quick Settings.
2. In Windhawk, create a new mod from `be200-quick-settings.wh.cpp`, compile, enable it **for testing only**.
3. Open `Win+A` and inspect Quick Actions, including the second page if needed.
4. Look at Windhawk logs for:
   - `[BE200-QS/BOOT] start`
   - `[BE200-QS/TAP] InitializeXamlDiagnosticsEx` and `AdviseVisualTreeChange`
   - `[BE200-QS/UI] INSERT OK` or `INSERT FAILED HRESULT=...`
   - `[BE200-QS/CLICK] disconnect_b3p (UI-only, no backend)` when clicking the new tile
5. Close/reopen Win+A to inspect persistence; disable the mod and verify `[BE200-QS/UI] REMOVE OK`.

If the test causes ShellHost instability, disable the Windhawk mod and restart ShellHost or sign out/in. If necessary, launch Windhawk with mods disabled to recover; no registry Quick Actions layout or Bluetooth driver is modified by this PoC.

## Source derivation and hooks

- `Windows 11 Notification Center Styler` (GPL-3.0): XAML Diagnostics/TAP, process creation hooks, UI-thread marshaling and temporary composition-diagnostics-registry virtualization.
- `Windows 11 Start Menu Buttons` (MIT, copyright 2026 Salyts): general pattern for constructing XAML Buttons and binding Click.
- Captured XAML UI tree (Build 26200): `ControlCenterRegion -> TogglesGroup -> PaginatedGridView -> RootGridView -> ItemsPresenter -> ItemsWrapGrid`; 11 QuickActionViewModel items and 3-column wrap layout.

The mod uses a GPL-3.0 declaration because its diagnostics integration derives from GPL-3.0 upstream. Preserve the MIT notice for any verbatim source reused from the Start Menu Buttons project.

## Deliberately not implemented

- Backend IPC, real B3P link state, reconnect/disconnect, session teardown, controller power.
- Native ViewModel insertion, pin persistence, provider registration or stable pagination.
- Cross-build compatibility and automated native Windows build testing.
