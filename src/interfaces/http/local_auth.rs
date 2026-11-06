//! Per-installation bearer credentials for the optional local HTTP transport.
//!
//! This module does not authorize tasks; callers must also enforce their task
//! authorization policy. Never include the returned bearer token in a URL or log.

use std::fmt;
use subtle::ConstantTimeEq;

const TOKEN_LEN: usize = 64;

/// An installation secret. Deliberately does not implement `Debug` or `Display`.
pub struct LocalCredential {
    token: String,
}

/// Credential-store failures are deliberately reported without secret material.
#[derive(Debug)]
pub struct CredentialError;

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("local credential store unavailable or credential invalid")
    }
}

impl std::error::Error for CredentialError {}

impl LocalCredential {
    /// Load the installation credential, creating it in the OS credential store
    /// if absent. A store failure, invalid existing item, or unsupported OS fails
    /// closed. Concurrent creators converge on the Keychain's existing item.
    pub fn load_or_create() -> Result<Self, CredentialError> {
        platform::load_or_create().map(|token| Self { token })
    }

    /// Compare a supplied bearer token against the installation credential.
    /// Length is public; equal-length contents are compared in constant time.
    pub fn authenticate(&self, token: &[u8]) -> bool {
        token.len() == TOKEN_LEN && bool::from(self.token.as_bytes().ct_eq(token))
    }

    /// Obtain the token for use in an Authorization header by a local client.
    /// Treat this value as secret; never put it in a URL, command line, or log.
    pub fn bearer_token(&self) -> &str {
        &self.token
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::CredentialError;

    pub(super) fn load_or_create() -> Result<String, CredentialError> {
        Err(CredentialError)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{CredentialError, TOKEN_LEN};
    use std::ffi::c_void;
    use std::ptr;

    // Stable application-specific identifiers, not secrets. The default
    // Keychain is selected by passing a null keychain/search list.
    const SERVICE: &[u8] = b"com.desk-agent.local-http";
    const ACCOUNT: &[u8] = b"installation-bearer-v1";
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecRandomCopyBytes(rnd: *const c_void, count: usize, bytes: *mut u8) -> i32;
        fn SecKeychainFindGenericPassword(
            keychain_or_array: *const c_void,
            service_len: u32,
            service: *const u8,
            account_len: u32,
            account: *const u8,
            password_len: *mut u32,
            password_data: *mut *mut c_void,
            item: *mut *mut c_void,
        ) -> i32;
        fn SecKeychainAddGenericPassword(
            keychain: *const c_void,
            service_len: u32,
            service: *const u8,
            account_len: u32,
            account: *const u8,
            password_len: u32,
            password_data: *const c_void,
            item: *mut *mut c_void,
        ) -> i32;
        fn SecKeychainItemFreeContent(attributes: *mut c_void, data: *mut c_void) -> i32;
    }

    enum Lookup {
        Found(String),
        Missing,
    }

    fn lookup() -> Result<Lookup, CredentialError> {
        let mut len = 0u32;
        let mut data = ptr::null_mut();
        // Security allocates password data; free it even when its content is invalid.
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null(), SERVICE.len() as u32, SERVICE.as_ptr(),
                ACCOUNT.len() as u32, ACCOUNT.as_ptr(), &mut len, &mut data,
                ptr::null_mut(),
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(Lookup::Missing);
        }
        if status != 0 || data.is_null() {
            return Err(CredentialError);
        }
        let token = if len as usize == TOKEN_LEN {
            // The Security API guarantees `len` readable bytes on success.
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), TOKEN_LEN) };
            if bytes.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)) {
                String::from_utf8(bytes.to_vec()).ok()
            } else {
                None
            }
        } else {
            None
        };
        let free_status = unsafe { SecKeychainItemFreeContent(ptr::null_mut(), data) };
        if free_status != 0 {
            return Err(CredentialError);
        }
        token.map(Lookup::Found).ok_or(CredentialError)
    }

    pub(super) fn load_or_create() -> Result<String, CredentialError> {
        match lookup()? {
            Lookup::Found(token) => return Ok(token),
            Lookup::Missing => {}
        }

        let mut random = [0u8; 32];
        // A null random source selects kSecRandomDefault.
        if unsafe { SecRandomCopyBytes(ptr::null(), random.len(), random.as_mut_ptr()) } != 0 {
            return Err(CredentialError);
        }
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = [0u8; TOKEN_LEN];
        for (i, byte) in random.iter().enumerate() {
            encoded[2 * i] = HEX[(byte >> 4) as usize];
            encoded[2 * i + 1] = HEX[(byte & 0x0f) as usize];
        }
        let status = unsafe {
            SecKeychainAddGenericPassword(
                ptr::null(), SERVICE.len() as u32, SERVICE.as_ptr(),
                ACCOUNT.len() as u32, ACCOUNT.as_ptr(), TOKEN_LEN as u32,
                encoded.as_ptr().cast(), ptr::null_mut(),
            )
        };
        match status {
            0 => String::from_utf8(encoded.to_vec()).map_err(|_| CredentialError),
            // Another process may have installed its own key after our lookup.
            ERR_SEC_DUPLICATE_ITEM => match lookup()? {
                Lookup::Found(token) => Ok(token),
                Lookup::Missing => Err(CredentialError),
            },
            _ => Err(CredentialError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticates_only_the_exact_token() {
        let credential = LocalCredential { token: "a".repeat(TOKEN_LEN) };
        assert!(credential.authenticate("a".repeat(TOKEN_LEN).as_bytes()));
        assert!(!credential.authenticate("b".repeat(TOKEN_LEN).as_bytes()));
        assert!(!credential.authenticate(b"a"));
        assert!(!credential.authenticate(&[0xff; TOKEN_LEN]));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_platform_fails_closed() {
        assert!(LocalCredential::load_or_create().is_err());
    }
}
