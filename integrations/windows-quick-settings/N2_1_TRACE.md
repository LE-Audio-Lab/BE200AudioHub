# N2.1: visual-tree tracing and late-ancestor discovery

Status: committed source, **not yet Windows-compiled or HIL-tested**.

## Previous failure
The Windows launcher successfully invoked InitializeXamlDiagnosticsEx and the Rust TAP
reported SetSite and AdviseVisualTreeChange success. No [N2] insertion logs followed.
Previous versions did not log ordinary OnVisualTreeChange events, so it was impossible
to tell whether the events were absent or simply filtered by the strict
TogglesGroup -> RootGridView -> ItemsWrapGrid ancestor matching.

## N2.1 changes
- Every TAP log includes ShellHost PID and Unix-millisecond timestamp.
- Bounded first-event and named-anchor callbacks, plus a 1500ms callback/node summary.
- IVisualTreeServiceCallback2 interface implemented as a compatibility extension.
- Runtime IFrameworkElement::Name read as fallback if VisualElement.Name is empty.
- Previously seen ItemsWrapGrid nodes are rechecked if named ancestors arrive later.
- Additional unit test covers late-ancestor matching.

## Safe test in the isolated worktree
Prior N2 DLL remains loaded for the ShellHost process lifetime. Save files and **sign
out/in once before re-testing**; do not keep injecting repeatedly into the same
process. The experimental worktree remains independent from the active CIS branch.

```powershell
cd C:\STanJK\Development\LEAudioLab\BE200\BE200QuickSettings
git pull --ff-only
cargo build -p be200audiohub -p be200-quick-settings-tap
cargo test -p be200-quick-settings-tap
# After opening Win+A:
.\target\debug\be200hub.exe ui quick-settings inject
Get-Content "$env:LOCALAPPDATA\BE200AudioHub\logs\qs-tap.log" -Tail 120
```

## Decision table
- `callbacks=0`: subscription success does not imply any visual-tree
  events were delivered; examine when the panel gets created.
- `callbacks>0, wraps=0`: inspect emitted types and check whether the
  target ItemsWrapGrid was realized / in scope.
- `wraps>0, named_anchors=0`: names are missing despite runtime fallback.
- `target wrap matched`: insertion path reached; next log is exact HRESULT.
- `INSERT OK`: API inserted, but OS-managed ItemsHost may recycle the child;
  verify visible UI and click logging independently.

This stage is frontend-only. Nothing connects to Bluetooth, B3P or HCI.
