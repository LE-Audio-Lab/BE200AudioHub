// ==WindhawkMod==
// @id              be200-quick-settings-probe
// @name            BE200 Quick Settings - Direct Tile PoC
// @description     Experimental direct GridView ItemsWrapGrid insertion (UI only)
// @version         0.2.0
// @author          STanJK
// @license         GPL-3.0
// @include         ShellHost.exe
// @architecture    x86-64
// @compilerOptions -lole32 -loleaut32 -lruntimeobject -lshlwapi
// ==/WindhawkMod==

// ==WindhawkModReadme==
/*
# BE200 Quick Settings - Direct Tile PoC, Windows 11 build 26200

Front-end only. Inserts a visual GridViewItem directly into the native
ItemsWrapGrid of TogglesGroup / RootGridView, after Bluetooth (index 2).
Click writes [BE200-QS/CLICK] to the Windhawk log. It does NOT control BE200,
call AudioHub, open USB, create IPC or disconnect Bluetooth.

**WARNING:** The target ItemsWrapGrid is an OS-owned virtualizing ItemsHost.
The item is not registered as a QuickActionViewModel. The OS may reject the
insert, remove the item, mis-layout other items, or crash ShellHost.exe.
Disable the mod from Windhawk if the panel behaves unexpectedly.

The XAML Diagnostics/TAP pattern is adapted from the GPL-3.0 Windhawk
Windows 11 Notification Center Styler by m417z. Button construction follows
the pattern in Windows 11 Start Menu Buttons (its own license applies).
*/
// ==/WindhawkModReadme==

#include <windows.h>
#include <ocidl.h>
#include <xamlom.h>
#include <atomic>
#include <cwchar>
#include <vector>

#include <winrt/base.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.System.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Windows.UI.Xaml.Automation.h>

namespace wux = winrt::Windows::UI::Xaml;
namespace controls = winrt::Windows::UI::Xaml::Controls;
namespace media = winrt::Windows::UI::Xaml::Media;
namespace foundation = winrt::Windows::Foundation;
namespace automation = winrt::Windows::UI::Xaml::Automation;

static constexpr wchar_t kTileTag[] = L"BE200-QS-DIRECT-TILE-v1";
static std::atomic_bool g_unloading{false};
static std::atomic_bool g_tapInitialized{false};
static thread_local bool g_inserting = false;
// See Notification Center Styler: on the Advise worker, opt out of the
// unsafe process-wide composition diagnostics visual-tree walker.
static thread_local bool g_disableCompositionDiagOnThisThread = false;

static thread_local std::vector<winrt::weak_ref<controls::ItemsWrapGrid>> g_panels;

static HMODULE ThisModule() {
    HMODULE module = nullptr;
    GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                           GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                       reinterpret_cast<LPCWSTR>(&ThisModule), &module);
    return module;
}

static bool IsOurItem(wux::UIElement const& element) {
    auto item = element.try_as<controls::GridViewItem>();
    if (!item) return false;
    auto tag = winrt::unbox_value_or<winrt::hstring>(item.Tag(), winrt::hstring{});
    return tag == kTileTag;
}

static bool IsTogglesHost(controls::ItemsWrapGrid const& host) {
    bool gridViewFound = false;
    bool togglesGroupFound = false;
    wux::DependencyObject current = host;
    for (int depth = 0; current && depth < 24; ++depth) {
        if (auto grid = current.try_as<controls::GridView>()) {
            if (grid.Name() == L"RootGridView") gridViewFound = true;
        }
        if (auto cc = current.try_as<controls::ContentControl>()) {
            if (cc.Name() == L"TogglesGroup") togglesGroupFound = true;
        }
        current = media::VisualTreeHelper::GetParent(current);
    }
    return gridViewFound && togglesGroupFound;
}

