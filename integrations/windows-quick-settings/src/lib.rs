//! Rust-native TAP for Windows XAML Diagnostics.
//!
//! Loads inside ShellHost.exe.  This first milestone proves our own Rust COM
//! server can receive the Windows XAML tree without Windhawk or C++.
//! N2: experimental insertion of one UI-only tile into the native ItemsWrapGrid.
//! This is invasive: a Windows-owned ItemsHost can reject/recycle the child.
#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::ptr::null_mut;
use std::sync::{atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering}, Mutex, OnceLock};

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_NOINTERFACE: i32 = 0x80004002u32 as i32;
const E_POINTER: i32 = 0x80004003u32 as i32;
const E_FAIL: i32 = 0x80004005u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: i32 = 0x80040111u32 as i32;
const CLASS_E_NOAGGREGATION: i32 = 0x80040110u32 as i32;

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Guid { d1: u32, d2: u16, d3: u16, d4: [u8; 8] }

const CLSID_TAP: Guid = Guid { d1: 0xf18c96a3, d2: 0x281a, d3: 0x4dd0,
    d4: [0x9c,0x81,0xb0,0x20,0x40,0xb8,0x9e,0x3c] };
const IID_IUNKNOWN: Guid = Guid { d1:0, d2:0, d3:0, d4:[0xc0,0,0,0,0,0,0,0x46] };
const IID_ICLASSFACTORY: Guid = Guid { d1:1, d2:0, d3:0, d4:[0xc0,0,0,0,0,0,0,0x46] };
const IID_IOBJECTWITHSITE: Guid = Guid { d1:0xfc4801a3,d2:0x2ba9,d3:0x11cf,
    d4:[0xa2,0x29,0,0xaa,0,0x3d,0x73,0x52] };
const IID_IVISUALTREESERVICE3: Guid = Guid { d1:0x0e79c6e0,d2:0x85a0,d3:0x4be8,
    d4:[0xb4,0x1a,0x65,0x5c,0xf1,0xfd,0x19,0xbd] };
const IID_IVISUALTREESERVICECALLBACK: Guid = Guid { d1:0xaa7a8931,d2:0x80e4,d3:0x4fec,
    d4:[0x8f,0x3b,0x55,0x3f,0x87,0xb4,0x96,0x6e] };
const IID_IVISUALTREESERVICECALLBACK2: Guid = Guid { d1:0xbad9eb88,d2:0xae77,d3:0x4397,
    d4:[0xb9,0x48,0x5f,0xa2,0xdb,0x0a,0x19,0xea] };
const IID_IXAMLDIAGNOSTICS: Guid = Guid { d1:0x18c9e2b6,d2:0x3f43,d3:0x4116,
    d4:[0x9f,0x2b,0xff,0x93,0x5d,0x77,0x70,0xd2] };
const IID_IXAMLREADERSTATICS: Guid = Guid { d1:0x9891c6bd,d2:0x534f,d3:0x4955,
    d4:[0xb8,0x5a,0x8a,0x8d,0xc0,0xdc,0xa6,0x02] };
const IID_IFRAMEWORKELEMENT: Guid = Guid { d1:0xa391d09b,d2:0x4a99,d3:0x4b7c,
    d4:[0x9d,0x8d,0x6f,0xa5,0xd0,0x1f,0x6f,0xbf] };
const IID_IPANEL: Guid = Guid { d1:0xa50a4bbd,d2:0x8361,d3:0x469c,
    d4:[0x90,0xda,0xe9,0xa4,0x0c,0x74,0x74,0xdf] };
const IID_IBUTTONBASE: Guid = Guid { d1:0xfa002c1a,d2:0x494e,d3:0x46cf,
    d4:[0x91,0xd4,0xe1,0x4a,0x8d,0x79,0x86,0x74] };
const IID_IINSPECTABLE: Guid = Guid { d1:0xaf86e2e0,d2:0xb12d,d3:0x4c6a,
    d4:[0x9c,0x5a,0xd7,0xaa,0x65,0x10,0x1e,0x90] };
const IID_ROUTED_EVENT_HANDLER: Guid = Guid { d1:0xa856e674,d2:0xb0b6,d3:0x4bc3,
    d4:[0xbb,0xa8,0x1b,0xa0,0x6e,0x40,0xd4,0xb5] };
const IID_ICUSTOMPROPERTYPROVIDER: Guid = Guid { d1:0x7c925755,d2:0x3e48,d3:0x42b4,
    d4:[0x86,0x77,0x76,0x37,0x22,0x67,0x03,0x3f] };
const IID_IPROPERTYVALUE: Guid = Guid { d1:0x4bd682dd,d2:0x7554,d3:0x40e9,
    d4:[0x9a,0x9b,0x82,0x65,0x4e,0xde,0x7e,0x62] };
const IID_IACTIVATIONFACTORY: Guid = Guid { d1:0x00000035,d2:0x0000,d3:0x0000,
    d4:[0xc0,0x00,0x00,0x00,0x00,0x00,0x00,0x46] };


type Hwnd = *mut c_void;
type SubclassProc = unsafe extern "system" fn(Hwnd,u32,usize,isize,usize,usize)->isize;
const WM_BE200_QS_PROBE: u32 = 0x8000 + 0x4B2;
const WM_BE200_QS_DUPLICATE_VM: u32 = 0x8000 + 0x4B3;
const WM_BE200_QS_CUSTOMIZE_CARRIER: u32 = 0x8000 + 0x4B4;
const WM_BE200_QS_WIRE_BUTTON: u32 = 0x8000 + 0x4B5;
const CARRIER_INDEX: u32 = 2;
const QS_SUBCLASS_ID: usize = 0xBE20_0200;
const EXPECTED_NATIVE_TILES: u32 = 11;

#[link(name="kernel32")]
extern "system" {
    fn OutputDebugStringW(s: *const u16);
    fn GetCurrentThreadId() -> u32;
    fn LoadLibraryW(name:*const u16)->*mut c_void;
    fn GetProcAddress(module:*mut c_void,name:*const u8)->*mut c_void;
    fn FreeLibrary(module:*mut c_void)->i32;
}
#[link(name="user32")]
extern "system" {
    fn FindWindowW(class_name:*const u16, window_name:*const u16)->Hwnd;
    fn GetWindowThreadProcessId(hwnd:Hwnd, pid:*mut u32)->u32;
    fn PostMessageW(hwnd:Hwnd,msg:u32,wparam:usize,lparam:isize)->i32;
}
#[link(name="comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd:Hwnd,proc:SubclassProc,id:usize,ref_data:usize)->i32;
    fn DefSubclassProc(hwnd:Hwnd,msg:u32,wparam:usize,lparam:isize)->isize;
}
#[link(name="oleaut32")]
extern "system" {
    fn SysStringLen(bstr: *const u16) -> u32;
    fn SysFreeString(bstr: *mut u16);
}
#[link(name="ole32")]
extern "system" {
    fn CoTaskMemAlloc(size: usize) -> *mut c_void;
    fn CoTaskMemFree(ptr: *mut c_void);
}
#[link(name="runtimeobject")]
extern "system" {
    fn WindowsCreateString(value: *const u16, length: u32, result: *mut *mut c_void) -> i32;
    fn WindowsDeleteString(value: *mut c_void) -> i32;
    fn WindowsGetStringRawBuffer(value: *mut c_void, length: *mut u32) -> *const u16;
    fn RoGetActivationFactory(name: *mut c_void, iid: *const Guid, result: *mut *mut c_void) -> i32;
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

fn log(message: &str) {
    // Include PID and clock time: this is an append-only log across ShellHost lifetimes.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let output = format!("[BE200-QS/TAP] [pid={}] [unix_ms={}] {message}",
        std::process::id(), now.as_millis());
    unsafe { OutputDebugStringW(wide(&output).as_ptr()); }
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("BE200AudioHub").join("logs");
    if create_dir_all(&base).is_ok() {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true)
            .open(base.join("qs-tap.log")) {
            let _ = writeln!(file, "{output}");
        }
    }
}

// Safe only for valid, live COM interface pointers.
unsafe fn slot(ptr: *mut c_void, index: usize) -> *const c_void {
    let vtable = *(ptr as *const *const *const c_void);
    *vtable.add(index)
}
unsafe fn query_interface(ptr: *mut c_void, iid: &Guid, out: *mut *mut c_void) -> i32 {
    let func: unsafe extern "system" fn(*mut c_void,*const Guid,*mut *mut c_void)->i32
        = std::mem::transmute(slot(ptr,0));
    func(ptr, iid, out)
}
unsafe fn com_release(ptr: *mut c_void) {
    if !ptr.is_null() {
        let func: unsafe extern "system" fn(*mut c_void)->u32 = std::mem::transmute(slot(ptr,2));
        func(ptr);
    }
}

// Only use these helpers with live in-process COM pointers on the XAML UI thread.
struct ComPtr(*mut c_void);
impl ComPtr { fn as_raw(&self) -> *mut c_void { self.0 } }
impl Drop for ComPtr { fn drop(&mut self) { unsafe { com_release(self.0); } } }

struct HString(*mut c_void);
impl HString {
    fn new(value: &str) -> Result<Self, i32> {
        let units: Vec<u16> = value.encode_utf16().collect();
        let mut raw = null_mut();
        let hr = unsafe { WindowsCreateString(units.as_ptr(), units.len() as u32, &mut raw) };
        if hr < 0 { Err(hr) } else { Ok(Self(raw)) }
    }
}
impl Drop for HString { fn drop(&mut self) { unsafe { WindowsDeleteString(self.0); } } }

// The UWPSpy export uses FrameworkElement.Name. XamlOM VisualElement.Name
// may be blank on some element types / snapshots; do not rely on it alone.
// Only query these lightweight names for ContentControl/GridView candidates.
unsafe fn framework_element_name(handle: u64) -> Result<String, i32> {
    let diagnostics = DIAGNOSTICS_PTR.load(Ordering::Acquire) as *mut c_void;
    if diagnostics.is_null() { return Err(E_FAIL); }
    let from_handle: unsafe extern "system" fn(*mut c_void, u64, *mut *mut c_void) -> i32 =
        std::mem::transmute(slot(diagnostics, 6)); // IXamlDiagnostics.GetIInspectableFromHandle
    let mut element = null_mut();
    let hr = from_handle(diagnostics, handle, &mut element);
    if hr < 0 { return Err(hr); }
    if element.is_null() { return Err(E_FAIL); }
    let element = ComPtr(element);
    let mut framework = null_mut();
    let hr = query_interface(element.as_raw(), &IID_IFRAMEWORKELEMENT, &mut framework);
    if hr < 0 { return Err(hr); }
    let framework = ComPtr(framework);
    let get_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
        std::mem::transmute(slot(framework.as_raw(), 33)); // IFrameworkElement.get_Name
    let mut raw_name = null_mut();
    let hr = get_name(framework.as_raw(), &mut raw_name);
    if hr < 0 { return Err(hr); }
    let name = HString(raw_name);
    let mut length = 0u32;
    let buffer = WindowsGetStringRawBuffer(name.0, &mut length);
    if buffer.is_null() || length == 0 { return Ok(String::new()); }
    Ok(String::from_utf16_lossy(std::slice::from_raw_parts(buffer, length as usize)))
}

