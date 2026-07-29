//! The encrypted secret store (US-10):
//! XChaCha20-Poly1305 values behind an `enc:v1:<base64(nonce‖ciphertext)>`
//! envelope, keyed by a random 32-byte project key under the user config dir
//! (`~/.config/proef/keys/default.key`, created on first use, `0600`).
//!
//! Modest by design — keeps plaintext out of the repository, not a defense
//! against a compromised host. Resolution order at run time:
//! `PROEF_SECRET_<NAME>` environment override → this store.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

/// The project-local store file (committed-safe: ciphertext only).
pub const STORE_FILE: &str = ".proef-secrets.json";

const PREFIX: &str = "enc:v1:";
const NONCE_LEN: usize = 24;

/// Encrypt `plaintext` into an `enc:v1:` token (fresh random nonce per call).
pub fn encrypt(plaintext: &str, key: &[u8; 32]) -> String {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let Ok(ciphertext) = cipher.encrypt(&nonce, plaintext.as_bytes()) else {
        // Only fails on allocation failure — unrecoverable either way.
        unreachable!("XChaCha20-Poly1305 encryption cannot fail on valid input");
    };
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    )
}

/// Decrypt an `enc:v1:` token (authentication failure = wrong key/tampering).
pub fn decrypt(token: &str, key: &[u8; 32]) -> Result<String, String> {
    let b64 = token
        .strip_prefix(PREFIX)
        .ok_or_else(|| "value is not enc:v1: encrypted".to_owned())?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|err| format!("invalid base64: {err}"))?;
    if blob.len() < NONCE_LEN {
        return Err("ciphertext too short".to_owned());
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let plaintext = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| "cannot decrypt — wrong key or corrupted value".to_owned())?;
    String::from_utf8(plaintext).map_err(|err| format!("invalid utf8: {err}"))
}

/// The key file path (`$PROEF_CONFIG_DIR`, else XDG config, else `~/.config`).
fn key_path() -> PathBuf {
    let config_dir = std::env::var_os("PROEF_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config_dir.join("proef").join("keys").join("default.key")
}

/// Load the project key, creating a fresh random one on first use (`0600`).
pub fn load_or_create_key() -> Result<[u8; 32], String> {
    let path = key_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => decode_key(&path, &text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let key: [u8; 32] = XChaCha20Poly1305::generate_key(&mut OsRng).into();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(key);
            match write_private(&path, encoded.as_bytes()) {
                Ok(()) => {
                    eprintln!("created project key {}", path.display());
                    Ok(key)
                }
                // Lost the creation race: a concurrent proef won a moment
                // ago — its key is the project key, use it.
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let text = std::fs::read_to_string(&path)
                        .map_err(|err| format!("cannot read key file {}: {err}", path.display()))?;
                    decode_key(&path, &text)
                }
                Err(err) => Err(format!("cannot write key file {}: {err}", path.display())),
            }
        }
        Err(err) => Err(format!("cannot read key file {}: {err}", path.display())),
    }
}

fn decode_key(path: &Path, text: &str) -> Result<[u8; 32], String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|err| format!("key file {} is not base64: {err}", path.display()))?;
    bytes
        .try_into()
        .map_err(|_| format!("key file {} must hold 32 bytes", path.display()))
}

/// Create `path` private from the first byte: on unix the file is *opened*
/// with mode `0600` (no world-readable window, no separate chmod that could
/// silently fail); `create_new` because the caller just observed absence.
fn write_private(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(content)
}

/// Load the store (`name → enc:v1:token`); missing file = empty.
pub fn load_store() -> Result<BTreeMap<String, String>, String> {
    match std::fs::read_to_string(STORE_FILE) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|err| format!("{STORE_FILE} is invalid: {err}"))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(format!("cannot read {STORE_FILE}: {err}")),
    }
}