static void InsertDirectTile(controls::ItemsWrapGrid const& host) {
    if (g_unloading || g_inserting || !host || !IsTogglesHost(host)) return;

    auto children = host.Children();
    for (uint32_t i = 0; i < children.Size(); ++i) {
        if (IsOurItem(children.GetAt(i))) return;  // idempotent
    }

    // Wait for native quick actions during fresh Shell startup. The captured
    // 26200 tree has 11 native items, but layout may realize only a subset.
    if (children.Size() < 6) return;

    g_inserting = true;
    try {
        controls::FontIcon bluetoothIcon;
        bluetoothIcon.FontFamily(media::FontFamily(L"Segoe Fluent Icons"));
        bluetoothIcon.Glyph(L"\uE702");  // Bluetooth
        bluetoothIcon.FontSize(17);

        controls::Button action;
        action.Width(96);
        action.Height(48);
        action.Padding({0, 0, 0, 0});
        action.CornerRadius({4, 4, 4, 4});
        action.BorderThickness({1, 1, 1, 1});
        action.Background(media::SolidColorBrush({255, 61, 61, 61}));
        action.BorderBrush(media::SolidColorBrush({255, 81, 81, 81}));
        action.Content(bluetoothIcon);
        automation::AutomationProperties::SetName(action, L"断开 B3P (前端测试)");
        action.Click([](auto&&, auto&&) {
            Wh_Log(L"[BE200-QS/CLICK] disconnect_b3p (UI-only, no backend)");
        });

        controls::TextBlock label;
        label.Text(L"断开 B3P");
        label.FontSize(12);
        label.TextAlignment(wux::TextAlignment::Center);
        label.HorizontalAlignment(wux::HorizontalAlignment::Stretch);
        label.Margin({0, 7, 0, 0});

        controls::StackPanel content;
        content.Orientation(controls::Orientation::Vertical);
        content.Width(96);
        content.Children().Append(action);
        content.Children().Append(label);

        controls::GridViewItem item;
        item.Tag(winrt::box_value(winrt::hstring{kTileTag}));
        item.Width(96);
        item.Height(88);
        item.Padding({0, 0, 0, 0});
        item.Margin({0, 0, 12, 8});
        item.HorizontalContentAlignment(wux::HorizontalAlignment::Stretch);
        item.VerticalContentAlignment(wux::VerticalAlignment::Stretch);
        item.IsTabStop(false);
        item.Content(content);

        // Brute-force insertion directly into the native ItemsHost. No
        // QuickActionViewModel is created or registered.
        auto index = (children.Size() >= 2) ? 2u : children.Size();
        children.InsertAt(index, item);
        g_panels.push_back(winrt::make_weak(host));
        Wh_Log(L"[BE200-QS/UI] INSERT OK index=%u children=%u", index,
               children.Size());
    } catch (winrt::hresult_error const& e) {
        Wh_Log(L"[BE200-QS/UI] INSERT FAILED HRESULT=%08X %s",
               static_cast<unsigned>(e.code()), e.message().c_str());
    } catch (...) {
        Wh_Log(L"[BE200-QS/UI] INSERT FAILED unknown exception");
    }
    g_inserting = false;
}

static void RemoveDirectTileOnUiThread() {
    for (auto const& weak : g_panels) {
        try {
            auto panel = weak.get();
            if (!panel) continue;
            auto children = panel.Children();
            for (uint32_t n = children.Size(); n > 0; --n) {
                if (IsOurItem(children.GetAt(n - 1))) {
                    children.RemoveAt(n - 1);
                    Wh_Log(L"[BE200-QS/UI] REMOVE OK");
                }
            }
        } catch (winrt::hresult_error const& e) {
            Wh_Log(L"[BE200-QS/UI] REMOVE FAILED HRESULT=%08X",
                   static_cast<unsigned>(e.code()));
        }
    }
    g_panels.clear();
}