// Deliberately no bluetooth command in this markup or event callback.
const TILE_XAML: &str = r##"
<GridViewItem xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    x:Name="BE200QuickSettingsTile" Width="96" Height="88"
    Padding="0" Margin="0,0,12,8" IsTabStop="False"
    HorizontalContentAlignment="Stretch" VerticalContentAlignment="Stretch">
  <StackPanel Width="96" Orientation="Vertical">
    <Button x:Name="BE200QuickSettingsClick" Width="96" Height="48"
        Padding="0" CornerRadius="4" BorderThickness="1"
        Background="#FF3D3D3D" BorderBrush="#FF515151"
        AutomationProperties.Name="断开 B3P（仅前端测试）">
      <FontIcon FontFamily="Segoe Fluent Icons" Glyph="&#xE702;" FontSize="17"/>
    </Button>
    <TextBlock Text="断开 B3P" FontSize="12" TextAlignment="Center"
        Margin="0,7,0,0"/>
  </StackPanel>
</GridViewItem>
"##;

const CARRIER_TEMPLATE_XAML: &str = r##"
<DataTemplate xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
  <Grid Width="96" Height="72">
    <Grid.RowDefinitions>
      <RowDefinition Height="48"/>
      <RowDefinition Height="24"/>
    </Grid.RowDefinitions>
    <Button x:Name="BE200QuickSettingsN41Button"
        Grid.Row="0" Width="96" Height="48"
        Padding="0" CornerRadius="4" BorderThickness="1"
        Background="#FF3D3D3D" BorderBrush="#FF515151"
        AutomationProperties.AutomationId="BE200.QuickAction.DisconnectB3P"
        AutomationProperties.Name="断开 B3P（仅前端测试）">
      <FontIcon FontFamily="Segoe Fluent Icons" Glyph="&#xE711;" FontSize="17"/>
    </Button>
    <TextBlock Grid.Row="1" Text="断开 B3P" FontSize="12"
        Height="16" Margin="0,8,0,0"
        HorizontalAlignment="Stretch" VerticalAlignment="Top"
        TextAlignment="Center" TextTrimming="CharacterEllipsis"/>
  </Grid>
</DataTemplate>
"##;

#[repr(C)]
struct EventHandler { vtable: *const EventHandlerVtbl, count: AtomicU32 }
#[repr(C)]
struct EventHandlerVtbl {
    query_interface: unsafe extern "system" fn(*mut EventHandler,*const Guid,*mut *mut c_void)->i32,
    add_ref: unsafe extern "system" fn(*mut EventHandler)->u32,
    release: unsafe extern "system" fn(*mut EventHandler)->u32,
    get_iids: unsafe extern "system" fn(*mut EventHandler,*mut u32,*mut *mut Guid)->i32,
    get_runtime_class_name: unsafe extern "system" fn(*mut EventHandler,*mut *mut c_void)->i32,
    get_trust_level: unsafe extern "system" fn(*mut EventHandler,*mut i32)->i32,
    invoke: unsafe extern "system" fn(*mut EventHandler,*mut c_void,*mut c_void)->i32,
}
unsafe extern "system" fn ev_query(this:*mut EventHandler, iid:*const Guid, out:*mut *mut c_void)->i32 {
    if iid.is_null() || out.is_null() { return E_POINTER; }
    *out = null_mut();
    if *iid==IID_IUNKNOWN || *iid==IID_IINSPECTABLE || *iid==IID_ROUTED_EVENT_HANDLER {
        *out = this.cast(); ev_add_ref(this); S_OK
    } else { E_NOINTERFACE }
}
unsafe extern "system" fn ev_add_ref(this:*mut EventHandler)->u32 { (*this).count.fetch_add(1,Ordering::Relaxed)+1 }
unsafe extern "system" fn ev_release(this:*mut EventHandler)->u32 {
    let prior = (*this).count.fetch_sub(1,Ordering::AcqRel);
    if prior==1 { drop(Box::from_raw(this)); 0 } else { prior-1 }
}
unsafe extern "system" fn ev_get_iids(_: *mut EventHandler, count:*mut u32, out:*mut *mut Guid)->i32 {
    if count.is_null() || out.is_null() { return E_POINTER; }
    let ptr=CoTaskMemAlloc(std::mem::size_of::<Guid>()) as *mut Guid;
    if ptr.is_null() { *count=0; *out=null_mut(); return E_FAIL; }
    *ptr=IID_ROUTED_EVENT_HANDLER;
    *count=1; *out=ptr; S_OK
}
unsafe extern "system" fn ev_class_name(_: *mut EventHandler, out:*mut *mut c_void)->i32 {
    if out.is_null() { return E_POINTER; }
    let class="Windows.UI.Xaml.RoutedEventHandler";
    let utf16:Vec<u16>=class.encode_utf16().collect();
    WindowsCreateString(utf16.as_ptr(),utf16.len() as u32,out)
}
unsafe extern "system" fn ev_trust_level(_: *mut EventHandler, out:*mut i32)->i32 {
    if out.is_null() { return E_POINTER; }
    *out=0; S_OK
}
unsafe extern "system" fn ev_invoke(_: *mut EventHandler, _: *mut c_void, _: *mut c_void)->i32 {
    // An FFI callback must never unwind through the OS's COM event dispatcher.
    let _ = std::panic::catch_unwind(|| log("[CLICK] disconnect_b3p (UI ONLY; no backend)"));
    S_OK
}
static EVENT_VTBL: EventHandlerVtbl = EventHandlerVtbl {
    query_interface:ev_query,add_ref:ev_add_ref,release:ev_release,
    get_iids:ev_get_iids,get_runtime_class_name:ev_class_name,
    get_trust_level:ev_trust_level,invoke:ev_invoke,
};

static SERVICE_PTR: AtomicUsize = AtomicUsize::new(0);
static DIAGNOSTICS_PTR: AtomicUsize = AtomicUsize::new(0);
static INSERT_BUSY: AtomicBool = AtomicBool::new(false);
static PROBE_SCHEDULED: AtomicBool = AtomicBool::new(false);
static PENDING_GRID: AtomicU64 = AtomicU64::new(0);
static DUPLICATE_NATIVE_VM_MODE: AtomicBool = AtomicBool::new(false);
static CUSTOM_TILE_MODE: AtomicBool = AtomicBool::new(false);
static DUPLICATE_VM_SCHEDULED: AtomicBool = AtomicBool::new(false);
static DUPLICATE_VM_DONE: AtomicBool = AtomicBool::new(false);
static CARRIER_ARMED: AtomicBool = AtomicBool::new(false);
static CUSTOMIZE_SCHEDULED: AtomicBool = AtomicBool::new(false);
static CUSTOMIZE_DONE: AtomicBool = AtomicBool::new(false);
static WIRE_BUTTON_SCHEDULED: AtomicBool = AtomicBool::new(false);
static BLUETOOTH_VM_HANDLE: AtomicU64 = AtomicU64::new(0);
static PENDING_SOURCE: AtomicU64 = AtomicU64::new(0);
static PENDING_VM: AtomicU64 = AtomicU64::new(0);
static PENDING_CONTAINER: AtomicU64 = AtomicU64::new(0);
static PENDING_BUTTON: AtomicU64 = AtomicU64::new(0);
static SUBCLASS_HWND: AtomicUsize = AtomicUsize::new(0);
static PROBED_GRIDS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
static INSERTED_HOSTS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
static WIRED_BUTTONS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
fn inserted_hosts() -> &'static Mutex<HashSet<u64>> {
    INSERTED_HOSTS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn probed_grids() -> &'static Mutex<HashSet<u64>> {
    PROBED_GRIDS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn wired_buttons() -> &'static Mutex<HashSet<u64>> {
    WIRED_BUTTONS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Create a complete GridViewItem from UWP XAML inside ShellHost itself.
/// On success the caller owns the returned COM reference.
unsafe fn make_xaml_tile() -> Result<ComPtr,i32> {
    let class=HString::new("Windows.UI.Xaml.Markup.XamlReader")?;
    let mut factory=null_mut();
    let hr=RoGetActivationFactory(class.0,&IID_IXAMLREADERSTATICS,&mut factory);
    if hr<0 { log(&format!("[N2] RoGetActivationFactory(XamlReader) failed: 0x{:08X}",hr as u32)); return Err(hr); }
    let factory=ComPtr(factory);
    let xaml=HString::new(TILE_XAML)?;
    let load:unsafe extern "system" fn(*mut c_void,*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(factory.as_raw(),6));
    let mut root=null_mut();
    let hr=load(factory.as_raw(),xaml.0,&mut root);
    if hr<0 { log(&format!("[N2] XamlReader.Load failed: 0x{:08X}",hr as u32)); return Err(hr); }
    log("[N2] XamlReader.Load created a native GridViewItem");
    Ok(ComPtr(root))
}

/// Find the Button by x:Name and attach its Windows.UI.Xaml.RoutedEventHandler.
/// All references/tokens are intentionally retained through ShellHost lifetime in this PoC.
unsafe fn make_carrier_data_template() -> Result<ComPtr,i32> {
    let class=HString::new("Windows.UI.Xaml.Markup.XamlReader")?;
    let mut factory=null_mut();
    let hr=RoGetActivationFactory(class.0,&IID_IXAMLREADERSTATICS,&mut factory);
    if hr<0 {
        log(&format!("[N4.1] RoGetActivationFactory(XamlReader) failed: 0x{:08X}",hr as u32));
        return Err(hr);
    }
    let factory=ComPtr(factory);
    let xaml=HString::new(CARRIER_TEMPLATE_XAML)?;
    let load:unsafe extern "system" fn(*mut c_void,*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(factory.as_raw(),6));
    let mut template=null_mut();
    let hr=load(factory.as_raw(),xaml.0,&mut template);
    if hr<0 {
        log(&format!("[N4.1] XamlReader.Load(DataTemplate) failed: 0x{:08X}",hr as u32));
        return Err(hr);
    }
    log("[N4.1] XamlReader.Load created carrier DataTemplate");
    Ok(ComPtr(template))
}

unsafe fn wire_ui_only_click(root: *mut c_void) -> Result<(),i32> {
    let mut framework=null_mut();
    let hr=query_interface(root,&IID_IFRAMEWORKELEMENT,&mut framework);
    if hr<0 { return Err(hr); }
    let framework=ComPtr(framework);
    let name=HString::new("BE200QuickSettingsClick")?;
    let find: unsafe extern "system" fn(*mut c_void,*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(framework.as_raw(),51));
    let mut button=null_mut();
    let hr=find(framework.as_raw(),name.0,&mut button);
    if hr<0 || button.is_null() { return Err(if hr<0 {hr} else {E_FAIL}); }
    let button=ComPtr(button);
    let mut base=null_mut();
    let hr=query_interface(button.as_raw(),&IID_IBUTTONBASE,&mut base);
    if hr<0 { return Err(hr); }
    let base=ComPtr(base);
    let callback=Box::into_raw(Box::new(EventHandler{vtable:&EVENT_VTBL,count:AtomicU32::new(1)}));
    let register: unsafe extern "system" fn(*mut c_void,*mut EventHandler,*mut i64)->i32=
        std::mem::transmute(slot(base.as_raw(),14));
    let mut token=0_i64;
    let hr=register(base.as_raw(),callback,&mut token);
    ev_release(callback); // After AddClick the Windows event source owns its own reference.
    if hr<0 { log(&format!("[N2] Button.Click registration failed: 0x{:08X}",hr as u32)); return Err(hr); }
    log(&format!("[N2] Button.Click registered, token={} (UI ONLY)",token));
    Ok(())
}

