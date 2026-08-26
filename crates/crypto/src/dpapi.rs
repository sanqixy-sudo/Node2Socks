use crate::SecretKey;
use node2socks_domain::{AppError, AppResult, ErrorCode};

#[cfg(windows)]
pub fn protect_key(key: &SecretKey) -> AppResult<Vec<u8>> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData},
    };
    let mut bytes = key.expose_for_platform_protection();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            windows::core::w!("Node2Socks local master key"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(dpapi_error)?;
        let protected = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(protected)
    }
}

#[cfg(windows)]
pub fn unprotect_key(protected: &[u8]) -> AppResult<SecretKey> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };
    let mut input_bytes = protected.to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: input_bytes.len() as u32,
        pbData: input_bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(dpapi_error)?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        if bytes.len() != 32 {
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            return Err(AppError::new(
                ErrorCode::CryptoError,
                "DPAPI key has invalid length",
            ));
        }
        let mut key = [0_u8; 32];
        key.copy_from_slice(bytes);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(SecretKey::from_bytes(key))
    }
}

#[cfg(not(windows))]
pub fn protect_key(_: &SecretKey) -> AppResult<Vec<u8>> {
    Err(AppError::new(
        ErrorCode::CryptoError,
        "DPAPI is Windows-only",
    ))
}
#[cfg(not(windows))]
pub fn unprotect_key(_: &[u8]) -> AppResult<SecretKey> {
    Err(AppError::new(
        ErrorCode::CryptoError,
        "DPAPI is Windows-only",
    ))
}
#[cfg(windows)]
fn dpapi_error(error: windows::core::Error) -> AppError {
    AppError::new(ErrorCode::CryptoError, error.to_string())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn dpapi_roundtrip_is_bound_to_current_windows_user() {
        let key = SecretKey::random();
        let fingerprint = key.fingerprint();
        let protected = protect_key(&key).unwrap();
        assert!(
            !protected
                .windows(4)
                .any(|v| v == &key.expose_for_platform_protection()[..4])
        );
        assert_eq!(
            unprotect_key(&protected).unwrap().fingerprint(),
            fingerprint
        )
    }
}