// A minimal XAML Diagnostics/TAP implementation. On Win11 26200, Quick
// Settings lives in a ShellHost.exe XAML Island; Window::Current().Content()
// is not a reliable way to obtain the ControlCenter root.
class TreeWatcher : public winrt::implements<TreeWatcher,
                                               IVisualTreeServiceCallback2,
                                               winrt::non_agile> {
    winrt::com_ptr<IXamlDiagnostics> diagnostics_;
    winrt::com_ptr<IVisualTreeService3> service_;

public:
    explicit TreeWatcher(winrt::com_ptr<IUnknown> site)
        : diagnostics_(site.as<IXamlDiagnostics>()),
          service_(site.as<IVisualTreeService3>()) {
        AddRef();
        HANDLE thread = CreateThread(nullptr, 0,
            [](LPVOID ptr) -> DWORD {
                auto self = static_cast<TreeWatcher*>(ptr);
                g_disableCompositionDiagOnThisThread = true;
                HRESULT hr = self->service_->AdviseVisualTreeChange(self);
                g_disableCompositionDiagOnThisThread = false;
                Wh_Log(L"[BE200-QS/TAP] AdviseVisualTreeChange: %08X",
                       static_cast<unsigned>(hr));
                self->Release();
                return 0;
            }, this, 0, nullptr);
        if (thread) CloseHandle(thread);
        else {
            Release();
            Wh_Log(L"[BE200-QS/TAP] CreateThread failed %lu", GetLastError());
        }
    }

    void Stop() {
        HRESULT hr = service_->UnadviseVisualTreeChange(this);
        Wh_Log(L"[BE200-QS/TAP] UnadviseVisualTreeChange: %08X",
               static_cast<unsigned>(hr));
    }

    HRESULT STDMETHODCALLTYPE OnVisualTreeChange(
        ParentChildRelation, VisualElement element,
        VisualMutationType mutation) noexcept override {
        if (g_unloading || g_inserting || mutation != Add) return S_OK;
        try {
            foundation::IInspectable inspectable;
            winrt::check_hresult(diagnostics_->GetIInspectableFromHandle(
                element.Handle,
                reinterpret_cast<::IInspectable**>(winrt::put_abi(inspectable))));
            if (auto host = inspectable.try_as<controls::ItemsWrapGrid>()) {
                InsertDirectTile(host);
            } else if (auto item = inspectable.try_as<controls::GridViewItem>()) {
                // If the ItemsWrapGrid appeared before its item containers,
                // retry when native containers become available.
                auto parent = media::VisualTreeHelper::GetParent(item);
                if (auto host = parent.try_as<controls::ItemsWrapGrid>()) {
                    InsertDirectTile(host);
                }
            }
        } catch (winrt::hresult_error const& e) {
            Wh_Log(L"[BE200-QS/TAP] traversal exception %08X",
                   static_cast<unsigned>(e.code()));
        } catch (...) {
            Wh_Log(L"[BE200-QS/TAP] traversal unknown exception");
        }
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE OnElementStateChanged(
        InstanceHandle, VisualElementState, LPCWSTR) noexcept override {
        return S_OK;
    }
};

static winrt::com_ptr<TreeWatcher> g_watcher;
static constexpr CLSID kTapClsid = {
    0xa8b19740, 0x1ddf, 0x46e9, {0xb1,0x44,0x9f,0xb4,0x6c,0xa2,0x51,0x31}};

class WindhawkTap : public winrt::implements<WindhawkTap, IObjectWithSite,
                                             winrt::non_agile> {
    winrt::com_ptr<IUnknown> site_;
public:
    HRESULT STDMETHODCALLTYPE SetSite(IUnknown* site) noexcept override {
        try {
            if (g_watcher) {
                g_watcher->Stop();
                g_watcher = nullptr;
            }
            site_.copy_from(site);
            if (site) {
                // Balance the module reference acquired by XAML diagnostics.
                FreeLibrary(ThisModule());
                g_watcher = winrt::make_self<TreeWatcher>(site_);
            }
            return S_OK;
        } catch (...) {
            return winrt::to_hresult();
        }
    }
    HRESULT STDMETHODCALLTYPE GetSite(REFIID iid, void** out) noexcept override {
        return site_.as(iid, out);
    }
};

template <class T>
class SimpleFactory : public winrt::implements<SimpleFactory<T>, IClassFactory,
                                                winrt::non_agile> {
public:
    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer, REFIID iid,
                                             void** out) noexcept override {
        if (outer) return CLASS_E_NOAGGREGATION;
        try { return winrt::make<T>().as(iid, out); }
        catch (...) { return winrt::to_hresult(); }
    }
    HRESULT STDMETHODCALLTYPE LockServer(BOOL) noexcept override { return S_OK; }
};