unsafe fn wire_button_click_handle(handle:u64)->Result<(),i32> {
    if wired_buttons().lock().map(|set|set.contains(&handle)).unwrap_or(true) {
        return Ok(());
    }
    let button=inspectable_from_handle(handle)?;
    let mut base=null_mut();
    let hr=query_interface(button.as_raw(),&IID_IBUTTONBASE,&mut base);
    if hr<0{return Err(hr);}
    let base=ComPtr(base);
    let callback=Box::into_raw(Box::new(EventHandler{vtable:&EVENT_VTBL,count:AtomicU32::new(1)}));
    let register:unsafe extern "system" fn(*mut c_void,*mut EventHandler,*mut i64)->i32=
        std::mem::transmute(slot(base.as_raw(),14));
    let mut token=0_i64;
    let hr=register(base.as_raw(),callback,&mut token);
    ev_release(callback);
    if hr<0 {
        log(&format!("[N4.1] custom Button.Click registration failed: 0x{:08X}",hr as u32));
        return Err(hr);
    }
    if let Ok(mut set)=wired_buttons().lock(){set.insert(handle);}
    log(&format!("[N4.1] custom Button.Click registered handle=0x{handle:x} token={token} (UI ONLY)"));
    Ok(())
}

unsafe fn query_collection_handle(service:*mut c_void, host:u64)->Result<(u64,u32),i32> {
    let get_index:unsafe extern "system" fn(*mut c_void,u64,*const u16,*mut u32)->i32=
        std::mem::transmute(slot(service,15));
    let get_prop:unsafe extern "system" fn(*mut c_void,u64,u32,*mut u64)->i32=
        std::mem::transmute(slot(service,16));
    let get_count:unsafe extern "system" fn(*mut c_void,u64,*mut u32)->i32=
        std::mem::transmute(slot(service,10));
    let mut last_hr=E_FAIL;
    for name in ["Children","Panel.Children","Windows.UI.Xaml.Controls.Panel.Children"] {
        let mut property=0;
        let hr=get_index(service,host,wide(name).as_ptr(),&mut property);
        if hr<0 { last_hr=hr; continue; }
        let mut collection=0_u64;
        let hr=get_prop(service,host,property,&mut collection);
        if hr<0 || collection==0 { last_hr=if hr<0 {hr} else {E_FAIL}; continue; }
        let mut count=0;
        let hr=get_count(service,collection,&mut count);
        if hr<0 { last_hr=hr; continue; }
        log(&format!("[N2] Children collection found using {name}: handle=0x{collection:x} size={count}"));
        return Ok((collection,count));
    }
    Err(last_hr)
}


// Fallback for non-DP Panel.Children: read IPanel::Children from the actual
// in-process WinRT interface, then obtain an xamlom collection handle.
unsafe fn native_panel_children(service:*mut c_void, diagnostics:*mut c_void, host:u64)
    ->Result<(u64,u32),i32>
{
    let from_handle:unsafe extern "system" fn(*mut c_void,u64,*mut *mut c_void)->i32=
        std::mem::transmute(slot(diagnostics,6));
    let mut element=null_mut();
    let hr=from_handle(diagnostics,host,&mut element);
    if hr<0 { return Err(hr); }
    let element=ComPtr(element);
    let mut panel=null_mut();
    let hr=query_interface(element.as_raw(),&IID_IPANEL,&mut panel);
    if hr<0 { return Err(hr); }
    let panel=ComPtr(panel);
    let get_children:unsafe extern "system" fn(*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(panel.as_raw(),6));
    let mut children=null_mut();
    let hr=get_children(panel.as_raw(),&mut children);
    if hr<0 { return Err(hr); }
    let children=ComPtr(children);
    let handle_for:unsafe extern "system" fn(*mut c_void,*mut c_void,*mut u64)->i32=
        std::mem::transmute(slot(diagnostics,7));
    let mut handle=0_u64;
    let hr=handle_for(diagnostics,children.as_raw(),&mut handle);
    if hr<0 || handle==0 { return Err(if hr<0 {hr} else {E_FAIL}); }
    let get_count:unsafe extern "system" fn(*mut c_void,u64,*mut u32)->i32=
        std::mem::transmute(slot(service,10));
    let mut count=0_u32;
    let hr=get_count(service,handle,&mut count);
    if hr<0 { return Err(hr); }
    log(&format!("[N2] IPanel::Children native fallback handle=0x{handle:x} size={count}"));
    Ok((handle,count))
}

unsafe fn insert_tile(host:u64) -> Result<(),i32> {
    let service=SERVICE_PTR.load(Ordering::Acquire) as *mut c_void;
    let diagnostics=DIAGNOSTICS_PTR.load(Ordering::Acquire) as *mut c_void;
    if service.is_null() || diagnostics.is_null() { return Err(E_FAIL); }

    // AddChild takes a COLLECTION handle, not the ItemsWrapGrid's element handle.
    let (collection,count)=match query_collection_handle(service,host) {
        Ok(x)=>x,
        Err(index_hr)=>{
            log(&format!("[N2] XamlOM Children property unavailable: 0x{:08X}; using IPanel fallback",index_hr as u32));
            match native_panel_children(service,diagnostics,host) {
                Ok(x)=>x,
                Err(hr)=>{log(&format!("[N2] Native Children fallback FAILED: 0x{:08X}",hr as u32));return Err(hr);}
            }
        }
    };
    // The captured Build 26200 tree has 11 native Quick Action containers.
    // Never mutate the ItemsHost while its ItemContainerGenerator is still
    // realizing those children; doing so froze ShellHost in the N2 HIL run.
    if count < EXPECTED_NATIVE_TILES { return Err(S_FALSE); }

    let item=make_xaml_tile()?;
    wire_ui_only_click(item.as_raw())?;
    let register:unsafe extern "system" fn(*mut c_void,*mut c_void,*mut u64)->i32=
        std::mem::transmute(slot(diagnostics,9));
    let mut item_handle=0_u64;
    let hr=register(diagnostics,item.as_raw(),&mut item_handle);
    if hr<0 { log(&format!("[N2] IXamlDiagnostics.RegisterInstance failed: 0x{:08X}",hr as u32)); return Err(hr); }
    let add_child:unsafe extern "system" fn(*mut c_void,u64,u64,u32)->i32=
        std::mem::transmute(slot(service,12));
    let index=2_u32.min(count);
    let hr=add_child(service,collection,item_handle,index);
    if hr<0 { log(&format!("[N2] AddChild(index={index}) failed: 0x{:08X}",hr as u32)); return Err(hr); }
    log(&format!("[N2] INSERT OK index={index} native_children_before={count} item_handle=0x{item_handle:x}"));
    Ok(())
}

fn try_insert_now(host:u64) {
    if INSERT_BUSY.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() { return; }
    struct Guard;
    impl Drop for Guard { fn drop(&mut self){INSERT_BUSY.store(false,Ordering::Release);} }
    let _guard=Guard;
    if inserted_hosts().lock().map(|set|set.contains(&host)).unwrap_or(true) {return;}
    let result=std::panic::catch_unwind(|| unsafe { insert_tile(host) });
    match result {
        Ok(Ok(()))=>{if let Ok(mut set)=inserted_hosts().lock(){set.insert(host);}},
        Ok(Err(S_FALSE))=>log(&format!("[N2.2] deferred insert waiting for {EXPECTED_NATIVE_TILES} native children")),
        Ok(Err(hr))=>log(&format!("[N2.2] INSERT FAILED: 0x{:08X} (host=0x{host:x})",hr as u32)),
        Err(_)=>log("[N2.2] panic caught while inserting (ShellHost left unchanged if AddChild not reached)"),
    }
}

unsafe extern "system" fn qs_subclass_proc(hwnd:Hwnd,msg:u32,wparam:usize,lparam:isize,
                                             _id:usize,_ref_data:usize)->isize {
    if msg==WM_BE200_QS_DUPLICATE_VM {
        let source=PENDING_SOURCE.swap(0,Ordering::AcqRel);
        let vm=PENDING_VM.swap(0,Ordering::AcqRel);
        if source!=0 && vm!=0 {
            let service=SERVICE_PTR.load(Ordering::Acquire) as *mut c_void;
            if !service.is_null() {
                let get_count:unsafe extern "system" fn(*mut c_void,u64,*mut u32)->i32=
                    std::mem::transmute(slot(service,10));
                let add_child:unsafe extern "system" fn(*mut c_void,u64,u64,u32)->i32=
                    std::mem::transmute(slot(service,12));
                let mut before=0u32;
                let hr_count=get_count(service,source,&mut before);
                if hr_count<0 {
                    log(&format!("[N4] duplicate VM count failed: 0x{:08X}",hr_count as u32));
                } else {
                    let index=2u32.min(before);
                    let hr=add_child(service,source,vm,index);
                    if hr>=0 {
                        let mut after=0u32;
                        let hr_after=get_count(service,source,&mut after);
                        DUPLICATE_VM_DONE.store(true,Ordering::Release);
                        log(&format!("[N4] DATASOURCE INSERT OK source=0x{source:x} vm=0x{vm:x} index={index} count_before={before} count_after={} count_hr=0x{:08X}",
                            if hr_after>=0{after}else{0},hr_after as u32));
                    } else {
                        log(&format!("[N4] DATASOURCE INSERT FAILED source=0x{source:x} vm=0x{vm:x} index={index} hr=0x{:08X}",hr as u32));
                    }
                }
            }
        }
        DUPLICATE_VM_SCHEDULED.store(false,Ordering::Release);
        return 0;
    }
    if msg==WM_BE200_QS_PROBE {
        let grid=PENDING_GRID.swap(0,Ordering::AcqRel);
        if grid!=0 {
            log(&format!("[N3] deferred UI-thread QuickActionViewModel probe grid=0x{grid:x}"));
            match std::panic::catch_unwind(|| unsafe { probe_quick_action_items(grid) }) {
                Ok(Ok(())) => {
                    if let Ok(mut set)=probed_grids().lock(){ set.insert(grid); }
                }
                Ok(Err(S_FALSE)) => log("[N3] ItemsSource not fully ready; waiting for later callback"),
                Ok(Err(hr)) => log(&format!("[N3] VM PROBE FAILED: HRESULT=0x{:08X}",hr as u32)),
                Err(_) => log("[N3] panic trapped during read-only VM probe"),
            }
        }
        PROBE_SCHEDULED.store(false,Ordering::Release);
        return 0;
    }
    DefSubclassProc(hwnd,msg,wparam,lparam)
}

fn schedule_vm_probe(grid:u64) {
    if probed_grids().lock().map(|set|set.contains(&grid)).unwrap_or(true) { return; }
    if PROBE_SCHEDULED.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() {
        return;
    }
    let ok=unsafe {
        let class=wide("ControlCenterWindow");
        let hwnd=FindWindowW(class.as_ptr(),null_mut());
        if hwnd.is_null() {
            log("[N3] ControlCenterWindow not found while scheduling VM probe");
            false
        } else {
            let window_thread=GetWindowThreadProcessId(hwnd,null_mut());
            let current_thread=GetCurrentThreadId();
            if window_thread!=current_thread {
                log(&format!("[N3] refusing cross-thread subclass: callback_tid={current_thread} window_tid={window_thread}"));
                false
            } else {
                let previous=SUBCLASS_HWND.load(Ordering::Acquire);
                if previous != hwnd as usize {
                    if SetWindowSubclass(hwnd,qs_subclass_proc,QS_SUBCLASS_ID,0)==0 {
                        log("[N3] SetWindowSubclass failed");
                        false
                    } else {
                        SUBCLASS_HWND.store(hwnd as usize,Ordering::Release);
                        log(&format!("[N3] ControlCenterWindow subclass installed tid={current_thread}"));
                        true
                    }
                } else { true }
            }
        }
    };
    if !ok {
        PROBE_SCHEDULED.store(false,Ordering::Release);
        return;
    }
    PENDING_GRID.store(grid,Ordering::Release);
    let hwnd=SUBCLASS_HWND.load(Ordering::Acquire) as Hwnd;
    let posted=unsafe{PostMessageW(hwnd,WM_BE200_QS_PROBE,0,0)};
    if posted==0 {
        PENDING_GRID.store(0,Ordering::Release);
        PROBE_SCHEDULED.store(false,Ordering::Release);
        log("[N3] PostMessageW(VM probe) failed");
    } else {
        log(&format!("[N3] read-only VM probe queued grid=0x{grid:x}"));
    }
}


