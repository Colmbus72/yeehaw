use std::fs;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Argon2, Algorithm, Version, Params};
use rand::RngCore;

use anyhow::{Context, Result, bail};

use crate::types::Vault;

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Create an Argon2id instance with explicit OWASP recommended params.
fn argon2_instance() -> Argon2<'static> {
    // OWASP recommended minimums: 19456 KiB memory, 2 iterations, 1 lane
    let params = Params::new(19456, 2, 1, Some(KEY_LEN))
        .expect("valid argon2 params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Derive a 256-bit key from a master password and salt using Argon2id.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    let argon2 = argon2_instance();
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("Key derivation failed: {}", e))?;
    Ok(key)
}

/// Create a new encrypted vault file with an empty vault.
pub fn create_vault(path: &Path, master_password: &str) -> Result<()> {
    let vault = Vault::new();
    save_vault(path, &vault, master_password)
}

/// Decrypt and deserialize a vault file.
/// Returns Err if the password is wrong (AES-GCM auth tag fails).
pub fn unlock_vault(path: &Path, master_password: &str) -> Result<Vault> {
    let data = fs::read(path).context("Failed to read vault file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path).context("Failed to read vault metadata")?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o600 {
            eprintln!("Warning: vault file has permissions {:o}, expected 600", mode);
        }
    }

    if data.len() < SALT_LEN + NONCE_LEN + 1 {
        bail!("Vault file is corrupted (too small)");
    }

    let salt = &data[..SALT_LEN];
    let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &data[SALT_LEN + NONCE_LEN..];

    let key = derive_key(master_password, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("Wrong master password"))?;

    let yaml_str = String::from_utf8(plaintext)
        .context("Decrypted data is not valid UTF-8")?;

    let vault: Vault = serde_yaml::from_str(&yaml_str)
        .context("Failed to parse vault data")?;

    Ok(vault)
}

/// Serialize and encrypt a vault, writing it to the file.
/// Fresh nonce on every save. Reuses existing salt or generates new.
pub fn save_vault(path: &Path, vault: &Vault, master_password: &str) -> Result<()> {
    let yaml_str = serde_yaml::to_string(vault)
        .context("Failed to serialize vault")?;

    let salt = if path.exists() {
        let existing = fs::read(path).context("Failed to read existing vault")?;
        if existing.len() >= SALT_LEN {
            let mut s = [0u8; SALT_LEN];
            s.copy_from_slice(&existing[..SALT_LEN]);
            s
        } else {
            let mut s = [0u8; SALT_LEN];
            OsRng.fill_bytes(&mut s);
            s
        }
    } else {
        let mut s = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut s);
        s
    };

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(master_password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, yaml_str.as_bytes())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    let mut output = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    // The vault is plaintext credentials once decrypted, so it is written
    // through the store's atomic-write helper rather than by hand: a temp name
    // that is unique per call (no collision between two overlapping saves), an
    // fsync of the contents, and 0600 applied to the temp file *before* the
    // rename — so the file is never reachable at its real name with anything
    // laxer than owner-only.
    crate::store::write_atomic_with_mode(path, &output, 0o600)
        .context("Failed to write vault file")?;

    Ok(())
}

/// Change the master password: re-encrypts with new salt and nonce.
pub fn change_master_password(path: &Path, old_password: &str, new_password: &str) -> Result<()> {
    let vault = unlock_vault(path, old_password)?;
    // Write to temp path first to avoid data loss if save fails
    let tmp_path = path.with_extension("rekey.tmp");
    save_vault(&tmp_path, &vault, new_password)?;
    fs::rename(&tmp_path, path).context("Failed to finalize re-keyed vault")?;
    Ok(())
}

