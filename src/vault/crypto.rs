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

    let tmp_path = path.with_extension("enc.tmp");
    fs::write(&tmp_path, &output).context("Failed to write vault file")?;
    fs::rename(&tmp_path, path).context("Failed to finalize vault file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(path, perms);
    }

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
}