fn schedule_duplicate_native_vm(source:u64,vm:u64) {
    if !DUPLICATE_NATIVE_VM_MODE.load(Ordering::Acquire) ||
       DUPLICATE_VM_DONE.load(Ordering::Acquire) {
        return;
    }
    if DUPLICATE_VM_SCHEDULED.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() {
        return;
    }
    let hwnd=SUBCLASS_HWND.load(Ordering::Acquire) as Hwnd;
    if hwnd.is_null() {
        DUPLICATE_VM_SCHEDULED.store(false,Ordering::Release);
        log("[N4] duplicate VM scheduling failed: ControlCenterWindow subclass is unavailable");
        return;
    }
    PENDING_SOURCE.store(source,Ordering::Release);
    PENDING_VM.store(vm,Ordering::Release);
    let posted=unsafe{PostMessageW(hwnd,WM_BE200_QS_DUPLICATE_VM,0,0)};
    if posted==0 {
        PENDING_SOURCE.store(0,Ordering::Release);
        PENDING_VM.store(0,Ordering::Release);
        DUPLICATE_VM_SCHEDULED.store(false,Ordering::Release);
        log("[N4] PostMessageW(duplicate native VM) failed");
    } else {
        log(&format!("[N4] queued data-source duplicate source=0x{source:x} bluetooth_vm=0x{vm:x}"));
    }
}

#[repr(C)]
struct CollectionElementValue {
    index: u32,
    value_type: *mut u16,
    value: *mut u16,
    metadata_bits: i64,
}

#[repr(C)]
struct PropertyChainSource {
    handle: u64,
    target_type: *mut u16,
    name: *mut u16,
    source: i32,
    source_info: SourceInfo,
}

#[repr(C)]
struct PropertyChainValue {
    index: u32,
    ty: *mut u16,
    declaring_type: *mut u16,
    value_type: *mut u16,
    item_type: *mut u16,
    value: *mut u16,
    overridden: i32,
    metadata_bits: i64,
    property_name: *mut u16,
    property_chain_index: u32,
}

const META_IS_VALUE_HANDLE: i64 = 0x1;
const META_IS_PROPERTY_READ_ONLY: i64 = 0x2;
const META_IS_VALUE_COLLECTION: i64 = 0x4;
const META_IS_VALUE_HANDLE_AND_EVALUATED_VALUE: i64 = 0x40;

fn metadata_has_value_handle(bits:i64)->bool {
    bits & (META_IS_VALUE_HANDLE | META_IS_VALUE_HANDLE_AND_EVALUATED_VALUE) != 0
}

fn vm_log(message:&str) {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("BE200AudioHub").join("logs");
    if create_dir_all(&base).is_ok() {
        if let Ok(mut file)=OpenOptions::new().create(true).append(true)
            .open(base.join("qs-vm-dump.log")) {
            let _=writeln!(file,"[pid={}] {message}",std::process::id());
        }
    }
}

fn parse_instance_handle(value:&str)->Option<u64> {
    let text=value.trim();
    if let Some(pos)=text.find("0x") {
        let digits:String=text[pos+2..].chars().take_while(|c|c.is_ascii_hexdigit()).collect();
        if !digits.is_empty() { return u64::from_str_radix(&digits,16).ok(); }
    }
    let digits:String=text.chars().skip_while(|c|!c.is_ascii_digit())
        .take_while(|c|c.is_ascii_digit()).collect();
    if digits.is_empty(){None}else{digits.parse().ok()}
}

fn guid_text(g:&Guid)->String {
    format!("{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.d1,g.d2,g.d3,g.d4[0],g.d4[1],g.d4[2],g.d4[3],g.d4[4],g.d4[5],g.d4[6],g.d4[7])
}

unsafe fn runtime_class_and_iids(handle:u64)->Result<(String,Vec<Guid>),i32> {
    let diagnostics=DIAGNOSTICS_PTR.load(Ordering::Acquire) as *mut c_void;
    if diagnostics.is_null(){return Err(E_FAIL);}
    let from_handle:unsafe extern "system" fn(*mut c_void,u64,*mut *mut c_void)->i32=
        std::mem::transmute(slot(diagnostics,6));
    let mut inspectable=null_mut();
    let hr=from_handle(diagnostics,handle,&mut inspectable);
    if hr<0||inspectable.is_null(){return Err(if hr<0{hr}else{E_FAIL});}
    let inspectable=ComPtr(inspectable);

    let get_class:unsafe extern "system" fn(*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(inspectable.as_raw(),4));
    let mut raw_name=null_mut();
    let hr=get_class(inspectable.as_raw(),&mut raw_name);
    let class_name=if hr>=0 && !raw_name.is_null() {
        let h=HString(raw_name);
        let mut len=0u32;
        let ptr=WindowsGetStringRawBuffer(h.0,&mut len);
        if ptr.is_null(){String::new()}else{String::from_utf16_lossy(std::slice::from_raw_parts(ptr,len as usize))}
    } else { String::new() };

    let get_iids:unsafe extern "system" fn(*mut c_void,*mut u32,*mut *mut Guid)->i32=
        std::mem::transmute(slot(inspectable.as_raw(),3));
    let mut count=0u32;
    let mut ptr=null_mut();
    let hr_iids=get_iids(inspectable.as_raw(),&mut count,&mut ptr);
    let iids=if hr_iids>=0 && !ptr.is_null() {
        let out=std::slice::from_raw_parts(ptr,count as usize).to_vec();
        CoTaskMemFree(ptr.cast());
        out
    } else { Vec::new() };
    Ok((class_name,iids))
}


#[repr(C)]
struct TypeName {
    name: *mut c_void,
    kind: i32,
}

unsafe fn take_hstring(raw:*mut c_void)->String {
    if raw.is_null(){return String::new();}
    let h=HString(raw);
    let mut len=0u32;
    let ptr=WindowsGetStringRawBuffer(h.0,&mut len);
    if ptr.is_null(){String::new()}else{String::from_utf16_lossy(std::slice::from_raw_parts(ptr,len as usize))}
}

unsafe fn inspectable_from_handle(handle:u64)->Result<ComPtr,i32> {
    let diagnostics=DIAGNOSTICS_PTR.load(Ordering::Acquire) as *mut c_void;
    if diagnostics.is_null(){return Err(E_FAIL);}
    let from_handle:unsafe extern "system" fn(*mut c_void,u64,*mut *mut c_void)->i32=
        std::mem::transmute(slot(diagnostics,6));
    let mut inspectable=null_mut();
    let hr=from_handle(diagnostics,handle,&mut inspectable);
    if hr<0||inspectable.is_null(){Err(if hr<0{hr}else{E_FAIL})}else{Ok(ComPtr(inspectable))}
}

unsafe fn inspectable_runtime_class(ptr:*mut c_void)->String {
    if ptr.is_null(){return String::new();}
    let get_class:unsafe extern "system" fn(*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(ptr,4));
    let mut raw=null_mut();
    let hr=get_class(ptr,&mut raw);
    if hr<0||raw.is_null(){String::new()}else{take_hstring(raw)}
}

unsafe fn boxed_value_summary(value:*mut c_void)->String {
    if value.is_null(){return "<null>".into();}
    let mut property_value=null_mut();
    let hr=query_interface(value,&IID_IPROPERTYVALUE,&mut property_value);
    if hr<0||property_value.is_null() {
        return format!("inspectable(class={:?})",inspectable_runtime_class(value));
    }
    let property_value=ComPtr(property_value);
    let get_type:unsafe extern "system" fn(*mut c_void,*mut i32)->i32=
        std::mem::transmute(slot(property_value.as_raw(),6));
    let mut ty=0i32;
    let hr=get_type(property_value.as_raw(),&mut ty);
    if hr<0{return format!("IPropertyValue(type HRESULT=0x{:08X})",hr as u32);}
    match ty {
        1=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut u8)->i32=std::mem::transmute(slot(property_value.as_raw(),8));
            let mut v=0u8; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("UInt8({v})")}else{format!("UInt8(hr=0x{:08X})",hr as u32)}
        }
        2=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut i16)->i32=std::mem::transmute(slot(property_value.as_raw(),9));
            let mut v=0i16; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Int16({v})")}else{format!("Int16(hr=0x{:08X})",hr as u32)}
        }
        3=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut u16)->i32=std::mem::transmute(slot(property_value.as_raw(),10));
            let mut v=0u16; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("UInt16({v})")}else{format!("UInt16(hr=0x{:08X})",hr as u32)}
        }
        4=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut i32)->i32=std::mem::transmute(slot(property_value.as_raw(),11));
            let mut v=0i32; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Int32({v})")}else{format!("Int32(hr=0x{:08X})",hr as u32)}
        }
        5=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut u32)->i32=std::mem::transmute(slot(property_value.as_raw(),12));
            let mut v=0u32; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("UInt32({v})")}else{format!("UInt32(hr=0x{:08X})",hr as u32)}
        }
        6=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut i64)->i32=std::mem::transmute(slot(property_value.as_raw(),13));
            let mut v=0i64; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Int64({v})")}else{format!("Int64(hr=0x{:08X})",hr as u32)}
        }
        7=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut u64)->i32=std::mem::transmute(slot(property_value.as_raw(),14));
            let mut v=0u64; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("UInt64({v})")}else{format!("UInt64(hr=0x{:08X})",hr as u32)}
        }
        8=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut f32)->i32=std::mem::transmute(slot(property_value.as_raw(),15));
            let mut v=0f32; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Single({v})")}else{format!("Single(hr=0x{:08X})",hr as u32)}
        }
        9=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut f64)->i32=std::mem::transmute(slot(property_value.as_raw(),16));
            let mut v=0f64; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Double({v})")}else{format!("Double(hr=0x{:08X})",hr as u32)}
        }
        10=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut u16)->i32=std::mem::transmute(slot(property_value.as_raw(),17));
            let mut v=0u16; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Char16({:?})",char::from_u32(v as u32))}else{format!("Char16(hr=0x{:08X})",hr as u32)}
        }
        11=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut u8)->i32=std::mem::transmute(slot(property_value.as_raw(),18));
            let mut v=0u8; let hr=f(property_value.as_raw(),&mut v);
            if hr>=0{format!("Boolean({})",v!=0)}else{format!("Boolean(hr=0x{:08X})",hr as u32)}
        }
        12=>{
            let f:unsafe extern "system" fn(*mut c_void,*mut *mut c_void)->i32=std::mem::transmute(slot(property_value.as_raw(),19));
            let mut raw=null_mut(); let hr=f(property_value.as_raw(),&mut raw);
            if hr>=0{format!("String({:?})",take_hstring(raw))}else{format!("String(hr=0x{:08X})",hr as u32)}
        }
        _=>format!("IPropertyValue(type={ty})"),
    }
}

