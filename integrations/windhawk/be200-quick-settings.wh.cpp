// ==WindhawkMod==
// @id be200-quick-settings-probe
// @name BE200 Quick Settings (experimental probe)
// @description Non-invasive ShellHost lifecycle probe for a future B3P disconnect Quick Action.
// @version 0.1.0
// @author STanJK
// @license MIT
// @include ShellHost.exe
// @include ShellExperienceHost.exe
// @architecture x86-64
// ==/WindhawkMod==

// ==WindhawkModReadme==
/*
# BE200 Quick Settings - frontend probe

This is a *probe only*. No Quick Settings button is installed yet.
No Bluetooth, WinUSB, HCI, ACL, CIG, CIS or AudioHub operation is performed.

Open this file in Windhawk's "Create a new mod" editor, compile, enable,
then open Win+A. Check the Windhawk logs for [BE200-QS] messages.
The next stage needs an exported UWPSpy XAML tree from the target Windows build.

Source: integrations/windhawk/be200-quick-settings.wh.cpp
*/
// ==/WindhawkModReadme==

#include <windows.h>

// Frontend-only contracts. These are not serialized IPC structures.
namespace be200_qs {

enum class SessionState {
    Unknown,
    Disconnected,
    Connected,
    Disconnecting,
    Error
};

enum class ActionResult {
    BackendUnavailable,
    Accepted,
    Rejected,
    Failed
};

// The intended tile is a one-shot action, NOT a Bluetooth radio power toggle.
constexpr wchar_t kTileLabel[] = L"断开 B3P";
constexpr wchar_t kActionName[] = L"disconnect_b3p";
constexpr unsigned kProtocolVersion = 1;

// Boundary between Shell UI and a future out-of-process control helper.
// Do not open the BE200 USB interface or execute HCI from ShellHost.exe.
// The implementation must return promptly; completion is asynchronous.
ActionResult RequestDisconnectB3P() noexcept {
    Wh_Log(L"[BE200-QS/COMMAND] %s ignored: backend not connected (probe only)",
           kActionName);
    return ActionResult::BackendUnavailable;
}

// Future XAML Button::Click handler will call this function after
// successful UI attachment and state validation. Not wired in this PoC.
void OnTileClicked() noexcept {
    const ActionResult result = RequestDisconnectB3P();
    Wh_Log(L"[BE200-QS/UI] %s click dispatched; result=%d",
           kTileLabel, static_cast<int>(result));
}

// Future XAML implementation will locate the actual Quick Actions
// container using the target build's UWPSpy tree. We deliberately avoid
// guessing visual-tree indices or mutating system-owned controls here.
bool TryAttachQuickSettingsUi() noexcept {
    Wh_Log(L"[BE200-QS/UI] No XAML hook installed; awaiting verified tree");
    return false;
}

void DetachQuickSettingsUi() noexcept {
    // Placeholder: later release event revokers, references, and inserted UI.
}

}  // namespace be200_qs

BOOL Wh_ModInit() {
    Wh_Log(L"[BE200-QS/BOOT] probe initialized, pid=%lu", GetCurrentProcessId());
    return TRUE;
}

void Wh_ModAfterInit() {
    be200_qs::TryAttachQuickSettingsUi();
}

void Wh_ModBeforeUninit() {
    be200_qs::DetachQuickSettingsUi();
}

void Wh_ModUninit() {
    Wh_Log(L"[BE200-QS/BOOT] probe unloaded");
}