/// Check if the vault file exists.
pub fn vault_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_and_unlock_vault() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");

        create_vault(&path, "testpass123").unwrap();
        assert!(path.exists());

        let vault = unlock_vault(&path, "testpass123").unwrap();
        assert!(vault.entries.is_empty());
    }

    #[test]
    fn test_wrong_password_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");

        create_vault(&path, "correct").unwrap();
        let result = unlock_vault(&path, "wrong");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Wrong master password"));
    }

    #[test]
    fn test_save_and_load_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");

        let mut vault = Vault::new();
        vault.entries.push(crate::types::VaultEntry {
            id: "test-id".to_string(),
            name: "Test Entry".to_string(),
            username: Some("user@example.com".to_string()),
            password: "secret123".to_string(),
            notes: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        });

        save_vault(&path, &vault, "mypass").unwrap();
        let loaded = unlock_vault(&path, "mypass").unwrap();

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].name, "Test Entry");
        assert_eq!(loaded.entries[0].password, "secret123");
        assert_eq!(loaded.entries[0].username.as_deref(), Some("user@example.com"));
    }

    #[test]
    fn test_change_master_password() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");

        let mut vault = Vault::new();
        vault.entries.push(crate::types::VaultEntry {
            id: "id1".to_string(),
            name: "Entry".to_string(),
            username: None,
            password: "pw".to_string(),
            notes: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        });
        save_vault(&path, &vault, "old").unwrap();

        change_master_password(&path, "old", "new").unwrap();

        assert!(unlock_vault(&path, "old").is_err());
        let loaded = unlock_vault(&path, "new").unwrap();
        assert_eq!(loaded.entries.len(), 1);
    }

    /// The vault holds plaintext credentials. It must be 0600 the moment the
    /// name resolves — not 0600 shortly after a rename published it at the
    /// default mode.
    #[cfg(unix)]
    #[test]
    fn saved_vault_is_owner_only_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");

        create_vault(&path, "pw").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "vault mode is {:o}, expected 600", mode);

        let names: Vec<String> = fs::read_dir(dir.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["vault.enc".to_string()], "stray temp file: {:?}", names);
    }

    /// A resave must keep 0600 even when the target already exists at a laxer
    /// mode — `rename` carries the *temp file's* mode onto the target, so the
    /// mode has to be set on the temp file rather than inherited.
    #[cfg(unix)]
    #[test]
    fn resaving_a_world_readable_vault_restores_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");
        create_vault(&path, "pw").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let vault = unlock_vault(&path, "pw").unwrap();
        save_vault(&path, &vault, "pw").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "vault mode is {:o}, expected 600", mode);
    }

    /// Two saves of the same vault path must both succeed. A fixed temp name
    /// (`vault.enc.tmp`) makes them collide: both write the same temp file,
    /// the first rename moves it away, and the second fails with ENOENT — or
    /// worse, the two interleaved writes are renamed into place as a corrupt
    /// vault. Correct behavior is atomic last-wins.
    #[test]
    fn concurrent_saves_of_one_vault_all_succeed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");
        create_vault(&path, "pw").unwrap();

        const THREADS: usize = 4;
        let mut handles = Vec::new();
        for t in 0..THREADS {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                let mut vault = Vault::new();
                vault.entries.push(crate::types::VaultEntry {
                    id: format!("id-{}", t),
                    name: format!("entry-{}", t),
                    username: None,
                    password: "pw".to_string(),
                    notes: None,
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    updated_at: "2026-01-01T00:00:00Z".to_string(),
                });
                save_vault(&path, &vault, "pw")
                    .unwrap_or_else(|e| panic!("thread {} save failed: {:?}", t, e));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // The survivor must be exactly one writer's vault, never a mixture.
        let loaded = unlock_vault(&path, "pw").expect("surviving vault must decrypt");
        assert_eq!(loaded.entries.len(), 1);
        assert!(
            (0..THREADS).any(|t| loaded.entries[0].name == format!("entry-{}", t)),
            "unexpected surviving entry: {}",
            loaded.entries[0].name
        );

        let names: Vec<String> = fs::read_dir(dir.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["vault.enc".to_string()], "stray temp file: {:?}", names);
    }

    /// The temp file's name must be unique per call, not a fixed
    /// `<target>.enc.tmp` derived from the target. Two saves of one vault that
    /// overlap would otherwise build the *same* temp path: they interleave
    /// their bytes into one file, and the second rename fails with ENOENT
    /// after the first has already moved it away.
    ///
    /// That race is far too narrow to provoke on demand — Argon2 dominates
    /// every save, so two threads are essentially never inside the few
    /// microseconds between write and rename together. So the property is
    /// pinned the way `store.rs` pins it: occupy the predictable name and
    /// require the save to be unaffected by it. A save that still insists on
    /// `vault.enc.tmp` cannot get past this.
    #[test]
    fn saving_does_not_depend_on_a_predictable_temp_name() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("vault.enc");

        // Squat on the name the hand-rolled writer used.
        fs::create_dir(dir.path().join("vault.enc.tmp")).unwrap();

        create_vault(&path, "pw").expect("save must not collide with vault.enc.tmp");
        let loaded = unlock_vault(&path, "pw").unwrap();
        assert!(loaded.entries.is_empty());
    }
}