unsafe fn probe_custom_property_provider(index:u32,handle:u64) {
    let inspectable=match inspectable_from_handle(handle) {
        Ok(x)=>x,
        Err(hr)=>{vm_log(&format!("item[{index}] IInspectable lookup failed hr=0x{:08X}",hr as u32));return;}
    };
    let mut provider=null_mut();
    let hr=query_interface(inspectable.as_raw(),&IID_ICUSTOMPROPERTYPROVIDER,&mut provider);
    if hr<0||provider.is_null() {
        vm_log(&format!("item[{index}] ICustomPropertyProvider unavailable hr=0x{:08X}",hr as u32));
        return;
    }
    let provider=ComPtr(provider);

    let get_string:unsafe extern "system" fn(*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(provider.as_raw(),8));
    let mut repr=null_mut();
    let hr_repr=get_string(provider.as_raw(),&mut repr);
    let repr_text=if hr_repr>=0{take_hstring(repr)}else{format!("<hr=0x{:08X}>",hr_repr as u32)};

    let get_type:unsafe extern "system" fn(*mut c_void,*mut TypeName)->i32=
        std::mem::transmute(slot(provider.as_raw(),9));
    let mut type_name=TypeName{name:null_mut(),kind:0};
    let hr_type=get_type(provider.as_raw(),&mut type_name);
    let type_text=if hr_type>=0{take_hstring(type_name.name)}else{format!("<hr=0x{:08X}>",hr_type as u32)};
    vm_log(&format!("item[{index}] ICustomPropertyProvider repr={repr_text:?} type={type_text:?} kind={}",type_name.kind));

    const CANDIDATES:&[&str]=&[
        "Name","DisplayName","Title","Label","Text","Description","Subtitle","Status",
        "AutomationName","AutomationId","QuickActionId","QuickActionID","Id","Identifier",
        "Icon","IconSource","FallbackIconSource","Glyph","IconGlyph","IconId","IconName",
        "State","ToggleState","IsEnabled","IsChecked","IsOn","IsActive","IsAvailable",
        "IsVisible","Visibility","IsSplitButton","HasSecondaryAction","CanShowDetails",
        "PrimaryAction","SecondaryAction","Action","Command","ToggleCommand","SettingsCommand",
        "QuickAction","QuickActionName","QuickActionType","Source","Uri","SettingsUri","LaunchUri",
        "ToolTip","Tooltip","PositionInSet","SizeOfSet"
    ];
    let get_property:unsafe extern "system" fn(*mut c_void,*mut c_void,*mut *mut c_void)->i32=
        std::mem::transmute(slot(provider.as_raw(),6));

    for candidate in CANDIDATES {
        let name=match HString::new(candidate){Ok(x)=>x,Err(_)=>continue};
        let mut prop=null_mut();
        let hr=get_property(provider.as_raw(),name.0,&mut prop);
        if hr<0||prop.is_null(){continue;}
        let prop=ComPtr(prop);

        let get_prop_name:unsafe extern "system" fn(*mut c_void,*mut *mut c_void)->i32=
            std::mem::transmute(slot(prop.as_raw(),7));
        let mut raw_name=null_mut();
        let hr_name=get_prop_name(prop.as_raw(),&mut raw_name);
        let prop_name=if hr_name>=0{take_hstring(raw_name)}else{candidate.to_string()};

        let get_prop_type:unsafe extern "system" fn(*mut c_void,*mut TypeName)->i32=
            std::mem::transmute(slot(prop.as_raw(),6));
        let mut prop_type=TypeName{name:null_mut(),kind:0};
        let hr_pt=get_prop_type(prop.as_raw(),&mut prop_type);
        let prop_type_text=if hr_pt>=0{take_hstring(prop_type.name)}else{format!("<hr=0x{:08X}>",hr_pt as u32)};

        let can_write_fn:unsafe extern "system" fn(*mut c_void,*mut u8)->i32=
            std::mem::transmute(slot(prop.as_raw(),12));
        let can_read_fn:unsafe extern "system" fn(*mut c_void,*mut u8)->i32=
            std::mem::transmute(slot(prop.as_raw(),13));
        let mut can_write=0u8; let _=can_write_fn(prop.as_raw(),&mut can_write);
        let mut can_read=0u8; let _=can_read_fn(prop.as_raw(),&mut can_read);

        let get_value:unsafe extern "system" fn(*mut c_void,*mut c_void,*mut *mut c_void)->i32=
            std::mem::transmute(slot(prop.as_raw(),8));
        let mut value=null_mut();
        let hr_value=get_value(prop.as_raw(),inspectable.as_raw(),&mut value);
        let value_summary=if hr_value>=0&&!value.is_null() {
            let value=ComPtr(value);
            boxed_value_summary(value.as_raw())
        } else if hr_value>=0 {"<null>".into()} else {format!("<hr=0x{:08X}>",hr_value as u32)};

        vm_log(&format!("  custom-prop candidate={candidate:?} name={prop_name:?} type={prop_type_text:?} kind={} can_read={} can_write={} value={value_summary}",
            prop_type.kind,can_read!=0,can_write!=0));
    }
}


unsafe fn probe_runtime_metadata() {
    let module=LoadLibraryW(wide("WinTypes.dll").as_ptr());
    if module.is_null() {
        vm_log("[N3.2] LoadLibraryW(WinTypes.dll) failed; metadata path probe skipped");
        return;
    }
    let proc=GetProcAddress(module,b"RoGetMetaDataFile\0".as_ptr());
    if proc.is_null() {
        vm_log("[N3.2] WinTypes.dll has no RoGetMetaDataFile export");
        FreeLibrary(module);
        return;
    }
    let resolve:unsafe extern "system" fn(
        *mut c_void,*mut c_void,*mut *mut c_void,*mut *mut c_void,*mut u32
    )->i32=std::mem::transmute(proc);
    let class=match HString::new("ControlCenter.QuickActionViewModel") {
        Ok(x)=>x,
        Err(hr)=>{
            vm_log(&format!("[N3.2] metadata HSTRING failed hr=0x{:08X}",hr as u32));
            FreeLibrary(module);
            return;
        }
    };
    let mut path=null_mut();
    let mut metadata_import=null_mut();
    let mut type_def=0u32;
    let hr=resolve(class.0,null_mut(),&mut path,&mut metadata_import,&mut type_def);
    let path_text=if !path.is_null(){take_hstring(path)}else{String::new()};
    if !metadata_import.is_null(){com_release(metadata_import);}
    FreeLibrary(module);
    vm_log(&format!("[N3.2] RoGetMetaDataFile(ControlCenter.QuickActionViewModel) hr=0x{:08X} type_def=0x{type_def:08X} path={path_text:?}",hr as u32));
}

unsafe fn probe_activation_factory() {
    let class=match HString::new("ControlCenter.QuickActionViewModel"){Ok(x)=>x,Err(hr)=>{
        vm_log(&format!("[N3.2] QuickActionViewModel HSTRING failed hr=0x{:08X}",hr as u32));return;
    }};
    let mut factory=null_mut();
    let hr=RoGetActivationFactory(class.0,&IID_IACTIVATIONFACTORY,&mut factory);
    if hr>=0&&!factory.is_null() {
        vm_log("[N3.2] RoGetActivationFactory(ControlCenter.QuickActionViewModel): SUCCESS (class exposes IActivationFactory)");
        com_release(factory);
    } else {
        vm_log(&format!("[N3.2] RoGetActivationFactory(ControlCenter.QuickActionViewModel) failed hr=0x{:08X}",hr as u32));
    }
}

unsafe fn free_source_array(ptr:*mut PropertyChainSource,count:u32) {
    if ptr.is_null(){return;}
    for item in std::slice::from_raw_parts_mut(ptr,count as usize) {
        if !item.target_type.is_null(){SysFreeString(item.target_type);}
        if !item.name.is_null(){SysFreeString(item.name);}
        if !item.source_info.filename.is_null(){SysFreeString(item.source_info.filename as *mut u16);}
        if !item.source_info.hash.is_null(){SysFreeString(item.source_info.hash as *mut u16);}
    }
    CoTaskMemFree(ptr.cast());
}
unsafe fn free_property_array(ptr:*mut PropertyChainValue,count:u32) {
    if ptr.is_null(){return;}
    for item in std::slice::from_raw_parts_mut(ptr,count as usize) {
        for b in [item.ty,item.declaring_type,item.value_type,item.item_type,item.value,item.property_name] {
            if !b.is_null(){SysFreeString(b);}
        }
    }
    CoTaskMemFree(ptr.cast());
}

unsafe fn dump_vm_properties(service:*mut c_void,index:u32,handle:u64)->Result<i32,i32> {
    let get_chain:unsafe extern "system" fn(*mut c_void,u64,*mut u32,*mut *mut PropertyChainSource,
                                            *mut u32,*mut *mut PropertyChainValue)->i32=
        std::mem::transmute(slot(service,7));
    let mut source_count=0u32;
    let mut sources=null_mut();
    let mut property_count=0u32;
    let mut properties=null_mut();
    let hr=get_chain(service,handle,&mut source_count,&mut sources,&mut property_count,&mut properties);
    if hr<0 {
        vm_log(&format!("item[{index}] handle=0x{handle:x} GetPropertyValuesChain FAILED hr=0x{:08X}",hr as u32));
        return Err(hr);
    }

    let mut bluetooth_score=0i32;
    vm_log(&format!("---- item[{index}] handle=0x{handle:x} properties={property_count} ----"));
    if !properties.is_null() {
        for p in std::slice::from_raw_parts(properties,property_count as usize) {
            let name=bstr_string(p.property_name);
            let value=bstr_string(p.value);
            let value_type=bstr_string(p.value_type);
            let declaring=bstr_string(p.declaring_type);
            let lower=format!("{name}\n{value}\n{value_type}\n{declaring}").to_ascii_lowercase();
            if lower.contains("microsoft.quickaction.bluetooth"){bluetooth_score+=100;}
            if lower.contains("qa_bluetooth"){bluetooth_score+=50;}
            if lower.contains("bluetooth"){bluetooth_score+=10;}
            let ro=if p.metadata_bits & META_IS_PROPERTY_READ_ONLY !=0 {"RO"} else {"RW?"};
            let handle_flag=if metadata_has_value_handle(p.metadata_bits) {"H"} else {"-"};
            let coll=if p.metadata_bits & META_IS_VALUE_COLLECTION !=0 {"C"} else {"-"};
            vm_log(&format!("  prop idx={} name={:?} value={:?} value_type={:?} declaring={:?} meta=0x{:x} [{ro} {handle_flag}{coll}]",
                p.index,name,value,value_type,declaring,p.metadata_bits));
        }
    }
    free_source_array(sources,source_count);
    free_property_array(properties,property_count);
    Ok(bluetooth_score)
}

unsafe fn property_index_from_chain(service:*mut c_void, object:u64, wanted:&str)
    ->Result<(u32,Option<u64>),i32>
{
    let get_chain:unsafe extern "system" fn(*mut c_void,u64,*mut u32,*mut *mut PropertyChainSource,
                                            *mut u32,*mut *mut PropertyChainValue)->i32=
        std::mem::transmute(slot(service,7));
    let mut source_count=0u32;
    let mut sources=null_mut();
    let mut property_count=0u32;
    let mut properties=null_mut();
    let hr=get_chain(service,object,&mut source_count,&mut sources,&mut property_count,&mut properties);
    if hr<0 {
        if !sources.is_null(){free_source_array(sources,source_count);}
        if !properties.is_null(){free_property_array(properties,property_count);}
        return Err(hr);
    }

    let mut found=None;
    if !properties.is_null() {
        for p in std::slice::from_raw_parts(properties,property_count as usize) {
            let name=bstr_string(p.property_name);
            let declaring=bstr_string(p.declaring_type);
            let matches=name.eq_ignore_ascii_case(wanted)
                || name.rsplit('.').next().map_or(false,|tail|tail.eq_ignore_ascii_case(wanted));
            if matches {
                let value=bstr_string(p.value);
                let parsed=if metadata_has_value_handle(p.metadata_bits) {
                    parse_instance_handle(&value)
                } else { None };
                vm_log(&format!("property-chain fallback object=0x{object:x} wanted={wanted:?} idx={} name={name:?} declaring={declaring:?} meta=0x{:x} parsed_handle={:?}",
                    p.index,p.metadata_bits,parsed.map(|h|format!("0x{h:x}"))));
                found=Some((p.index,parsed));
                break;
            }
        }
    }
    free_source_array(sources,source_count);
    free_property_array(properties,property_count);
    found.ok_or(E_FAIL)
}

