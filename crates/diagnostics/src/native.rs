use crate::config::Config;
use libloading::Library;
use std::{
    ffi::{CString, c_char, c_int},
    path::Path,
};

type SetString = unsafe extern "C" fn(*const c_char);
type SetValue = unsafe extern "C" fn(*const c_char, *const c_char);
type ReportWindows = unsafe extern "C" fn(
    c_int,
    *const c_char,
    *const c_char,
    *const c_char,
    *const c_char,
    bool,
    *const c_char,
);
type ReportMac =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char) -> c_int;

enum Api {
    Windows {
        report: ReportWindows,
        set_value: SetValue,
    },
    Mac {
        report: ReportMac,
        set_value: SetValue,
    },
}
pub(crate) struct NativeSdk {
    // SDK crash handlers can run during teardown. Once initialized, this entire
    // object is deliberately retained until process termination (no dlclose).
    _library: Library,
    _initial_strings: Vec<CString>,
    _workspace: Vec<u16>,
    api: Api,
}
fn c(value: &str) -> Result<CString, &'static str> {
    CString::new(value).map_err(|_| "Unexpected NUL in SDK configuration")
}
unsafe fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, &'static str> {
    // SAFETY: caller supplies the exact documented ABI for each known symbol.
    unsafe {
        library
            .get::<T>(name)
            .map(|symbol| *symbol)
            .map_err(|_| "CrashSight SDK ABI is incompatible; required export missing")
    }
}
unsafe fn library(path: &Path) -> Result<Library, &'static str> {
    if !path.is_absolute() || !path.is_file() {
        return Err("CrashSight SDK is not installed");
    }
    #[cfg(windows)]
    {
        // Search the SDK directory and System32, never the current working directory.
        unsafe {
            libloading::os::windows::Library::load_with_flags(path, 0x100 | 0x800)
                .map(Into::into)
                .map_err(|_| "Unable to load CrashSight SDK or its dependencies")
        }
    }
    #[cfg(not(windows))]
    unsafe {
        Library::new(path).map_err(|_| "Unable to load CrashSight SDK or its dependencies")
    }
}