/// Serialize store mutations across processes: an exclusive advisory lock
/// on a sibling `.lock` file. The lock file (not the store itself) carries
/// the lock because [`save_store`] renames over the store — a lock on the
/// old inode would not stop a writer opening the new one.
fn lock_store() -> Result<std::fs::File, String> {
    let path = format!("{STORE_FILE}.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|err| format!("cannot open {path}: {err}"))?;
    file.lock()
        .map_err(|err| format!("cannot lock {path}: {err}"))?;
    Ok(file)
}

fn save_store(store: &BTreeMap<String, String>) -> Result<(), String> {
    use std::io::Write as _;
    let json = serde_json::to_string_pretty(store)
        .map_err(|err| format!("cannot serialize store: {err}"))?;
    // Temp + rename: an interrupt mid-write must never corrupt the store,
    // and readers always see a complete file. Private from the first byte.
    let tmp = format!("{STORE_FILE}.{}.tmp", std::process::id());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&tmp)
        .and_then(|mut file| file.write_all(format!("{json}\n").as_bytes()))
        .map_err(|err| format!("cannot write {tmp}: {err}"))?;
    std::fs::rename(&tmp, STORE_FILE)
        .map_err(|err| format!("cannot move {tmp} into place: {err}"))?;
    // Ciphertext-only, but private anyway (TECH-SPEC §13 discipline) — the
    // mode at open only applies on creation, not to a reused temp file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(STORE_FILE, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("cannot set permissions on {STORE_FILE}: {err}"))?;
    }
    Ok(())
}

/// `proef secret set NAME [--value V]` — value via hidden prompt when absent.
pub fn set(name: &str, value: Option<&str>) -> Result<(), String> {
    let value = match value {
        Some(value) => value.to_owned(),
        None => rpassword::prompt_password(format!("value for `{name}` (hidden): "))
            .map_err(|err| format!("cannot read value: {err}"))?,
    };
    let key = load_or_create_key()?;
    // The whole read-modify-write is one critical section: concurrent
    // `secret set` calls must all land (released on drop).
    let _lock = lock_store()?;
    let mut store = load_store()?;
    store.insert(name.to_owned(), encrypt(&value, &key));
    save_store(&store)?;
    crate::render::outln!("secret `{name}` stored in {STORE_FILE} (ciphertext only)");
    Ok(())
}

/// `proef secret rm NAME` — remove a stored secret (same locked
/// read-modify-write as `set`). Removing an absent name is a user error:
/// a typo'd cleanup must not report success.
pub fn rm(name: &str) -> Result<(), String> {
    let _lock = lock_store()?;
    let mut store = load_store()?;
    if store.remove(name).is_none() {
        return Err(format!(
            "no secret named `{name}` in {STORE_FILE} (see `proef secret list`)"
        ));
    }
    save_store(&store)?;
    crate::render::outln!("secret `{name}` removed from {STORE_FILE}");
    Ok(())
}

/// `proef secret list` — names only, never values (ADR-0005).
pub fn list() -> Result<(), String> {
    let store = load_store()?;
    if store.is_empty() {
        crate::render::outln!("no secrets stored ({STORE_FILE})");
    }
    for name in store.keys() {
        crate::render::outln!("{name}");
    }
    Ok(())
}

/// Decrypt the stored value for `name`, if present.
pub fn resolve(name: &str) -> Result<Option<String>, String> {
    let store = load_store()?;
    let Some(token) = store.get(name) else {
        return Ok(None);
    };
    let key = load_or_create_key()?;
    decrypt(token, &key).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_rejection() {
        let key = [7u8; 32];
        let token = encrypt("hunter2", &key);
        assert!(token.starts_with("enc:v1:"));
        assert_eq!(decrypt(&token, &key).as_deref(), Ok("hunter2"));

        let wrong = [8u8; 32];
        assert!(decrypt(&token, &wrong).is_err(), "wrong key must fail auth");
        assert!(decrypt("enc:v1:AAAA", &key).is_err(), "short blob rejected");
        assert!(decrypt("plaintext", &key).is_err(), "unprefixed rejected");
    }

    #[test]
    fn distinct_nonces_per_encryption() {
        let key = [7u8; 32];
        assert_ne!(encrypt("same", &key), encrypt("same", &key));
    }
}