unsafe fn property_handle(service:*mut c_void, object:u64, property_name:&str)->Result<u64,i32> {
    let get_index:unsafe extern "system" fn(*mut c_void,u64,*const u16,*mut u32)->i32=
        std::mem::transmute(slot(service,15));
    let get_prop:unsafe extern "system" fn(*mut c_void,u64,u32,*mut u64)->i32=
        std::mem::transmute(slot(service,16));

    // Build 26200 can reject the short inherited-property name with E_INVALIDARG.
    // Try owner-qualified spellings first, then discover the exact property index
    // from GetPropertyValuesChain instead of assuming one XamlOM naming convention.
    let candidates:Vec<String>=match property_name {
        "ItemsSource"=>vec![
            "ItemsSource".into(),
            "ItemsControl.ItemsSource".into(),
            "Windows.UI.Xaml.Controls.ItemsControl.ItemsSource".into(),
        ],
        "Content"=>vec![
            "Content".into(),
            "ContentControl.Content".into(),
            "Windows.UI.Xaml.Controls.ContentControl.Content".into(),
        ],
        _=>vec![property_name.into()],
    };

    let mut last_hr=E_FAIL;
    for candidate in candidates {
        let mut property=0u32;
        let hr=get_index(service,object,wide(&candidate).as_ptr(),&mut property);
        if hr<0 {
            last_hr=hr;
            continue;
        }
        let mut value=0u64;
        let hr=get_prop(service,object,property,&mut value);
        if hr>=0 && value!=0 {
            vm_log(&format!("property lookup object=0x{object:x} requested={property_name:?} resolved={candidate:?} idx={property} handle=0x{value:x}"));
            return Ok(value);
        }
        last_hr=if hr<0{hr}else{E_FAIL};
    }

    match property_index_from_chain(service,object,property_name) {
        Ok((property,parsed))=>{
            let mut value=0u64;
            let hr=get_prop(service,object,property,&mut value);
            if hr>=0 && value!=0 {
                vm_log(&format!("property lookup object=0x{object:x} requested={property_name:?} resolved=property-chain idx={property} handle=0x{value:x}"));
                Ok(value)
            } else if let Some(handle)=parsed {
                vm_log(&format!("property lookup object=0x{object:x} requested={property_name:?} using parsed property-chain handle=0x{handle:x}; GetProperty hr=0x{:08X}",hr as u32));
                Ok(handle)
            } else {
                Err(if hr<0{hr}else{last_hr})
            }
        }
        Err(chain_hr)=>{
            vm_log(&format!("property lookup FAILED object=0x{object:x} requested={property_name:?} direct_hr=0x{:08X} chain_hr=0x{:08X}",
                last_hr as u32,chain_hr as u32));
            Err(if chain_hr<0{chain_hr}else{last_hr})
        }
    }
}


unsafe fn xaml_property_text(service:*mut c_void, object:u64, wanted:&str)->Option<String> {
    let get_chain:unsafe extern "system" fn(*mut c_void,u64,*mut u32,*mut *mut PropertyChainSource,
                                            *mut u32,*mut *mut PropertyChainValue)->i32=
        std::mem::transmute(slot(service,7));
    let mut source_count=0u32;
    let mut sources=null_mut();
    let mut property_count=0u32;
    let mut properties=null_mut();
    let hr=get_chain(service,object,&mut source_count,&mut sources,&mut property_count,&mut properties);
    if hr<0{return None;}
    let mut result=None;
    if !properties.is_null() {
        for p in std::slice::from_raw_parts(properties,property_count as usize) {
            let name=bstr_string(p.property_name);
            if name.eq_ignore_ascii_case(wanted) ||
               name.rsplit('.').next().map_or(false,|tail|tail.eq_ignore_ascii_case(wanted)) {
                result=Some(bstr_string(p.value));
                break;
            }
        }
    }
    free_source_array(sources,source_count);
    free_property_array(properties,property_count);
    result
}

unsafe fn visual_quick_action_identities(service:*mut c_void,grid:u64)->HashMap<u32,String> {
    let buttons:Vec<(u32,u64)>=tree().lock().map(|nodes| {
        let mut out=Vec::new();
        for (&handle,node) in nodes.iter() {
            if !node.ty.ends_with("PaginatedToggleButton") || node.name!="ToggleButton" {continue;}
            let mut ancestor=handle;
            let mut item=None;
            for _ in 0..32 {
                let Some(n)=nodes.get(&ancestor) else{break;};
                if n.ty.ends_with("GridViewItem") {
                    item=Some((n.index,ancestor));
                    break;
                }
                if n.parent==0||n.parent==ancestor{break;}
                ancestor=n.parent;
            }
            if let Some((index,item_handle))=item {
                if quick_actions_grid_handle(&nodes,item_handle)==Some(grid) {
                    out.push((index,handle));
                }
            }
        }
        out.sort_unstable_by_key(|(index,_)|*index);
        out
    }).unwrap_or_default();

    let mut map=HashMap::new();
    for (index,button) in buttons {
        let automation_id=xaml_property_text(service,button,"AutomationId")
            .or_else(||xaml_property_text(service,button,"AutomationProperties.AutomationId"))
            .unwrap_or_default();
        let automation_name=xaml_property_text(service,button,"AutomationProperties.Name")
            .unwrap_or_default();
        vm_log(&format!("visual item[{index}] button=0x{button:x} AutomationId={automation_id:?} AutomationName={automation_name:?}"));
        if !automation_id.is_empty(){map.insert(index,automation_id);}
    }
    map
}

// GetCollectionElements can return only an evaluated string for a handle-backed
// object on some XamlOM surfaces. The realized GridViewItem.Content property is
// an independent, read-only route to the exact QuickActionViewModel handle.
unsafe fn realized_vm_handles_for_grid(service:*mut c_void, grid:u64)->HashMap<u32,u64> {
    let containers:Vec<(u32,u64)>=tree().lock().map(|nodes| {
        let mut wraps=HashSet::new();
        for (&handle,node) in nodes.iter() {
            if node.ty.ends_with("ItemsWrapGrid") &&
               quick_actions_grid_handle(&nodes,handle)==Some(grid) {
                wraps.insert(handle);
            }
        }
        let mut out:Vec<(u32,u64)>=nodes.iter()
            .filter_map(|(&handle,node)| {
                if wraps.contains(&node.parent) && node.ty.ends_with("GridViewItem") {
                    Some((node.index,handle))
                } else { None }
            })
            .collect();
        out.sort_unstable_by_key(|(index,_)|*index);
        out
    }).unwrap_or_default();

    let mut result=HashMap::new();
    for (index,container) in containers {
        match property_handle(service,container,"Content") {
            Ok(handle)=>{
                vm_log(&format!("item[{index}] realized GridViewItem.Content fallback: container=0x{container:x} vm=0x{handle:x}"));
                result.insert(index,handle);
            }
            Err(hr)=>vm_log(&format!("item[{index}] GridViewItem.Content fallback unavailable: container=0x{container:x} hr=0x{:08X}",hr as u32)),
        }
    }
    result
}

unsafe fn probe_quick_action_items(grid:u64)->Result<(),i32> {
    let service=SERVICE_PTR.load(Ordering::Acquire) as *mut c_void;
    if service.is_null(){return Err(E_FAIL);}
    let get_count:unsafe extern "system" fn(*mut c_void,u64,*mut u32)->i32=
        std::mem::transmute(slot(service,10));
    let get_elements:unsafe extern "system" fn(*mut c_void,u64,u32,*mut u32,*mut *mut CollectionElementValue)->i32=
        std::mem::transmute(slot(service,11));

    let source=match property_handle(service,grid,"ItemsSource") {
        Ok(handle)=>handle,
        Err(hr)=>{
            log(&format!("[N3.1] ItemsSource property resolution failed: 0x{:08X}. See qs-vm-dump.log",hr as u32));
            return Err(hr);
        }
    };
    let mut count=0u32;
    let hr=get_count(service,source,&mut count);
    if hr<0{return Err(hr);}
    log(&format!("[N3] RootGridView.ItemsSource handle=0x{source:x} count={count}"));
    if count==0{return Err(S_FALSE);}
    probe_activation_factory();
    probe_runtime_metadata();
    let visual_identities=visual_quick_action_identities(service,grid);
    let realized_vm_handles=realized_vm_handles_for_grid(service,grid);
    log(&format!("[N3] realized container Content handles available={}",realized_vm_handles.len()));

    let mut requested=count;
    let mut elements=null_mut();
    let hr=get_elements(service,source,0,&mut requested,&mut elements);
    if hr<0||elements.is_null(){return Err(if hr<0{hr}else{E_FAIL});}
    vm_log(&format!("==== QuickActionViewModel snapshot pid={} grid=0x{grid:x} source=0x{source:x} count={requested} ====",
        std::process::id()));

    let mut best:Option<(u32,u64,i32)>=None;
    for e in std::slice::from_raw_parts(elements,requested as usize) {
        let value_type=bstr_string(e.value_type);
        let value=bstr_string(e.value);
        let parsed_handle=if metadata_has_value_handle(e.metadata_bits) {
            parse_instance_handle(&value)
        } else { None };
        let fallback_handle=realized_vm_handles.get(&e.index).copied();
        let handle=parsed_handle.or(fallback_handle);
        let handle_source=if parsed_handle.is_some() {"collection"}
            else if fallback_handle.is_some() {"GridViewItem.Content"} else {"none"};
        vm_log(&format!("item[{}] type={:?} value={:?} meta=0x{:x} handle={:?} source={handle_source}",
            e.index,value_type,value,e.metadata_bits,handle.map(|h|format!("0x{h:x}"))));
        if let Some(handle)=handle {
            if let Ok((class_name,iids))=runtime_class_and_iids(handle) {
                vm_log(&format!("  runtime_class={:?} iids=[{}]",class_name,
                    iids.iter().map(guid_text).collect::<Vec<_>>().join(",")));
            }
            let collection_text=format!("{value_type}\n{value}").to_ascii_lowercase();
            let visual_id=visual_identities.get(&e.index).cloned().unwrap_or_default();
            let mut score=0i32;
            if visual_id.eq_ignore_ascii_case("Microsoft.QuickAction.Bluetooth"){score+=1000;}
            if collection_text.contains("microsoft.quickaction.bluetooth"){score+=100;}
            if collection_text.contains("qa_bluetooth"){score+=50;}
            if collection_text.contains("bluetooth"){score+=10;}

            if score>0 && best.map_or(true,|(_,_,old)|score>old){best=Some((e.index,handle,score));}
        }
    }
    for e in std::slice::from_raw_parts_mut(elements,requested as usize) {
        if !e.value_type.is_null(){SysFreeString(e.value_type);}
        if !e.value.is_null(){SysFreeString(e.value);}
    }
    CoTaskMemFree(elements.cast());

    if let Some((index,handle,score))=best {
        // QuickActionViewModel is a plain WinRT bindable object here.
        // XamlOM GetPropertyValuesChain returned ERROR_NOT_FOUND for VM
        // handles in HIL; query only the visually-proven Bluetooth VM through
        // its advertised ICustomPropertyProvider to keep the UI-thread probe bounded.
        probe_custom_property_provider(index,handle);
        log(&format!("[N3] VM snapshot complete; visually-proven Bluetooth item[{index}] handle=0x{handle:x} score={score}. Full dump: qs-vm-dump.log"));
        schedule_duplicate_native_vm(source,handle);
    } else {
        log("[N3.2] VM snapshot complete but no visual AutomationId proved the Bluetooth item. See qs-vm-dump.log");
    }
    Ok(())
}