extern "C" __declspec(dllexport) HRESULT __stdcall DllGetClassObject(
    REFCLSID clsid, REFIID iid, void** out) {
    if (clsid != kTapClsid) return CLASS_E_CLASSNOTAVAILABLE;
    try { return winrt::make<SimpleFactory<WindhawkTap>>().as(iid, out); }
    catch (...) { return winrt::to_hresult(); }
}

extern "C" __declspec(dllexport) HRESULT __stdcall DllCanUnloadNow() {
    return winrt::get_module_lock() ? S_FALSE : S_OK;
}

static HRESULT AttachDiagnostics() {
    HMODULE module = ThisModule();
    if (!module) return HRESULT_FROM_WIN32(GetLastError());
    wchar_t path[MAX_PATH] = {};
    if (!GetModuleFileNameW(module, path, MAX_PATH))
        return HRESULT_FROM_WIN32(GetLastError());
    HMODULE xaml = LoadLibraryExW(L"Windows.UI.Xaml.dll", nullptr,
                                   LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!xaml) return HRESULT_FROM_WIN32(GetLastError());
    using InitFn = decltype(&InitializeXamlDiagnosticsEx);
    auto init = reinterpret_cast<InitFn>(
        GetProcAddress(xaml, "InitializeXamlDiagnosticsEx"));
    if (!init) return HRESULT_FROM_WIN32(GetLastError());

    HRESULT result = HRESULT_FROM_WIN32(ERROR_NOT_FOUND);
    for (int i = 1; i <= 10000; ++i) {
        wchar_t conn[64];
        wsprintfW(conn, L"VisualDiagConnection%d", i);
        result = init(conn, GetCurrentProcessId(), L"", path, kTapClsid, nullptr);
        if (result != HRESULT_FROM_WIN32(ERROR_NOT_FOUND)) break;
    }
    return result;
}

// Marshal just like Notification Center Styler, using a thread-specific
// CallWndProc hook. Never operate XAML objects from Windhawk's loader thread.
using UiProc = void(WINAPI*)(void*);
static bool OnWindowThread(HWND window, UiProc proc, void* context) {
    static const UINT msg = RegisterWindowMessageW(
        L"Windhawk_BE200QS_RunOnControlCenterThread");
    struct Params { UiProc proc; void* context; };
    DWORD tid = GetWindowThreadProcessId(window, nullptr);
    if (!tid) return false;
    if (tid == GetCurrentThreadId()) { proc(context); return true; }
    HHOOK hook = SetWindowsHookExW(WH_CALLWNDPROC,
        [](int code, WPARAM wp, LPARAM lp) -> LRESULT {
            if (code == HC_ACTION) {
                auto call = reinterpret_cast<CWPSTRUCT*>(lp);
                if (call->message == msg) {
                    auto params = reinterpret_cast<Params*>(call->lParam);
                    params->proc(params->context);
                }
            }
            return CallNextHookEx(nullptr, code, wp, lp);
        }, nullptr, tid);
    if (!hook) return false;
    Params params{proc, context};
    SendMessageW(window, msg, 0, reinterpret_cast<LPARAM>(&params));
    UnhookWindowsHookEx(hook);
    return true;
}

static void InitializeOnUiThread(void*) {
    if (g_unloading || g_tapInitialized.exchange(true)) return;
    HRESULT hr = AttachDiagnostics();
    Wh_Log(L"[BE200-QS/TAP] InitializeXamlDiagnosticsEx: %08X",
           static_cast<unsigned>(hr));
    if (FAILED(hr)) g_tapInitialized = false;
}

// The XAML island is not initialized inside CreateWindowInBand yet. Schedule
// a one-shot, after-creation callback on that window's own message loop.
static bool OnWindowThreadDeferred(HWND hwnd, UiProc proc, void* context) {
    static const UINT msg = RegisterWindowMessageW(
        L"Windhawk_BE200QS_DeferOnControlCenterThread");
    struct Params { UiProc proc; void* context; HHOOK hook; };
    DWORD tid = GetWindowThreadProcessId(hwnd, nullptr);
    if (!tid) return false;
    HHOOK hook = SetWindowsHookExW(WH_GETMESSAGE,
        [](int code, WPARAM wp, LPARAM lp) -> LRESULT {
            if (code == HC_ACTION && wp == PM_REMOVE) {
                MSG* m = reinterpret_cast<MSG*>(lp);
                if (m->message == msg && m->lParam) {
                    auto params = reinterpret_cast<Params*>(m->lParam);
                    params->proc(params->context);
                    UnhookWindowsHookEx(params->hook);
                    delete params;
                    m->lParam = 0;
                }
            }
            return CallNextHookEx(nullptr, code, wp, lp);
        }, nullptr, tid);
    if (!hook) return false;
    auto* params = new Params{proc, context, hook};
    if (!PostMessageW(hwnd, msg, 0, reinterpret_cast<LPARAM>(params))) {
        UnhookWindowsHookEx(hook);
        delete params;
        return false;
    }
    return true;
}

