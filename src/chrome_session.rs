//! Reads the claude.ai `sessionKey` cookie from Chromium-based browsers on Windows.
//!
//! Chrome ≥v80 encrypts cookies with AES-256-GCM. The AES key lives in
//! `%LOCALAPPDATA%\<browser>\User Data\Local State` (itself DPAPI-encrypted).
//! Cookie ciphertext lives in the browser's SQLite `Cookies` database.
//!
//! NOTE: Chrome 127+ uses App-Bound Encryption on top of DPAPI, which requires
//! elevated privileges to bypass. This module works reliably with Edge and Brave
//! (which do not use App-Bound Encryption) and with older Chrome versions.
//!
//! On non-Windows platforms this module is a no-op stub that always returns `None`.

/// Returns the claude.ai `sessionKey` cookie value if readable from a
/// Chromium-based browser's encrypted cookie store. Windows-only.
pub fn read_claude_session_key() -> Option<String> {
    #[cfg(windows)]
    return imp::read_claude_session_key();
    #[cfg(not(windows))]
    return None;
}

#[cfg(windows)]
mod imp {
    use aes_gcm::{
        Aes256Gcm, Nonce,
        aead::{Aead, KeyInit},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use rusqlite::{Connection, OpenFlags};
    use std::path::{Path, PathBuf};
    use tracing::debug;

    /// Chromium browser data directories relative to `%LOCALAPPDATA%`.
    /// Edge and Brave are tried first because they lack Chrome 127+ App-Bound Encryption.
    const BROWSER_DIRS: &[&str] = &[
        r"Microsoft\Edge\User Data",
        r"BraveSoftware\Brave-Browser\User Data",
        r"Google\Chrome\User Data",
    ];

    /// Cookie DB paths relative to the browser data directory (newest first).
    const COOKIES_SUBPATHS: &[&str] = &[
        r"Default\Network\Cookies", // Chrome/Edge 96+
        r"Default\Cookies",         // older versions
    ];

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *const core::ffi::c_void,
            p_prompt_struct: *const core::ffi::c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(h_mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    }

    fn dpapi_decrypt(data: &[u8]) -> Option<Vec<u8>> {
        let input = DataBlob {
            cb_data: data.len() as u32,
            pb_data: data.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        unsafe {
            let ok = CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut output,
            );
            if ok == 0 {
                debug!("chrome_session: CryptUnprotectData failed");
                return None;
            }
            let result =
                std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
            LocalFree(output.pb_data as *mut core::ffi::c_void);
            Some(result)
        }
    }

    fn get_chrome_aes_key(browser_dir: &Path) -> Option<Vec<u8>> {
        let local_state_path = browser_dir.join("Local State");
        let raw = std::fs::read_to_string(&local_state_path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&raw).ok()?;

        let b64 = json.get("os_crypt")?.get("encrypted_key")?.as_str()?;

        let encrypted = STANDARD.decode(b64).ok()?;

        // Chrome prepends the literal ASCII bytes "DPAPI" (5 bytes) before the ciphertext.
        if encrypted.len() <= 5 || &encrypted[..5] != b"DPAPI" {
            debug!("chrome_session: unexpected encrypted_key prefix (App-Bound?)");
            return None;
        }

        let key = dpapi_decrypt(&encrypted[5..])?;
        if key.len() != 32 {
            debug!(len = key.len(), "chrome_session: unexpected AES key length");
            return None;
        }
        Some(key)
    }

    fn decrypt_cookie_value(aes_key: &[u8], encrypted: &[u8]) -> Option<String> {
        // Modern format: b"v10" | 12-byte nonce | ciphertext+tag (AES-256-GCM)
        // v11 uses the same layout with App-Bound key (we can still try to decrypt).
        if encrypted.len() >= 15 && (encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11"))
        {
            let nonce_bytes = encrypted.get(3..15)?;
            let ciphertext_and_tag = encrypted.get(15..)?;

            let key = aes_gcm::Key::<Aes256Gcm>::from_slice(aes_key);
            let cipher = Aes256Gcm::new(key);
            let nonce = Nonce::from_slice(nonce_bytes);

            if let Ok(plaintext) = cipher.decrypt(nonce, ciphertext_and_tag) {
                return String::from_utf8(plaintext).ok();
            }
            return None;
        }

        // Legacy unencrypted value (very old Chromium), raw UTF-8.
        if !encrypted.is_empty() {
            return String::from_utf8(encrypted.to_vec()).ok();
        }

        None
    }

    fn read_session_key_from_db(cookies_path: &Path, aes_key: &[u8]) -> Option<String> {
        let opened = open_cookies_db(cookies_path)?;
        let conn = &opened.connection;

        for host in [".claude.ai", "claude.ai"] {
            let result = conn.query_row(
                "SELECT encrypted_value FROM cookies \
                 WHERE host_key = ?1 AND name = 'sessionKey' LIMIT 1",
                [host],
                |row| row.get::<_, Vec<u8>>(0),
            );
            if let Ok(encrypted) = result
                && let Some(value) = decrypt_cookie_value(aes_key, &encrypted)
                && !value.is_empty()
            {
                return Some(value);
            }
        }
        None
    }

    /// Fields drop in order, so the connection closes before the copy folder goes.
    struct CookiesDb {
        connection: Connection,
        _copy: Option<tempfile::TempDir>,
    }

    fn open_cookies_db(path: &Path) -> Option<CookiesDb> {
        if let Ok(connection) = crate::foreign_sqlite::open_read_only(path) {
            return Some(CookiesDb {
                connection,
                _copy: None,
            });
        }

        // A running browser holds the file exclusively on Windows. Read a
        // private copy in Pulse's cache folder, removed as soon as this read ends.
        let cache = crate::storage::cache_path("");
        std::fs::create_dir_all(&cache).ok()?;
        let copy = tempfile::Builder::new()
            .prefix("cookies-")
            .tempdir_in(cache)
            .ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        std::fs::copy(path, copy.path().join(&name)).ok()?;
        for suffix in ["-wal", "-shm"] {
            let source = path.with_file_name(format!("{name}{suffix}"));
            if source.exists() {
                let _ = std::fs::copy(&source, copy.path().join(format!("{name}{suffix}")));
            }
        }
        let connection =
            Connection::open_with_flags(copy.path().join(&name), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .ok()?;
        Some(CookiesDb {
            connection,
            _copy: Some(copy),
        })
    }

    pub fn read_claude_session_key() -> Option<String> {
        let local_appdata = match std::env::var("LOCALAPPDATA") {
            Ok(v) => v,
            Err(_) => return None,
        };

        for browser_rel in BROWSER_DIRS {
            let browser_dir = PathBuf::from(&local_appdata).join(browser_rel);
            if !browser_dir.exists() {
                continue;
            }

            let aes_key = match get_chrome_aes_key(&browser_dir) {
                Some(k) => k,
                None => {
                    debug!(
                        dir = %browser_dir.display(),
                        "chrome_session: skipping browser (AES key unavailable)"
                    );
                    continue;
                }
            };

            for cookies_sub in COOKIES_SUBPATHS {
                let cookies_path = browser_dir.join(cookies_sub);
                if !cookies_path.exists() {
                    continue;
                }
                if let Some(key) = read_session_key_from_db(&cookies_path, &aes_key) {
                    debug!(browser = browser_rel, "chrome_session: sessionKey found");
                    return Some(key);
                }
            }
        }

        debug!("chrome_session: sessionKey not found in any Chromium browser");
        None
    }
}