#[repr(C)]
struct SourceInfo { filename: *const u16, line: u32, column: u32, position: u32, hash: *const u16 }
#[repr(C)]
#[derive(Clone, Copy)]
struct ParentChildRelation { parent: u64, child: u64, index: u32 }
#[repr(C)]
struct VisualElement { handle: u64, source: SourceInfo, ty: *const u16, name: *const u16, child_count: u32 }
unsafe fn bstr_string(value: *const u16) -> String {
    if value.is_null() { String::new() }
    else { String::from_utf16_lossy(std::slice::from_raw_parts(value, SysStringLen(value) as usize)) }
}

#[derive(Clone)]
struct Node { parent: u64, index: u32, ty: String, name: String }
static TREE: OnceLock<Mutex<HashMap<u64, Node>>> = OnceLock::new();
static WATCHER_STARTED: OnceLock<()> = OnceLock::new();
static CALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);
fn tree() -> &'static Mutex<HashMap<u64,Node>> { TREE.get_or_init(|| Mutex::new(HashMap::new())) }

fn is_quick_settings_wrap(nodes: &HashMap<u64,Node>, mut handle:u64)->bool {
    let (mut grid,mut toggles)=(false,false);
    for _ in 0..40 {
        // Initial XAML callbacks may report the target subtree before its
        // outer page/root ancestors. Missing a node above BOTH required
        // anchors must not invalidate a proven ancestor chain.
        let Some(n)=nodes.get(&handle) else { break; };
        if n.name == "RootGridView" && n.ty.ends_with("GridView") { grid=true; }
        if n.name == "TogglesGroup" { toggles=true; }
        if n.parent==0 || n.parent==handle { break; }
        handle=n.parent;
    }
    grid && toggles
}

fn quick_actions_grid_handle(nodes:&HashMap<u64,Node>,mut handle:u64)->Option<u64> {
    let mut grid=None;
    let mut toggles=false;
    for _ in 0..40 {
        let Some(n)=nodes.get(&handle) else { break; };
        if n.name=="RootGridView" && n.ty.ends_with("GridView"){grid=Some(handle);}
        if n.name=="TogglesGroup"{toggles=true;}
        if n.parent==0||n.parent==handle{break;}
        handle=n.parent;
    }
    if toggles{grid}else{None}
}

fn ancestor_summary(nodes: &HashMap<u64,Node>, mut handle:u64) -> String {
    let mut levels = Vec::new();
    for _ in 0..24 {
        let Some(n)=nodes.get(&handle) else {
            levels.push(format!("0x{handle:x}:<MISSING>"));
            break;
        };
        levels.push(format!("{}#{}@0x{handle:x}", n.ty, n.name));
        if n.parent==0 || n.parent==handle { break; }
        handle=n.parent;
    }
    levels.join(" <- ")
}

#[repr(C)]
struct Callback { vtable: *const CallbackVtbl, count: AtomicU32 }
#[repr(C)]
struct CallbackVtbl {
    query_interface: unsafe extern "system" fn(*mut Callback,*const Guid,*mut *mut c_void)->i32,
    add_ref: unsafe extern "system" fn(*mut Callback)->u32,
    release: unsafe extern "system" fn(*mut Callback)->u32,
    on_visual_tree_change: unsafe extern "system" fn(*mut Callback,ParentChildRelation,VisualElement,i32)->i32,
    on_element_state_changed: unsafe extern "system" fn(*mut Callback,u64,i32,*const u16)->i32,
}
unsafe extern "system" fn cb_query(this:*mut Callback, iid:*const Guid, out:*mut *mut c_void)->i32 {
    if iid.is_null() || out.is_null() { return E_POINTER; }
    *out = null_mut();
    if *iid == IID_IUNKNOWN || *iid == IID_IVISUALTREESERVICECALLBACK ||
       *iid == IID_IVISUALTREESERVICECALLBACK2 {
        *out = this.cast(); cb_add_ref(this); S_OK
    } else { E_NOINTERFACE }
}
unsafe extern "system" fn cb_add_ref(this:*mut Callback)->u32 {
    (*this).count.fetch_add(1,Ordering::Relaxed)+1
}
unsafe extern "system" fn cb_release(this:*mut Callback)->u32 {
    let prev=(*this).count.fetch_sub(1,Ordering::AcqRel);
    if prev==1 { drop(Box::from_raw(this)); 0 } else { prev-1 }
}
// Debug counters are deliberately bounded; do not dump the entire XAML tree
// from inside ShellHost on every callback.
unsafe extern "system" fn on_change(_this:*mut Callback, relation:ParentChildRelation,
                                    element:VisualElement,mutation:i32)->i32 {
    let sequence=CALLBACK_COUNT.fetch_add(1,Ordering::Relaxed)+1;
    let result=std::panic::catch_unwind(|| {
        if sequence==1 { log("[N2/TRACE] First OnVisualTreeChange callback received"); }
        let ty=unsafe{bstr_string(element.ty)};
        let mut name=unsafe{bstr_string(element.name)};
        if mutation==0 && name.is_empty() && (ty.ends_with("GridView") || ty.ends_with("ContentControl")) {
            if let Ok(runtime_name) = unsafe { framework_element_name(element.handle) } {
                if !runtime_name.is_empty() { name=runtime_name; }
            }
        }
        let is_wrap=ty.ends_with("ItemsWrapGrid");
        let is_item=ty.ends_with("GridViewItem");
        let is_named_anchor=name=="TogglesGroup" || name=="RootGridView";
        if sequence<=8 || sequence%256==0 || is_wrap || is_named_anchor {
            log(&format!("[N2/TRACE] event={sequence} mutation={mutation} handle=0x{:x} parent=0x{:x} type={ty} name={name}",
                element.handle,relation.parent));
        }

        let mut candidate_grids=Vec::new();
        let mut missing_ancestor=None;
        if let Ok(mut nodes)=tree().lock() {
            if mutation==0 {
                nodes.insert(element.handle,Node {parent:relation.parent,index:relation.index,ty:ty.clone(),name});
                // The initial callbacks are not guaranteed parent-first: once a
                // missing ancestor arrives, retry every known Quick Actions wrap.
                if is_wrap || is_item || is_named_anchor {
                    for (&handle,node) in nodes.iter() {
                        if !node.ty.ends_with("ItemsWrapGrid") { continue; }
                        if is_quick_settings_wrap(&nodes,handle) {
                            if !is_item || relation.parent==handle {
                                if let Some(grid)=quick_actions_grid_handle(&nodes,handle) {
                                    candidate_grids.push(grid);
                                }
                            }
                        } else if is_wrap && handle==element.handle {
                            missing_ancestor=Some(ancestor_summary(&nodes,handle));
                        }
                    }
                }
            } else if mutation==1 {
                if is_wrap {
                    if let Ok(mut set)=inserted_hosts().lock(){set.remove(&element.handle);}
                }
                nodes.remove(&element.handle);
            }
        }
        if let Some(trace)=missing_ancestor {
            log(&format!("[N2/TRACE] ItemsWrapGrid exists, but target ancestors not yet matched: {trace}"));
        }
        candidate_grids.sort_unstable();
        candidate_grids.dedup();
        for grid in candidate_grids {
            // N3 is deliberately read-only: inspect the ItemsSource VM vector
            // on the next UI-loop turn; do not touch ItemsWrapGrid.Children.
            log(&format!("[N3/TRACE] Quick Actions grid matched, scheduling VM snapshot grid=0x{grid:x}"));
            schedule_vm_probe(grid);
        }
    });
    if result.is_err(){log("[N3] callback panic trapped at COM boundary");}
    S_OK
}
unsafe extern "system" fn on_state_change(_: *mut Callback, _: u64, _: i32, _: *const u16)->i32 {
    S_OK
}
static CALLBACK_VTBL: CallbackVtbl = CallbackVtbl {
    query_interface: cb_query, add_ref: cb_add_ref, release: cb_release,
    on_visual_tree_change: on_change,
    on_element_state_changed: on_state_change,
};


unsafe fn configure_mode_from_initialization_data(diagnostics:*mut c_void) {
    let get_data:unsafe extern "system" fn(*mut c_void,*mut *mut u16)->i32=
        std::mem::transmute(slot(diagnostics,10));
    let mut raw=null_mut();
    let hr=get_data(diagnostics,&mut raw);
    if hr<0 {
        log(&format!("[N4] GetInitializationData failed: 0x{:08X}; defaulting to probe-only",hr as u32));
        return;
    }
    let mode=if raw.is_null(){String::new()}else{bstr_string(raw)};
    if !raw.is_null(){SysFreeString(raw);}
    let duplicate=mode.eq_ignore_ascii_case("duplicate-native-vm");
    DUPLICATE_NATIVE_VM_MODE.store(duplicate,Ordering::Release);
    log(&format!("[N4] initialization mode={mode:?} duplicate_native_vm={duplicate}"));
}

