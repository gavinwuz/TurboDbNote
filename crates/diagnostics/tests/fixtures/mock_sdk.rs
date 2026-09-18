//! Local-only fake DLL: never captures crashes or accesses a network.
use std::ffi::{CStr, c_char};
use std::sync::Mutex;
static CALLS: Mutex<Vec<String>>=Mutex::new(Vec::new());
fn string(value:*const c_char)->String { unsafe {CStr::from_ptr(value).to_string_lossy().into_owned()} }
fn call(value:String) {CALLS.lock().unwrap().push(value);}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_SetAppVersion(v:*const c_char){call(format!("version:{}",string(v)));}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_ConfigCrashServerUrl(v:*const c_char){call(format!("server:{}",string(v)));}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_SetWorkSpaceW(v:*const u16){assert!(!v.is_null());call("workspace".into());}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_SetCrashUploadEnable(v:bool){call(format!("upload:{v}"));}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_InitWithAppId(v:*const c_char){call(format!("init:{}",string(v)));}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_SetUserValue(k:*const c_char,v:*const c_char){call(format!("kv:{}={}",string(k),string(v)));}
#[unsafe(no_mangle)] pub unsafe extern "C" fn CS_ReportException(kind:i32,n:*const c_char,m:*const c_char,s:*const c_char,e:*const c_char,async_:bool,a:*const c_char){
    call(format!("report:{kind}:{}:{}:{}:{}:{async_}:{}",string(n),string(m),string(s),string(e),string(a)));
}
#[unsafe(no_mangle)] pub unsafe extern "C" fn mock_copy_calls(out:*mut u8,capacity:usize)->usize {
    let data=CALLS.lock().unwrap().join("\n");let len=data.len().min(capacity);
    unsafe {std::ptr::copy_nonoverlapping(data.as_ptr(),out,len);}
    len
}
