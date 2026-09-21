//! Current-user DPAPI, with no application key or environment variable.
use uuid::Uuid;
pub fn protect(id: Uuid, value: &str) -> Result<Vec<u8>, String> {
    if value.len() > 2560 {
        return Err("密码超过支持长度".into());
    }
    transform(id, value.as_bytes(), true)
}
pub fn unprotect(id: Uuid, value: &[u8]) -> Result<String, String> {
    String::from_utf8(transform(id, value, false)?)
        .map_err(|_| "密码解密结果无效，请重新输入密码".into())
}
#[cfg(windows)]
fn transform(id: Uuid, value: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    if value.len() > 65536 {
        return Err("加密密码记录过大".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: value.len() as u32,
        pbData: value.as_ptr().cast_mut(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: 16,
        pbData: id.as_bytes().as_ptr().cast_mut(),
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    // SAFETY: buffers outlive these calls. Omitting LOCAL_MACHINE binds to the current user.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err(if encrypt {
            "无法加密密码，连接未保存"
        } else {
            "无法解密密码，请在原 Windows 用户及电脑上打开，或重新输入密码"
        }
        .into());
    }
    // SAFETY: successful DPAPI returns a valid allocation and size; free after copying.
    let result = unsafe {
        let bytes = if output.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        if !output.pbData.is_null() {
            for i in 0..output.cbData as usize {
                output.pbData.add(i).write_volatile(0);
            }
            LocalFree(output.pbData.cast());
        }
        bytes
    };
    Ok(result)
}
#[cfg(not(windows))]
fn transform(_: Uuid, _: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("当前平台不支持 Windows 密码保护，请使用会话密码".into())
}
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn encrypted_password_is_randomized_and_bound_to_reference() {
        let id = Uuid::new_v4();
        let value = Uuid::new_v4().to_string();
        let blob = protect(id, &value).unwrap();
        assert!(blob != value.as_bytes());
        assert!(blob != protect(id, &value).unwrap());
        assert!(unprotect(id, &blob).unwrap() == value);
        assert!(unprotect(Uuid::new_v4(), &blob).is_err());
        let mut corrupt = blob;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(unprotect(id, &corrupt).is_err());
        assert!(unprotect(id, &protect(id, "").unwrap()).unwrap().is_empty());
    }
}