impl NativeSdk {
    pub fn load(
        exe_dir: &Path,
        data_dir: &Path,
        config: &Config,
        version: &str,
    ) -> Result<&'static Self, &'static str> {
        if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
            Self::windows(
                &exe_dir.join("diagnostics/CrashSight64.dll"),
                data_dir,
                config,
                version,
            )
        } else if cfg!(target_os = "macos") {
            let bundle = exe_dir.join("../Frameworks/libturbodbnote_crashsight.dylib");
            let path = if bundle.is_file() {
                bundle
            } else {
                exe_dir.join("diagnostics/libturbodbnote_crashsight.dylib")
            };
            Self::mac(&path, config, version)
        } else {
            Err("CrashSight native adapter is unavailable on this target")
        }
    }
    fn windows(
        path: &Path,
        data_dir: &Path,
        config: &Config,
        version: &str,
    ) -> Result<&'static Self, &'static str> {
        let strings = vec![
            c(&config.app_id)?,
            c(config.region.windows_host())?,
            c(version)?,
        ];
        std::fs::create_dir_all(data_dir)
            .map_err(|_| "Unable to create native diagnostic directory")?;
        let workspace: Vec<u16> = data_dir
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: the SDK path is application-controlled; all exports are resolved
        // before any call. Windows 2.2.1+ documents these C signatures.
        unsafe {
            let lib = library(path)?;
            let set_version: SetString = symbol(&lib, b"CS_SetAppVersion\0")?;
            let set_server: SetString = symbol(&lib, b"CS_ConfigCrashServerUrl\0")?;
            let set_workspace: unsafe extern "C" fn(*const u16) =
                symbol(&lib, b"CS_SetWorkSpaceW\0")?;
            let init: SetString = symbol(&lib, b"CS_InitWithAppId\0")?;
            let upload: unsafe extern "C" fn(bool) = symbol(&lib, b"CS_SetCrashUploadEnable\0")?;
            let report = symbol(&lib, b"CS_ReportException\0")?;
            let set_value = symbol(&lib, b"CS_SetUserValue\0")?;
            let sdk = Box::leak(Box::new(Self {
                _library: lib,
                _initial_strings: strings,
                _workspace: workspace,
                api: Api::Windows { report, set_value },
            }));
            set_version(sdk._initial_strings[2].as_ptr());
            set_server(sdk._initial_strings[1].as_ptr());
            set_workspace(sdk._workspace.as_ptr());
            upload(true);
            init(sdk._initial_strings[0].as_ptr());
            Ok(sdk)
        }
    }
    fn mac(path: &Path, config: &Config, version: &str) -> Result<&'static Self, &'static str> {
        let strings = vec![c(&config.app_id)?, c(config.region.mac_url())?, c(version)?];
        // SAFETY: these are our bridge's versioned C exports, not assumed CS_* exports.
        unsafe {
            let lib = library(path)?;
            let abi: unsafe extern "C" fn() -> u32 = symbol(&lib, b"tdn_cs_abi_version\0")?;
            if abi() != 1 {
                return Err("CrashSight macOS bridge ABI mismatch");
            }
            let init: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> c_int =
                symbol(&lib, b"tdn_cs_init\0")?;
            let report = symbol(&lib, b"tdn_cs_report\0")?;
            let set_value = symbol(&lib, b"tdn_cs_set_value\0")?;
            let sdk = Box::leak(Box::new(Self {
                _library: lib,
                _initial_strings: strings,
                _workspace: vec![],
                api: Api::Mac { report, set_value },
            }));
            if init(
                sdk._initial_strings[0].as_ptr(),
                sdk._initial_strings[1].as_ptr(),
                sdk._initial_strings[2].as_ptr(),
            ) != 0
            {
                return Err("CrashSight macOS bridge initialization failed");
            }
            Ok(sdk)
        }
    }
    pub fn set_value(&self, key: &str, value: &str) {
        let (Ok(key), Ok(value)) = (c(key), c(value)) else {
            return;
        };
        let set = match self.api {
            Api::Windows { set_value, .. } | Api::Mac { set_value, .. } => set_value,
        };
        // SAFETY: validated function pointer; strings remain live for the call.
        unsafe {
            set(key.as_ptr(), value.as_ptr());
        }
    }
    pub fn report(&self, name: &str, message: &str, stack: &str, extras: &str) -> bool {
        let (Ok(name), Ok(message), Ok(stack), Ok(extras)) =
            (c(name), c(message), c(stack), c(extras))
        else {
            return false;
        };
        // SAFETY: the APIs use UTF-8 C strings and copy report data during the call.
        // No file attachments, callback pointers, or owned Rust objects cross FFI.
        unsafe {
            match self.api {
                Api::Windows { report, .. } => {
                    report(
                        1,
                        name.as_ptr(),
                        message.as_ptr(),
                        stack.as_ptr(),
                        extras.as_ptr(),
                        true,
                        c"".as_ptr(),
                    );
                    true // submitted to SDK, not a server delivery acknowledgement
                }
                Api::Mac { report, .. } => {
                    report(
                        name.as_ptr(),
                        message.as_ptr(),
                        stack.as_ptr(),
                        extras.as_ptr(),
                    ) == 0
                }
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn documented_seven_argument_abi_and_initialization_order() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("mock.dll");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_sdk.rs");
        let build = std::process::Command::new("rustc")
            .args(["--edition=2024", "--crate-type=cdylib"])
            .arg(source)
            .arg("-o")
            .arg(&dll)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "Mock DLL compilation failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let config =
            Config::from_json(br#"{"enabled":true,"app_id":"testonly","region":"global"}"#)
                .unwrap();
        let sdk =
            NativeSdk::windows(&dll, &dir.path().join("中文工作目录"), &config, "1.2.3").unwrap();
        sdk.set_value("app.last_action", "settings_opened");
        assert!(sdk.report("Test", "redacted", "", "{}"));
        // SAFETY: this extra export exists only in the fixture compiled above.
        let read: unsafe extern "C" fn(*mut u8, usize) -> usize =
            unsafe { symbol(&sdk._library, b"mock_copy_calls\0").unwrap() };
        let mut bytes = vec![0; 4096];
        let len = unsafe { read(bytes.as_mut_ptr(), bytes.len()) };
        let calls = String::from_utf8(bytes[..len].to_vec()).unwrap();
        assert!(calls.starts_with(
            "version:1.2.3\nserver:pc.crashsight.wetest.net\nworkspace\nupload:true\ninit:testonly"
        ));
        assert!(calls.contains("report:1:Test:redacted::{}:true:"));
        assert!(calls.contains("kv:app.last_action=settings_opened"));
    }
    #[test]
    fn absent_sdk_is_an_error_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        assert!(
            NativeSdk::windows(&dir.path().join("absent.dll"), dir.path(), &config, "1").is_err()
        );
    }
}
