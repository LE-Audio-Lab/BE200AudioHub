//! Rust-native TAP for Windows XAML Diagnostics.
//!
//! Loads inside ShellHost.exe.  This first milestone proves our own Rust COM
//! server can receive the Windows XAML tree without Windhawk or C++.
//! No Bluetooth functionality and no foreign UI mutation is performed yet.
#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::ptr::{null, null_mut};
use std::sync::{atomic::{AtomicU32, AtomicUsize, Ordering}, Mutex, OnceLock};

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

#[link(name="kernel32")]
extern "system" { fn OutputDebugStringW(s: *const u16); }
#[link(name="oleaut32")]
extern "system" { fn SysStringLen(bstr: *const u16) -> u32; }

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
static ROOT_FOUND: OnceLock<()> = OnceLock::new();
static WATCHER_STARTED: OnceLock<()> = OnceLock::new();
fn tree() -> &'static Mutex<HashMap<u64,Node>> { TREE.get_or_init(|| Mutex::new(HashMap::new())) }

fn is_quick_settings_wrap(nodes: &HashMap<u64,Node>, mut handle:u64)->bool {
    for _ in 0..32 {
        let Some(n) = nodes.get(&handle) else { return false; };
        if n.name == "RootGridView" && n.ty.contains("GridView") { return true; }
        if n.parent == 0 || n.parent == handle { break; }
        handle = n.parent;
    }
    false
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
    let ty=bstr_string(element.ty);
    let name=bstr_string(element.name);
    if let Ok(mut nodes)=tree().lock() {
        if mutation==0 {
            nodes.insert(element.handle,Node {parent:relation.parent,ty:ty.clone(),name});
            if ty.ends_with("ItemsWrapGrid")
               && is_quick_settings_wrap(&nodes, relation.parent)
               && ROOT_FOUND.set(()).is_ok() {
                log(&format!("TARGET FOUND: ItemsWrapGrid handle=0x{:x} parent=0x{:x}; direct tile insertion NOT YET ENABLED",
                            element.handle,relation.parent));
            }
        } else if mutation==1 { nodes.remove(&element.handle); }
    }
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
    let mut service:*mut c_void=null_mut();
    let hr=query_interface(input,&IID_IVISUALTREESERVICE3,&mut service);
    if hr<0 { log(&format!("QueryInterface(IVisualTreeService3) failed 0x{:08X}",hr as u32)); return hr; }
    let old=(*this).site.swap(input as usize,Ordering::AcqRel);
    let add:unsafe extern "system" fn(*mut c_void)->u32 = std::mem::transmute(slot(input,1));
    add(input);
    com_release(old as *mut c_void);
    log("SetSite received. Rust native TAP is loaded inside ShellHost.");
    if WATCHER_STARTED.set(()).is_ok() {
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
            // The callback's initial COM ref remains until ShellHost exits.
        });
    } else { com_release(service); }
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