static std::vector<HWND> ControlCenterWindows() {
    std::vector<HWND> windows;
    EnumWindows([](HWND hwnd, LPARAM param) -> BOOL {
        DWORD pid = 0;
        GetWindowThreadProcessId(hwnd, &pid);
        if (pid != GetCurrentProcessId()) return TRUE;
        wchar_t klass[64] = {};
        if (GetClassNameW(hwnd, klass, 64) &&
            _wcsicmp(klass, L"ControlCenterWindow") == 0) {
            reinterpret_cast<std::vector<HWND>*>(param)->push_back(hwnd);
        }
        return TRUE;
    }, reinterpret_cast<LPARAM>(&windows));
    return windows;
}

// The window is created on first Win+A. The separate initialization path
// below is for a mod enabled after the window already exists.
using CreateWindowInBand_t = HWND(WINAPI*)(DWORD,LPCWSTR,LPCWSTR,DWORD,int,int,
    int,int,HWND,HMENU,HINSTANCE,void*,DWORD);
static CreateWindowInBand_t pCreateWindowInBand = nullptr;
static HWND WINAPI CreateWindowInBandHook(DWORD ex,LPCWSTR klass,LPCWSTR title,
    DWORD style,int x,int y,int w,int h,HWND parent,HMENU menu,HINSTANCE instance,
    void* param,DWORD band) {
    HWND hwnd = pCreateWindowInBand(ex,klass,title,style,x,y,w,h,parent,menu,
                                    instance,param,band);
    if (hwnd && reinterpret_cast<ULONG_PTR>(klass) > 0xFFFF &&
        _wcsicmp(klass,L"ControlCenterWindow") == 0) {
        Wh_Log(L"[BE200-QS/WINDOW] created hwnd=%p", hwnd);
        if (!OnWindowThreadDeferred(hwnd, InitializeOnUiThread, nullptr))
            Wh_Log(L"[BE200-QS/WINDOW] defer failed %lu", GetLastError());
    }
    return hwnd;
}

using CreateWindowInBandEx_t = HWND(WINAPI*)(DWORD,LPCWSTR,LPCWSTR,DWORD,
    int,int,int,int,HWND,HMENU,HINSTANCE,void*,DWORD,DWORD);
static CreateWindowInBandEx_t pCreateWindowInBandEx = nullptr;
static HWND WINAPI CreateWindowInBandExHook(DWORD ex,LPCWSTR klass,LPCWSTR title,
    DWORD style,int x,int y,int w,int h,HWND parent,HMENU menu,HINSTANCE instance,
    void* param,DWORD band,DWORD typeFlags) {
    HWND hwnd = pCreateWindowInBandEx(ex,klass,title,style,x,y,w,h,parent,menu,
                                      instance,param,band,typeFlags);
    if (hwnd && reinterpret_cast<ULONG_PTR>(klass) > 0xFFFF &&
        _wcsicmp(klass,L"ControlCenterWindow") == 0) {
        Wh_Log(L"[BE200-QS/WINDOW] created (Ex) hwnd=%p", hwnd);
        if (!OnWindowThreadDeferred(hwnd, InitializeOnUiThread, nullptr))
            Wh_Log(L"[BE200-QS/WINDOW] defer Ex failed %lu", GetLastError());
    }
    return hwnd;
}

