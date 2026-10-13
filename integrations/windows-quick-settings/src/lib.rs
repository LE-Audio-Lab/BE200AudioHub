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
use std::ptr::{null, null_mut};
use std::sync::{atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering}, Mutex, OnceLock};

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_NOINTERFACE: i32 = 0x80004002u32 as i32;
const E_POINTER: i32 = 0x80004003u32 as i32;
const E_FAIL: i32 = 0x80004005u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: i32 = 0x80040111u32 as i32;
const CLASS_E_NOAGGREGATION: i32 = 0x80040110u32 as i32;

#[repr(C)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct Guid { d1: u32, d2: u16, d3: u16, d4: [u8; 8] }

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


#[link(name="kernel32")]
extern "system" { fn OutputDebugStringW(s: *const u16); }
#[link(name="oleaut32")]
extern "system" { fn SysStringLen(bstr: *const u16) -> u32; }
#[link(name="ole32")]
extern "system" { fn CoTaskMemAlloc(size: usize) -> *mut c_void; }
#[link(name="runtimeobject")]
extern "system" {
    fn WindowsCreateString(value: *const u16, length: u32, result: *mut *mut c_void) -> i32;
    fn WindowsDeleteString(value: *mut c_void) -> i32;
    fn RoGetActivationFactory(name: *mut c_void, iid: *const Guid, result: *mut *mut c_void) -> i32;
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

fn log(message: &str) {
    let output = format!("[BE200-QS/TAP] {message}");
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
static INSERTED_HOSTS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
fn inserted_hosts() -> &'static Mutex<HashSet<u64>> {
    INSERTED_HOSTS.get_or_init(|| Mutex::new(HashSet::new()))
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
    if count < 6 { return Err(S_FALSE); } // Wait for native item realization.

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

fn try_insert(host:u64) {
    if INSERT_BUSY.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() { return; }
    // Guard clears even if an unexpected panic happens while composing XAML.
    struct Guard;
    impl Drop for Guard { fn drop(&mut self){INSERT_BUSY.store(false,Ordering::Release);} }
    let _guard=Guard;
    if inserted_hosts().lock().map(|set|set.contains(&host)).unwrap_or(true) {return;}
    let result=std::panic::catch_unwind(|| unsafe { insert_tile(host) });
    match result {
        Ok(Ok(()))=>{if let Ok(mut set)=inserted_hosts().lock(){set.insert(host);}},
        Ok(Err(S_FALSE))=>{}, // no native containers realized yet
        Ok(Err(hr))=>log(&format!("[N2] INSERT FAILED: 0x{:08X} (host=0x{host:x})",hr as u32)),
        Err(_)=>log("[N2] panic caught while inserting (ShellHost left unchanged if AddChild not reached)"),
    }
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
struct Node { parent: u64, ty: String, name: String }
static TREE: OnceLock<Mutex<HashMap<u64, Node>>> = OnceLock::new();
static WATCHER_STARTED: OnceLock<()> = OnceLock::new();
fn tree() -> &'static Mutex<HashMap<u64,Node>> { TREE.get_or_init(|| Mutex::new(HashMap::new())) }

fn is_quick_settings_wrap(nodes: &HashMap<u64,Node>, mut handle:u64)->bool {
    let (mut grid,mut toggles)=(false,false);
    for _ in 0..40 {
        let Some(n)=nodes.get(&handle) else { return false; };
        if n.name == "RootGridView" && n.ty.ends_with("GridView") { grid=true; }
        if n.name == "TogglesGroup" { toggles=true; }
        if n.parent==0 || n.parent==handle { break; }
        handle=n.parent;
    }
    grid && toggles
}

#[repr(C)]
struct Callback { vtable: *const CallbackVtbl, count: AtomicU32 }
#[repr(C)]
struct CallbackVtbl {
    query_interface: unsafe extern "system" fn(*mut Callback,*const Guid,*mut *mut c_void)->i32,
    add_ref: unsafe extern "system" fn(*mut Callback)->u32,
    release: unsafe extern "system" fn(*mut Callback)->u32,
    on_visual_tree_change: unsafe extern "system" fn(*mut Callback,ParentChildRelation,VisualElement,i32)->i32,
}
unsafe extern "system" fn cb_query(this:*mut Callback, iid:*const Guid, out:*mut *mut c_void)->i32 {
    if iid.is_null() || out.is_null() { return E_POINTER; }
    *out = null_mut();
    if *iid == IID_IUNKNOWN || *iid == IID_IVISUALTREESERVICECALLBACK {
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
unsafe extern "system" fn on_change(_this:*mut Callback, relation:ParentChildRelation,
                                    element:VisualElement,mutation:i32)->i32 {
    let result=std::panic::catch_unwind(|| {
        let ty=unsafe{bstr_string(element.ty)};
        let name=unsafe{bstr_string(element.name)};
        let mut candidate=None;
        if let Ok(mut nodes)=tree().lock() {
            if mutation==0 {
                nodes.insert(element.handle,Node {parent:relation.parent,ty:ty.clone(),name});
                if ty.ends_with("ItemsWrapGrid") && is_quick_settings_wrap(&nodes,relation.parent) {
                    candidate=Some(element.handle);
                } else if ty.ends_with("GridViewItem") {
                    if let Some(parent)=nodes.get(&relation.parent) {
                        if parent.ty.ends_with("ItemsWrapGrid") &&
                           is_quick_settings_wrap(&nodes,parent.parent) {
                            candidate=Some(relation.parent);
                        }
                    }
                }
            } else if mutation==1 {
                if ty.ends_with("ItemsWrapGrid") {
                    if let Ok(mut set)=inserted_hosts().lock() {set.remove(&element.handle);}
                }
                nodes.remove(&element.handle);
            }
        }
        if let Some(host)=candidate {
            // Never hold the tree mutex while editing XAML. AddChild recurses
            // into OnVisualTreeChange and would otherwise deadlock ShellHost.
            try_insert(host);
        }
    });
    if result.is_err(){log("[N2] callback panic trapped at COM boundary");}
    S_OK
}
static CALLBACK_VTBL: CallbackVtbl = CallbackVtbl {
    query_interface: cb_query, add_ref: cb_add_ref, release: cb_release,
    on_visual_tree_change: on_change,
};

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
        log("[N2] SetSite called again; existing Rust TAP is already active");
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
    // TAP is intentionally process-lifetime pinned in this unsafe N2 PoC.
    // The interface references remain valid across callback invocations.
    SERVICE_PTR.store(service as usize,Ordering::Release);
    DIAGNOSTICS_PTR.store(diagnostics as usize,Ordering::Release);
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
            log(&format!("AdviseVisualTreeChange HRESULT=0x{:08X}",hr as u32));
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
    }

    #[test]
    fn only_quick_actions_wrap_matches_not_slider_grid() {
        let mut nodes=HashMap::new();
        nodes.insert(1,Node{parent:0,ty:"ControlCenter.ControlCenterPage".into(),name:String::new()});
        nodes.insert(2,Node{parent:1,ty:"ContentControl".into(),name:"TogglesGroup".into()});
        nodes.insert(3,Node{parent:2,ty:"GridView".into(),name:"RootGridView".into()});
        assert!(is_quick_settings_wrap(&nodes,3));
        nodes.insert(4,Node{parent:1,ty:"ContentControl".into(),name:"SlidersGroup".into()});
        nodes.insert(5,Node{parent:4,ty:"GridView".into(),name:"RootGridView".into()});
        assert!(!is_quick_settings_wrap(&nodes,5));
    }

    #[test]
    fn markup_contains_only_ui_and_named_button() {
        assert!(TILE_XAML.contains("x:Name=\"BE200QuickSettingsClick\""));
        assert!(TILE_XAML.contains("断开 B3P"));
        assert!(!TILE_XAML.contains("WinUSB"));
    }
}