#[repr(C)]
struct Site { vtable: *const SiteVtbl, count: AtomicU32, site: AtomicUsize }
#[repr(C)]
struct SiteVtbl {
    query_interface: unsafe extern "system" fn(*mut Site,*const Guid,*mut *mut c_void)->i32,
    add_ref: unsafe extern "system" fn(*mut Site)->u32,
    release: unsafe extern "system" fn(*mut Site)->u32,
    set_site: unsafe extern "system" fn(*mut Site,*mut c_void)->i32,
    get_site: unsafe extern "system" fn(*mut Site,*const Guid,*mut *mut c_void)->i32,
}
unsafe extern "system" fn site_query(this:*mut Site,iid:*const Guid,out:*mut *mut c_void)->i32 {
    if iid.is_null() || out.is_null() { return E_POINTER; }
    *out = null_mut();
    if *iid==IID_IUNKNOWN || *iid==IID_IOBJECTWITHSITE {
        *out=this.cast(); site_add_ref(this); S_OK
    } else { E_NOINTERFACE }
}
unsafe extern "system" fn site_add_ref(this:*mut Site)->u32 { (*this).count.fetch_add(1,Ordering::Relaxed)+1 }
unsafe extern "system" fn site_release(this:*mut Site)->u32 {
    let prev=(*this).count.fetch_sub(1,Ordering::AcqRel);
    if prev==1 {
        let old=(*this).site.swap(0,Ordering::AcqRel);
        com_release(old as *mut c_void);
        drop(Box::from_raw(this)); 0
    } else { prev-1 }
}
unsafe extern "system" fn site_set(this:*mut Site, input:*mut c_void)->i32 {
    if input.is_null() { return S_OK; }
    if WATCHER_STARTED.get().is_some() {
        log("[N3] SetSite called again; existing Rust TAP is already active");
        return S_OK;
    }
    let mut service:*mut c_void=null_mut();
    let hr=query_interface(input,&IID_IVISUALTREESERVICE3,&mut service);
    if hr<0 { log(&format!("QueryInterface(IVisualTreeService3) failed 0x{:08X}",hr as u32)); return hr; }
    let mut diagnostics:*mut c_void=null_mut();
    let hr=query_interface(input,&IID_IXAMLDIAGNOSTICS,&mut diagnostics);
    if hr<0 {
        com_release(service);
        log(&format!("QueryInterface(IXamlDiagnostics) failed 0x{:08X}",hr as u32));
        return hr;
    }
    if WATCHER_STARTED.set(()).is_err() {
        com_release(service); com_release(diagnostics);
        return S_OK;
    }
    // TAP is intentionally process-lifetime pinned in this read-only N3 probe.
    // The interface references remain valid across callback invocations.
    SERVICE_PTR.store(service as usize,Ordering::Release);
    DIAGNOSTICS_PTR.store(diagnostics as usize,Ordering::Release);
    configure_mode_from_initialization_data(diagnostics);
    let old=(*this).site.swap(input as usize,Ordering::AcqRel);
    let add:unsafe extern "system" fn(*mut c_void)->u32 = std::mem::transmute(slot(input,1));
    add(input);
    com_release(old as *mut c_void);
    log("SetSite received. Rust native TAP is loaded inside ShellHost.");
    {
        let handle=service as usize;
        std::thread::spawn(move || unsafe {
            // From a new thread: advising synchronously from SetSite can hang ShellHost.
            let service=handle as *mut c_void;
            let cb=Box::into_raw(Box::new(Callback {vtable:&CALLBACK_VTBL,count:AtomicU32::new(1)}));
            let advise: unsafe extern "system" fn(*mut c_void,*mut Callback)->i32=
                std::mem::transmute(slot(service,3));
            let hr=advise(service,cb);
            log(&format!("AdviseVisualTreeChange HRESULT=0x{:08X}, callbacks_so_far={}",
                hr as u32, CALLBACK_COUNT.load(Ordering::Relaxed)));
            if hr>=0 {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                let callback_count=CALLBACK_COUNT.load(Ordering::Relaxed);
                let (nodes,wraps,named)=tree().lock().map(|map| (
                    map.len(), map.values().filter(|n|n.ty.ends_with("ItemsWrapGrid")).count(),
                    map.values().filter(|n|n.name=="TogglesGroup" || n.name=="RootGridView").count()
                )).unwrap_or((0,0,0));
                log(&format!("[N3/TRACE] 1500ms snapshot: callbacks={callback_count} nodes={nodes} wraps={wraps} named_anchors={named}"));
            }
            // The process is intentionally pinned for this experimental session.
            // Callback and diagnostics COM refs live until ShellHost exits.
        });
    }
    S_OK
}
unsafe extern "system" fn site_get(this:*mut Site,iid:*const Guid,out:*mut *mut c_void)->i32 {
    if iid.is_null()||out.is_null() {return E_POINTER;}
    let site=(*this).site.load(Ordering::Acquire) as *mut c_void;
    if site.is_null(){*out=null_mut();return E_FAIL;}
    query_interface(site,&*iid,out)
}
static SITE_VTBL:SiteVtbl=SiteVtbl {query_interface:site_query,add_ref:site_add_ref,
  release:site_release,set_site:site_set,get_site:site_get};

#[repr(C)]
struct Factory {vtable:*const FactoryVtbl,count:AtomicU32}
#[repr(C)]
struct FactoryVtbl {
    query_interface: unsafe extern "system" fn(*mut Factory,*const Guid,*mut *mut c_void)->i32,
    add_ref: unsafe extern "system" fn(*mut Factory)->u32,
    release: unsafe extern "system" fn(*mut Factory)->u32,
    create_instance: unsafe extern "system" fn(*mut Factory,*mut c_void,*const Guid,*mut *mut c_void)->i32,
    lock_server: unsafe extern "system" fn(*mut Factory,i32)->i32,
}
unsafe extern "system" fn fac_query(this:*mut Factory,iid:*const Guid,out:*mut *mut c_void)->i32{
    if iid.is_null() || out.is_null(){return E_POINTER;}
    *out=null_mut();
    if *iid==IID_IUNKNOWN||*iid==IID_ICLASSFACTORY {*out=this.cast();fac_add_ref(this);S_OK}
    else{E_NOINTERFACE}
}
unsafe extern "system" fn fac_add_ref(this:*mut Factory)->u32 {(*this).count.fetch_add(1,Ordering::Relaxed)+1}
unsafe extern "system" fn fac_release(this:*mut Factory)->u32 {
    let prev=(*this).count.fetch_sub(1,Ordering::AcqRel);
    if prev==1 {drop(Box::from_raw(this));0} else{prev-1}
}
unsafe extern "system" fn fac_create(_this:*mut Factory,outer:*mut c_void,
    iid:*const Guid,out:*mut *mut c_void)->i32{
    if iid.is_null() || out.is_null(){return E_POINTER;}
    *out=null_mut();
    if !outer.is_null(){return CLASS_E_NOAGGREGATION;}
    let site=Box::into_raw(Box::new(Site{vtable:&SITE_VTBL,count:AtomicU32::new(1),site:AtomicUsize::new(0)}));
    let hr=site_query(site,iid,out);
    site_release(site);  // Return only the reference acquired by QI.
    hr
}
unsafe extern "system" fn fac_lock(_this:*mut Factory,_lock:i32)->i32 {S_OK}
static FACTORY_VTBL:FactoryVtbl=FactoryVtbl{query_interface:fac_query,add_ref:fac_add_ref,
  release:fac_release,create_instance:fac_create,lock_server:fac_lock};

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid:*const Guid,iid:*const Guid,out:*mut *mut c_void)->i32{
    if clsid.is_null()||iid.is_null()||out.is_null(){return E_POINTER;}
    *out=null_mut();
    if *clsid!=CLSID_TAP {return CLASS_E_CLASSNOTAVAILABLE;}
    let fac=Box::into_raw(Box::new(Factory{vtable:&FACTORY_VTBL,count:AtomicU32::new(1)}));
    let hr=fac_query(fac,iid,out);
    fac_release(fac);
    hr
}
#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow()->i32 {S_FALSE}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_struct_layout_matches_xamlom_idl_on_x64() {
        assert_eq!(std::mem::size_of::<Guid>(), 16);
        assert_eq!(std::mem::size_of::<ParentChildRelation>(), 24);
        assert_eq!(std::mem::size_of::<SourceInfo>(), 32);
        assert_eq!(std::mem::size_of::<VisualElement>(), 64);
        assert_eq!(std::mem::size_of::<CollectionElementValue>(), 32);
        assert_eq!(std::mem::size_of::<PropertyChainSource>(), 64);
        assert_eq!(std::mem::size_of::<PropertyChainValue>(), 80);
    }

    #[test]
    fn only_quick_actions_wrap_matches_not_slider_grid() {
        let mut nodes=HashMap::new();
        nodes.insert(1,Node{index:0,parent:0,ty:"ControlCenter.ControlCenterPage".into(),name:String::new()});
        nodes.insert(2,Node{index:0,parent:1,ty:"ContentControl".into(),name:"TogglesGroup".into()});
        nodes.insert(3,Node{index:0,parent:2,ty:"GridView".into(),name:"RootGridView".into()});
        assert!(is_quick_settings_wrap(&nodes,3));
        nodes.insert(4,Node{index:0,parent:1,ty:"ContentControl".into(),name:"SlidersGroup".into()});
        nodes.insert(5,Node{index:0,parent:4,ty:"GridView".into(),name:"RootGridView".into()});
        assert!(!is_quick_settings_wrap(&nodes,5));
    }

    #[test]
    fn late_ancestors_allow_wrap_matching() {
        let mut nodes=HashMap::new();
        nodes.insert(4,Node{index:0,parent:3,ty:"Windows.UI.Xaml.Controls.ItemsWrapGrid".into(),name:String::new()});
        assert!(!is_quick_settings_wrap(&nodes,4));
        nodes.insert(3,Node{index:0,parent:2,ty:"Windows.UI.Xaml.Controls.GridView".into(),name:"RootGridView".into()});
        assert!(!is_quick_settings_wrap(&nodes,4));
        nodes.insert(2,Node{index:0,parent:1,ty:"Windows.UI.Xaml.Controls.ContentControl".into(),name:"TogglesGroup".into()});
        assert!(is_quick_settings_wrap(&nodes,4));

        // If the intermediate group disappears, finding RootGridView alone
        // must not be enough to match the Quick Actions host.
        nodes.remove(&2);
        assert!(!is_quick_settings_wrap(&nodes,4));
    }

    #[test]
    fn missing_outer_ancestor_does_not_invalidate_proven_anchor_chain() {
        let mut nodes=HashMap::new();
        nodes.insert(4,Node{index:0,parent:3,ty:"Windows.UI.Xaml.Controls.ItemsWrapGrid".into(),name:String::new()});
        nodes.insert(3,Node{index:0,parent:2,ty:"Windows.UI.Xaml.Controls.GridView".into(),name:"RootGridView".into()});
        nodes.insert(2,Node{index:0,parent:99,ty:"Windows.UI.Xaml.Controls.ContentControl".into(),name:"TogglesGroup".into()});
        assert!(is_quick_settings_wrap(&nodes,4));
    }

    #[test]
    fn parse_instance_handle_accepts_xamlom_formats() {
        assert_eq!(parse_instance_handle("0x1d3b6bfd9d8"),Some(0x1d3b6bfd9d8));
        assert_eq!(parse_instance_handle("12345"),Some(12345));
        assert_eq!(parse_instance_handle("handle=0xABC; evaluated=foo"),Some(0xabc));
    }

    #[test]
    fn n4_duplicate_mode_is_explicit_and_off_by_default() {
        assert!(!DUPLICATE_NATIVE_VM_MODE.load(Ordering::Relaxed));
        assert_ne!(WM_BE200_QS_PROBE,WM_BE200_QS_DUPLICATE_VM);
    }

    #[test]
    fn n32_known_winrt_iids_match_public_abi() {
        assert_eq!(guid_text(&IID_ICUSTOMPROPERTYPROVIDER),"7c925755-3e48-42b4-8677-76372267033f");
        assert_eq!(guid_text(&IID_IPROPERTYVALUE),"4bd682dd-7554-40e9-9a9b-82654ede7e62");
        assert_eq!(guid_text(&IID_IACTIVATIONFACTORY),"00000035-0000-0000-c000-000000000046");
    }

    #[test]
    fn inherited_property_candidate_names_cover_build_26200() {
        let items=["ItemsSource","ItemsControl.ItemsSource","Windows.UI.Xaml.Controls.ItemsControl.ItemsSource"];
        assert!(items.iter().any(|x|x.ends_with("ItemsSource")));
        let content=["Content","ContentControl.Content","Windows.UI.Xaml.Controls.ContentControl.Content"];
        assert!(content.iter().any(|x|x.ends_with("Content")));
    }

    #[test]
    fn metadata_handle_detection_accepts_handle_and_evaluated_value() {
        assert!(metadata_has_value_handle(META_IS_VALUE_HANDLE));
        assert!(metadata_has_value_handle(META_IS_VALUE_HANDLE_AND_EVALUATED_VALUE));
        assert!(metadata_has_value_handle(META_IS_VALUE_HANDLE_AND_EVALUATED_VALUE | META_IS_PROPERTY_READ_ONLY));
        assert!(!metadata_has_value_handle(META_IS_VALUE_COLLECTION));
    }

    #[test]
    fn node_keeps_xaml_child_index_for_container_vm_fallback() {
        let node=Node{index:7,parent:11,ty:"Windows.UI.Xaml.Controls.GridViewItem".into(),name:String::new()};
        assert_eq!(node.index,7);
        assert_eq!(node.parent,11);
    }

    #[test]
    fn markup_contains_only_ui_and_named_button() {
        assert!(TILE_XAML.contains("x:Name=\"BE200QuickSettingsClick\""));
        assert!(TILE_XAML.contains("断开 B3P"));
        assert!(!TILE_XAML.contains("WinUSB"));
    }
}