// XAML diagnostics otherwise installs a composition-watcher with a known
// concurrent visual creation heap race. Return DisableCompositionDiag=1 only
// to the thread running AdviseVisualTreeChange, without editing the registry.
using RegOpenKeyExW_t = decltype(&RegOpenKeyExW);
static RegOpenKeyExW_t pRegOpenKeyExW = nullptr;
static LSTATUS WINAPI RegOpenKeyExWHook(HKEY root, LPCWSTR subkey,
    DWORD options, REGSAM sam, PHKEY out) {
    LSTATUS result = pRegOpenKeyExW(root, subkey, options, sam, out);
    if (result != ERROR_SUCCESS && g_disableCompositionDiagOnThisThread &&
        root == HKEY_LOCAL_MACHINE && subkey &&
        _wcsicmp(subkey, L"Software\\Microsoft\\XAML\\Debug") == 0) {
        return pRegOpenKeyExW(HKEY_LOCAL_MACHINE, L"Software\\Microsoft",
                              options, sam, out);
    }
    return result;
}
using RegQueryValueExW_t = decltype(&RegQueryValueExW);
static RegQueryValueExW_t pRegQueryValueExW = nullptr;
static LSTATUS WINAPI RegQueryValueExWHook(HKEY key, LPCWSTR name,
    LPDWORD reserved, LPDWORD type, LPBYTE data, LPDWORD bytes) {
    if (!g_disableCompositionDiagOnThisThread || !name ||
        _wcsicmp(name,L"DisableCompositionDiag") != 0)
        return pRegQueryValueExW(key,name,reserved,type,data,bytes);
    if (type) *type = REG_DWORD;
    if (data && (!bytes || *bytes < sizeof(DWORD))) {
        if (bytes) *bytes = sizeof(DWORD);
        return ERROR_MORE_DATA;
    }
    if (data) *reinterpret_cast<DWORD*>(data) = 1;
    if (bytes) *bytes = sizeof(DWORD);
    return ERROR_SUCCESS;
}

BOOL Wh_ModInit() {
    g_unloading = false;
    Wh_Log(L"[BE200-QS/BOOT] start pid=%lu; front-end only",
           GetCurrentProcessId());
    auto user32 = GetModuleHandleW(L"user32.dll");
    auto create = reinterpret_cast<void*>(
        GetProcAddress(user32, "CreateWindowInBand"));
    if (create) {
        Wh_SetFunctionHook(create, reinterpret_cast<void*>(CreateWindowInBandHook),
                           reinterpret_cast<void**>(&pCreateWindowInBand));
    }
    auto createEx = reinterpret_cast<void*>(
        GetProcAddress(user32, "CreateWindowInBandEx"));
    if (createEx) {
        Wh_SetFunctionHook(createEx, reinterpret_cast<void*>(CreateWindowInBandExHook),
                           reinterpret_cast<void**>(&pCreateWindowInBandEx));
    }
    if (HMODULE kbase = GetModuleHandleW(L"kernelbase.dll")) {
        if (auto f = GetProcAddress(kbase,"RegOpenKeyExW"))
            Wh_SetFunctionHook(reinterpret_cast<void*>(f),
                reinterpret_cast<void*>(RegOpenKeyExWHook),
                reinterpret_cast<void**>(&pRegOpenKeyExW));
        if (auto f = GetProcAddress(kbase,"RegQueryValueExW"))
            Wh_SetFunctionHook(reinterpret_cast<void*>(f),
                reinterpret_cast<void*>(RegQueryValueExWHook),
                reinterpret_cast<void**>(&pRegQueryValueExW));
    }
    return TRUE;
}

void Wh_ModAfterInit() {
    auto windows = ControlCenterWindows();
    Wh_Log(L"[BE200-QS/BOOT] existing ControlCenterWindow count=%u",
           static_cast<unsigned>(windows.size()));
    for (HWND hwnd : windows) OnWindowThread(hwnd, InitializeOnUiThread, nullptr);
}

void Wh_ModUninit() {
    g_unloading = true;
    if (g_watcher) {
        g_watcher->Stop();
        g_watcher = nullptr;
    }
    for (HWND hwnd : ControlCenterWindows()) {
        OnWindowThread(hwnd, [](void*) { RemoveDirectTileOnUiThread(); }, nullptr);
    }
    Wh_Log(L"[BE200-QS/BOOT] unloaded");
}