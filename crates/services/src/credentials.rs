//! Passwords are stored in the current Windows user's Credential Manager.
use uuid::Uuid;
#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{ERROR_NOT_FOUND, GetLastError},
        Security::Credentials::*,
    };
    fn target(id: Uuid) -> Vec<u16> {
        format!("TurboDbNote/MySQL/{id}\0").encode_utf16().collect()
    }
    pub fn write(id: Uuid, password: &str) -> Result<(), String> {
        if password.len() > 2560 {
            return Err("密码超过系统凭据长度限制".into());
        }
        let mut name = target(id);
        let mut bytes = password.as_bytes().to_vec();
        // SAFETY: buffers outlive CredWriteW; credential contains only owned buffers.
        let success = unsafe {
            let mut credential: CREDENTIALW = std::mem::zeroed();
            credential.Type = CRED_TYPE_GENERIC;
            credential.TargetName = name.as_mut_ptr();
            credential.CredentialBlobSize = bytes.len() as u32;
            credential.CredentialBlob = bytes.as_mut_ptr();
            credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&credential, 0)
        };
        bytes.fill(0);
        if success == 0 {
            Err("无法保存 Windows 凭据".into())
        } else {
            Ok(())
        }
    }
    pub fn read(id: Uuid) -> Result<Option<String>, String> {
        let name = target(id);
        let mut credential = std::ptr::null_mut();
        // SAFETY: Windows allocates the credential; it is freed after copying the blob.
        unsafe {
            if CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                return if GetLastError() == ERROR_NOT_FOUND {
                    Ok(None)
                } else {
                    Err("无法读取 Windows 凭据".into())
                };
            }
            let result = if (*credential).CredentialBlobSize == 0 {
                Ok(Some(String::new()))
            } else {
                let bytes = std::slice::from_raw_parts(
                    (*credential).CredentialBlob,
                    (*credential).CredentialBlobSize as usize,
                );
                String::from_utf8(bytes.to_vec())
                    .map(Some)
                    .map_err(|_| "凭据编码无效".into())
            };
            CredFree(credential as _);
            result
        }
    }
    pub fn delete(id: Uuid) -> Result<(), String> {
        let name = target(id);
        // SAFETY: name is a valid NUL-terminated target identifier.
        unsafe {
            if CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) == 0
                && GetLastError() != ERROR_NOT_FOUND
            {
                return Err("无法删除 Windows 凭据".into());
            }
        }
        Ok(())
    }
}
#[cfg(not(windows))]
mod platform {
    use super::*;
    pub fn write(_: Uuid, _: &str) -> Result<(), String> {
        Err("当前平台暂不支持凭据保存，请使用临时密码".into())
    }
    pub fn read(_: Uuid) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn delete(_: Uuid) -> Result<(), String> {
        Ok(())
    }
}
pub use platform::{delete, read, write};

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn windows_credential_round_trip() {
        let id = Uuid::new_v4();
        assert!(read(id).unwrap().is_none());
        write(id, "test-only-临时密码").unwrap();
        let result = read(id);
        let cleanup = delete(id);
        cleanup.unwrap();
        assert_eq!(result.unwrap().as_deref(), Some("test-only-临时密码"));
        assert!(read(id).unwrap().is_none());
    }
}
