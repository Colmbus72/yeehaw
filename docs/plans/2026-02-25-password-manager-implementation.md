# Password Manager (Vault) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Encrypted password vault accessible via Ctrl+P from anywhere in a Yeehaw tmux session, with context injection and clipboard support.

**Architecture:** New `vault` module handles crypto (Argon2id + AES-256-GCM) and entry management. New `VaultView` follows existing view pattern (action enum + sub-view struct). Tmux-level keybinding writes a trigger file; TUI event loop detects it and opens the vault.

**Tech Stack:** `argon2` + `aes-gcm` for encryption, `uuid` for entry IDs, `rand` for salt/nonce, `ratatui` for view rendering

---

### Task 1: Add dependencies and scaffold vault module

**Files:**
- Modify: `Cargo.toml`
- Create: `src/vault/mod.rs`
- Modify: `src/main.rs`

**Step 1: Add crypto dependencies to Cargo.toml**

Add these lines after the existing `regex` dependency in `Cargo.toml`:

```toml
# Vault encryption
argon2 = "0.5"
aes-gcm = "0.10"
uuid = { version = "1", features = ["v4"] }
rand = "0.8"
```

**Step 2: Create vault module scaffold**

Create `src/vault/mod.rs`:

```rust
pub mod crypto;
pub mod generator;
```

**Step 3: Register vault module in main.rs**

Add after `mod update_check;` (line 19):

```rust
mod vault;
```

**Step 4: Verify it compiles**

Run: `cargo check`
Expected: Errors about missing `crypto` and `generator` modules (that's fine, they come next)

**Step 5: Commit**

```bash
git add Cargo.toml src/vault/mod.rs src/main.rs
git commit -m "feat(vault): add crypto dependencies and scaffold vault module"
```

---

### Task 2: Vault types

**Files:**
- Modify: `src/types.rs`

**Step 1: Add VaultEntry and Vault structs**

Add at the end of `src/types.rs` (after the last struct/enum, before any impl blocks):

```rust
// ============================================================================
// Vault Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub entries: Vec<VaultEntry>,
}

impl Vault {
    pub fn new() -> Self {
        Self { entries: vec![] }
    }
}
```

**Step 2: Add AppView::Vault variant**

Add to the `AppView` enum (after the `NightSky` variant at line 384):

```rust
    Vault { source_pane: Option<String> },
```

**Step 3: Verify it compiles**

Run: `cargo check`
Expected: Warnings about non-exhaustive patterns in `app.rs` match statements (expected — we'll handle those in Task 7)

**Step 4: Commit**

```bash
git add src/types.rs
git commit -m "feat(vault): add VaultEntry, Vault, and AppView::Vault types"
```

---

### Task 3: Vault crypto module

**Files:**
- Create: `src/vault/crypto.rs`

**Step 1: Implement the crypto module**

Create `src/vault/crypto.rs`:

```rust
use std::fs;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;

use anyhow::{Context, Result, bail};

use crate::types::Vault;

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Derive a 256-bit key from a master password and salt using Argon2id.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    let argon2 = Argon2::default(); // Argon2id with recommended params
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

/// Decrypt and deserialize a vault file. Returns the vault entries.
/// Returns Err if the password is wrong (AES-GCM auth tag fails).
pub fn unlock_vault(path: &Path, master_password: &str) -> Result<Vault> {
    let data = fs::read(path).context("Failed to read vault file")?;

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
/// Generates a fresh nonce on every save. Reuses the existing salt if the file
/// exists, otherwise generates a new salt.
pub fn save_vault(path: &Path, vault: &Vault, master_password: &str) -> Result<()> {
    let yaml_str = serde_yaml::to_string(vault)
        .context("Failed to serialize vault")?;

    // Reuse existing salt or generate new
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

    // Fresh nonce every save
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(master_password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, yaml_str.as_bytes())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    // Assemble file: salt || nonce || ciphertext
    let mut output = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    // Write atomically: write to temp file then rename
    let tmp_path = path.with_extension("enc.tmp");
    fs::write(&tmp_path, &output).context("Failed to write vault file")?;
    fs::rename(&tmp_path, path).context("Failed to finalize vault file")?;

    // Set file permissions to 0600 (owner read/write only)
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

    // Delete existing file so save_vault generates a fresh salt
    let _ = fs::remove_file(path);
    save_vault(path, &vault, new_password)
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
```

**Step 2: Add tempfile as dev dependency for tests**

Add to `Cargo.toml` at the end:

```toml
[dev-dependencies]
tempfile = "3"
```

**Step 3: Run the tests**

Run: `cargo test vault::crypto`
Expected: All 4 tests pass

**Step 4: Commit**

```bash
git add src/vault/crypto.rs Cargo.toml
git commit -m "feat(vault): implement Argon2id + AES-256-GCM crypto module with tests"
```

---

### Task 4: Password generator

**Files:**
- Create: `src/vault/generator.rs`

**Step 1: Implement the password generator**

Create `src/vault/generator.rs`:

```rust
use rand::Rng;

const LOWERCASE: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const UPPERCASE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const SYMBOLS: &[u8] = b"!@#$%^&*-_+=";

/// Generate a random password of the given length.
/// Guarantees at least one character from each character class.
pub fn generate_password(length: usize) -> String {
    let length = length.max(4); // minimum 4 to fit all classes
    let mut rng = rand::thread_rng();

    let all_chars: Vec<u8> = [LOWERCASE, UPPERCASE, DIGITS, SYMBOLS].concat();

    // Start with one from each class to guarantee coverage
    let mut password: Vec<u8> = vec![
        LOWERCASE[rng.gen_range(0..LOWERCASE.len())],
        UPPERCASE[rng.gen_range(0..UPPERCASE.len())],
        DIGITS[rng.gen_range(0..DIGITS.len())],
        SYMBOLS[rng.gen_range(0..SYMBOLS.len())],
    ];

    // Fill the rest randomly from all classes
    for _ in 4..length {
        password.push(all_chars[rng.gen_range(0..all_chars.len())]);
    }

    // Shuffle to avoid predictable positions
    for i in (1..password.len()).rev() {
        let j = rng.gen_range(0..=i);
        password.swap(i, j);
    }

    String::from_utf8(password).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_password_length() {
        let pw = generate_password(20);
        assert_eq!(pw.len(), 20);
    }

    #[test]
    fn test_generate_password_has_all_classes() {
        // Run a few times to be statistically confident
        for _ in 0..10 {
            let pw = generate_password(20);
            assert!(pw.chars().any(|c| c.is_ascii_lowercase()), "missing lowercase");
            assert!(pw.chars().any(|c| c.is_ascii_uppercase()), "missing uppercase");
            assert!(pw.chars().any(|c| c.is_ascii_digit()), "missing digit");
            assert!(pw.chars().any(|c| "!@#$%^&*-_+=".contains(c)), "missing symbol");
        }
    }

    #[test]
    fn test_generate_password_minimum_length() {
        let pw = generate_password(2); // should be clamped to 4
        assert_eq!(pw.len(), 4);
    }
}
```

**Step 2: Run the tests**

Run: `cargo test vault::generator`
Expected: All 3 tests pass

**Step 3: Commit**

```bash
git add src/vault/generator.rs
git commit -m "feat(vault): add password generator with guaranteed character class coverage"
```

---

### Task 5: Config paths and vault trigger file

**Files:**
- Modify: `src/config.rs`

**Step 1: Add vault path functions**

Add after the `poll_state_dir()` function (around line 32) in `src/config.rs`:

```rust
pub fn vault_file() -> PathBuf { yeehaw_dir().join("vault.enc") }
pub fn vault_trigger_file() -> PathBuf { yeehaw_dir().join("vault-trigger") }
```

**Step 2: Verify it compiles**

Run: `cargo check`
Expected: Success (with existing warnings about non-exhaustive AppView patterns)

**Step 3: Commit**

```bash
git add src/config.rs
git commit -m "feat(vault): add vault file and trigger file paths to config"
```

---

### Task 6: Tmux keybinding for Ctrl+P

**Files:**
- Modify: `src/tmux.rs`

**Step 1: Add Ctrl+P binding to the tmux config**

In `src/tmux.rs`, inside the `generate_tmux_config()` function, find the Yeehaw keybindings block (around line 86-89):

```
# Yeehaw keybindings
bind-key -n C-y select-window -t :0    # Return to dashboard
bind-key -n C-h previous-window        # Go left one window
bind-key -n C-l next-window            # Go right one window
```

Add after `bind-key -n C-l next-window`:

```
bind-key -n C-p run-shell 'echo "#{pane_id}" > ~/.yeehaw/vault-trigger' \; select-window -t :0  # Password vault
```

**Step 2: Update the status bar right text to include the vault hint**

In the same function, find the status-right line (around line 95):

```
set -g status-right " Ctrl-y: dashboard "
```

Change it to:

```
set -g status-right " C-p: vault  C-y: dashboard "
```

**Step 3: Verify it compiles**

Run: `cargo check`
Expected: Success

**Step 4: Commit**

```bash
git add src/tmux.rs
git commit -m "feat(vault): register Ctrl+P tmux keybinding for password vault"
```

---

### Task 7: Vault view

**Files:**
- Create: `src/views/vault_view.rs`
- Modify: `src/views/mod.rs`

**Step 1: Create the vault view**

Create `src/views/vault_view.rs`:

```rust
use std::time::Instant;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Clear};

use crate::components::text_input::TextInput;
use crate::components::panel::Panel;
use crate::components::list::{self, ListItem, ListState};
use crate::types::{Vault, VaultEntry};
use crate::vault::generator;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);
const IDLE_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, PartialEq)]
pub enum VaultMode {
    Creating,        // First-time: enter master password
    CreatingConfirm, // First-time: confirm master password
    Locked,          // Enter master password to unlock
    Unlocked,        // Entry list
    Adding,          // New entry form
    Editing(usize),  // Editing entry at index
}

#[derive(Debug, Clone, PartialEq)]
pub enum VaultAction {
    None,
    Unlock(String),                  // master password
    CreateVault(String),             // master password
    InjectPassword(String),          // password to inject into source pane
    CopyPassword(String),            // password to copy to clipboard
    SaveEntry {
        name: String,
        username: Option<String>,
        password: String,
        notes: Option<String>,
        edit_index: Option<usize>,   // None = add, Some = update
    },
    DeleteEntry(usize),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FormField {
    Name,
    Username,
    Password,
    Notes,
}

impl FormField {
    fn next(self) -> Self {
        match self {
            FormField::Name => FormField::Username,
            FormField::Username => FormField::Password,
            FormField::Password => FormField::Notes,
            FormField::Notes => FormField::Name,
        }
    }

    fn prev(self) -> Self {
        match self {
            FormField::Name => FormField::Notes,
            FormField::Username => FormField::Name,
            FormField::Password => FormField::Username,
            FormField::Notes => FormField::Password,
        }
    }
}

pub struct VaultView {
    pub mode: VaultMode,
    pub entries: Vec<VaultEntry>,
    pub list_state: ListState,
    pub master_input: TextInput,
    pub confirm_input: TextInput,
    pub error: Option<String>,
    pub idle_timer: Instant,
    pub revealed: Option<usize>,
    pub search_query: String,
    pub searching: bool,

    // Form fields for add/edit
    form_field: FormField,
    name_input: TextInput,
    username_input: TextInput,
    password_input: TextInput,
    notes_input: TextInput,
    first_password: Option<String>, // stored during create flow
}

impl VaultView {
    pub fn new() -> Self {
        Self {
            mode: VaultMode::Locked,
            entries: vec![],
            list_state: ListState::new(),
            master_input: TextInput::new(""),
            confirm_input: TextInput::new(""),
            error: None,
            idle_timer: Instant::now(),
            revealed: None,
            search_query: String::new(),
            searching: false,
            form_field: FormField::Name,
            name_input: TextInput::new(""),
            username_input: TextInput::new(""),
            password_input: TextInput::new(""),
            notes_input: TextInput::new(""),
            first_password: None,
        }
    }

    pub fn reset_idle(&mut self) {
        self.idle_timer = Instant::now();
    }

    pub fn is_idle_expired(&self) -> bool {
        self.idle_timer.elapsed().as_secs() >= IDLE_TIMEOUT_SECS
    }

    pub fn set_entries(&mut self, entries: Vec<VaultEntry>) {
        self.entries = entries;
        self.list_state = ListState::new();
    }

    pub fn enter_creating(&mut self) {
        self.mode = VaultMode::Creating;
        self.master_input = TextInput::new("");
        self.error = None;
    }

    pub fn enter_locked(&mut self) {
        self.mode = VaultMode::Locked;
        self.master_input = TextInput::new("");
        self.entries.clear();
        self.revealed = None;
        self.error = None;
    }

    pub fn enter_unlocked(&mut self, entries: Vec<VaultEntry>) {
        self.mode = VaultMode::Unlocked;
        self.set_entries(entries);
        self.error = None;
        self.reset_idle();
    }

    fn enter_adding(&mut self) {
        self.mode = VaultMode::Adding;
        self.form_field = FormField::Name;
        self.name_input = TextInput::new("");
        self.username_input = TextInput::new("");
        self.password_input = TextInput::new("");
        self.notes_input = TextInput::new("");
    }

    fn enter_editing(&mut self, idx: usize) {
        if let Some(entry) = self.entries.get(idx) {
            self.mode = VaultMode::Editing(idx);
            self.form_field = FormField::Name;
            self.name_input = TextInput::new(&entry.name);
            self.username_input = TextInput::new(entry.username.as_deref().unwrap_or(""));
            self.password_input = TextInput::new(&entry.password);
            self.notes_input = TextInput::new(entry.notes.as_deref().unwrap_or(""));
        }
    }

    fn active_input(&mut self) -> &mut TextInput {
        match self.form_field {
            FormField::Name => &mut self.name_input,
            FormField::Username => &mut self.username_input,
            FormField::Password => &mut self.password_input,
            FormField::Notes => &mut self.notes_input,
        }
    }

    fn filtered_entries(&self) -> Vec<(usize, &VaultEntry)> {
        if self.search_query.is_empty() {
            self.entries.iter().enumerate().collect()
        } else {
            let q = self.search_query.to_lowercase();
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.name.to_lowercase().contains(&q)
                        || e.username.as_deref().unwrap_or("").to_lowercase().contains(&q)
                        || e.notes.as_deref().unwrap_or("").to_lowercase().contains(&q)
                })
                .collect()
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, modifiers: KeyModifiers) -> VaultAction {
        self.reset_idle();
        self.error = None;

        match &self.mode {
            VaultMode::Creating => {
                match key {
                    KeyCode::Esc => return VaultAction::Close,
                    KeyCode::Enter => {
                        let pw = self.master_input.value.clone();
                        if pw.is_empty() {
                            self.error = Some("Password cannot be empty".to_string());
                            return VaultAction::None;
                        }
                        self.first_password = Some(pw);
                        self.mode = VaultMode::CreatingConfirm;
                        self.confirm_input = TextInput::new("");
                        return VaultAction::None;
                    }
                    _ => { self.master_input.handle_input(key); }
                }
            }
            VaultMode::CreatingConfirm => {
                match key {
                    KeyCode::Esc => {
                        self.mode = VaultMode::Creating;
                        self.master_input = TextInput::new("");
                        self.first_password = None;
                        return VaultAction::None;
                    }
                    KeyCode::Enter => {
                        let confirm = self.confirm_input.value.clone();
                        if let Some(ref first) = self.first_password {
                            if confirm == *first {
                                let pw = first.clone();
                                self.first_password = None;
                                return VaultAction::CreateVault(pw);
                            } else {
                                self.error = Some("Passwords do not match".to_string());
                                self.confirm_input = TextInput::new("");
                                return VaultAction::None;
                            }
                        }
                    }
                    _ => { self.confirm_input.handle_input(key); }
                }
            }
            VaultMode::Locked => {
                match key {
                    KeyCode::Esc => return VaultAction::Close,
                    KeyCode::Enter => {
                        let pw = self.master_input.value.clone();
                        if pw.is_empty() {
                            return VaultAction::None;
                        }
                        return VaultAction::Unlock(pw);
                    }
                    _ => { self.master_input.handle_input(key); }
                }
            }
            VaultMode::Unlocked => {
                // Search mode
                if self.searching {
                    match key {
                        KeyCode::Esc => {
                            self.searching = false;
                            self.search_query.clear();
                        }
                        KeyCode::Enter => {
                            self.searching = false;
                        }
                        KeyCode::Backspace => {
                            self.search_query.pop();
                        }
                        KeyCode::Char(c) => {
                            self.search_query.push(c);
                        }
                        _ => {}
                    }
                    return VaultAction::None;
                }

                let filtered = self.filtered_entries();
                let filtered_len = filtered.len();
                let selected_real_idx = filtered.get(self.list_state.selected).map(|(i, _)| *i);

                match key {
                    KeyCode::Esc => return VaultAction::Close,
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.list_state.select_next(filtered_len);
                        self.revealed = None;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.list_state.select_prev();
                        self.revealed = None;
                    }
                    KeyCode::Char('g') => {
                        self.list_state.selected = 0;
                        self.revealed = None;
                    }
                    KeyCode::Char('G') => {
                        if filtered_len > 0 {
                            self.list_state.selected = filtered_len - 1;
                        }
                        self.revealed = None;
                    }
                    KeyCode::Char('n') => {
                        self.enter_adding();
                    }
                    KeyCode::Char('e') => {
                        if let Some(idx) = selected_real_idx {
                            self.enter_editing(idx);
                        }
                    }
                    KeyCode::Char('d') => {
                        if let Some(idx) = selected_real_idx {
                            return VaultAction::DeleteEntry(idx);
                        }
                    }
                    KeyCode::Char('i') => {
                        if let Some(idx) = selected_real_idx {
                            if let Some(entry) = self.entries.get(idx) {
                                return VaultAction::InjectPassword(entry.password.clone());
                            }
                        }
                    }
                    KeyCode::Char('c') => {
                        if let Some(idx) = selected_real_idx {
                            if let Some(entry) = self.entries.get(idx) {
                                return VaultAction::CopyPassword(entry.password.clone());
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(idx) = selected_real_idx {
                            self.revealed = if self.revealed == Some(idx) { None } else { Some(idx) };
                        }
                    }
                    KeyCode::Char('/') => {
                        self.searching = true;
                        self.search_query.clear();
                    }
                    _ => {}
                }
            }
            VaultMode::Adding | VaultMode::Editing(_) => {
                // Ctrl+G: generate password (only when on password field)
                if modifiers.contains(KeyModifiers::CONTROL) && key == KeyCode::Char('g') {
                    if self.form_field == FormField::Password {
                        let generated = generator::generate_password(20);
                        self.password_input = TextInput::new(&generated);
                        return VaultAction::None;
                    }
                }

                match key {
                    KeyCode::Esc => {
                        self.mode = VaultMode::Unlocked;
                    }
                    KeyCode::Tab => {
                        self.form_field = self.form_field.next();
                    }
                    KeyCode::BackTab => {
                        self.form_field = self.form_field.prev();
                    }
                    KeyCode::Enter => {
                        // Submit if on the last field or Ctrl+Enter from anywhere
                        let name = self.name_input.value.trim().to_string();
                        let password = self.password_input.value.trim().to_string();

                        if name.is_empty() {
                            self.error = Some("Name is required".to_string());
                            return VaultAction::None;
                        }
                        if password.is_empty() {
                            self.error = Some("Password is required".to_string());
                            return VaultAction::None;
                        }

                        let username = {
                            let v = self.username_input.value.trim().to_string();
                            if v.is_empty() { None } else { Some(v) }
                        };
                        let notes = {
                            let v = self.notes_input.value.trim().to_string();
                            if v.is_empty() { None } else { Some(v) }
                        };

                        let edit_index = match &self.mode {
                            VaultMode::Editing(idx) => Some(*idx),
                            _ => None,
                        };

                        return VaultAction::SaveEntry {
                            name,
                            username,
                            password,
                            notes,
                            edit_index,
                        };
                    }
                    _ => {
                        self.active_input().handle_input(key);
                    }
                }
            }
        }

        VaultAction::None
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Clear the area first
        frame.render_widget(Clear, area);

        // Outer border with title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BRAND_COLOR))
            .title(Span::styled(" Vault ", Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        match &self.mode {
            VaultMode::Creating | VaultMode::CreatingConfirm => {
                self.render_create(frame, inner);
            }
            VaultMode::Locked => {
                self.render_locked(frame, inner);
            }
            VaultMode::Unlocked => {
                self.render_unlocked(frame, inner);
            }
            VaultMode::Adding | VaultMode::Editing(_) => {
                self.render_form(frame, inner);
            }
        }
    }

    fn render_locked(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // spacer
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(2), // error
                Constraint::Min(0),   // filler
            ])
            .split(area);

        let title = Paragraph::new("Enter master password")
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[0]);

        let label = Paragraph::new("Password:")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[2]);

        // Render masked input
        self.render_masked_input(frame, chunks[3], &self.master_input);

        if let Some(ref err) = self.error {
            let err_text = Paragraph::new(err.as_str())
                .style(Style::default().fg(Color::Red));
            frame.render_widget(err_text, chunks[4]);
        }
    }

    fn render_create(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // spacer
                Constraint::Length(1), // label
                Constraint::Length(1), // input
                Constraint::Length(2), // error or hint
                Constraint::Min(0),
            ])
            .split(area);

        let (title_text, label_text) = match &self.mode {
            VaultMode::Creating => (
                "Create a new vault",
                "Choose a master password:",
            ),
            VaultMode::CreatingConfirm => (
                "Confirm master password",
                "Re-enter password:",
            ),
            _ => unreachable!(),
        };

        let title = Paragraph::new(title_text)
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[0]);

        let label = Paragraph::new(label_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(label, chunks[2]);

        let input = match &self.mode {
            VaultMode::CreatingConfirm => &self.confirm_input,
            _ => &self.master_input,
        };
        self.render_masked_input(frame, chunks[3], input);

        if let Some(ref err) = self.error {
            let err_text = Paragraph::new(err.as_str())
                .style(Style::default().fg(Color::Red));
            frame.render_widget(err_text, chunks[4]);
        }
    }

    fn render_unlocked(&mut self, frame: &mut Frame, area: Rect) {
        let filtered = self.filtered_entries();

        // Search bar area
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(if self.searching || !self.search_query.is_empty() { 1 } else { 0 }),
                Constraint::Min(1),
            ])
            .split(area);

        // Search bar
        if self.searching || !self.search_query.is_empty() {
            let search_text = if self.searching {
                format!("/ {}_", self.search_query)
            } else {
                format!("/ {} (Esc to clear)", self.search_query)
            };
            let search = Paragraph::new(search_text)
                .style(Style::default().fg(BRAND_COLOR));
            frame.render_widget(search, chunks[0]);
        }

        // Entry list
        let panel = Panel {
            title: &format!("Entries ({})", filtered.len()),
            focused: true,
            hints: Some("[i]nject  [c]opy  [n]ew  [e]dit  [d]el  [/]search"),
        };
        let list_area = panel.render(frame, chunks[1]);

        let items: Vec<ListItem> = filtered
            .iter()
            .map(|(real_idx, entry)| {
                let pw_display = if self.revealed == Some(*real_idx) {
                    entry.password.clone()
                } else {
                    "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}".to_string()
                };

                let meta = match &entry.username {
                    Some(u) => format!("{} | {}", u, pw_display),
                    None => pw_display,
                };

                ListItem {
                    id: entry.id.clone(),
                    label: entry.name.clone(),
                    status: None,
                    meta: Some(meta),
                    actions: vec![],
                }
            })
            .collect();

        list::render_list(frame, list_area, &items, &mut self.list_state, true);

        if filtered.is_empty() && !self.entries.is_empty() {
            let no_match = Paragraph::new("No matching entries")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(no_match, list_area);
        } else if self.entries.is_empty() {
            let empty = Paragraph::new("No entries yet. Press [n] to add one.")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(empty, list_area);
        }
    }

    fn render_form(&mut self, frame: &mut Frame, area: Rect) {
        let is_editing = matches!(self.mode, VaultMode::Editing(_));
        let title_text = if is_editing { "Edit Entry" } else { "New Entry" };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([
                Constraint::Length(2), // title
                Constraint::Length(1), // spacer
                Constraint::Length(1), // name label
                Constraint::Length(1), // name input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // username label
                Constraint::Length(1), // username input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // password label
                Constraint::Length(1), // password input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // notes label
                Constraint::Length(1), // notes input
                Constraint::Length(1), // spacer
                Constraint::Length(1), // error
                Constraint::Length(1), // hint
                Constraint::Min(0),
            ])
            .split(area);

        let title = Paragraph::new(title_text)
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        frame.render_widget(title, chunks[0]);

        // Name
        let name_style = if self.form_field == FormField::Name {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        frame.render_widget(Paragraph::new("Name:").style(name_style), chunks[2]);
        if self.form_field == FormField::Name {
            self.name_input.render(frame, chunks[3]);
        } else {
            frame.render_widget(Paragraph::new(self.name_input.value.as_str()).style(Style::default().fg(Color::White)), chunks[3]);
        }

        // Username
        let user_style = if self.form_field == FormField::Username {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        frame.render_widget(Paragraph::new("Username (optional):").style(user_style), chunks[5]);
        if self.form_field == FormField::Username {
            self.username_input.render(frame, chunks[6]);
        } else {
            frame.render_widget(Paragraph::new(self.username_input.value.as_str()).style(Style::default().fg(Color::White)), chunks[6]);
        }

        // Password
        let pw_style = if self.form_field == FormField::Password {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let pw_label = if self.form_field == FormField::Password {
            "Password (Ctrl+G to generate):"
        } else {
            "Password:"
        };
        frame.render_widget(Paragraph::new(pw_label).style(pw_style), chunks[8]);
        if self.form_field == FormField::Password {
            self.password_input.render(frame, chunks[9]);
        } else {
            // Show masked
            let masked = "\u{2022}".repeat(self.password_input.value.len().min(20));
            frame.render_widget(Paragraph::new(masked).style(Style::default().fg(Color::White)), chunks[9]);
        }

        // Notes
        let notes_style = if self.form_field == FormField::Notes {
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        frame.render_widget(Paragraph::new("Notes (optional):").style(notes_style), chunks[11]);
        if self.form_field == FormField::Notes {
            self.notes_input.render(frame, chunks[12]);
        } else {
            frame.render_widget(Paragraph::new(self.notes_input.value.as_str()).style(Style::default().fg(Color::White)), chunks[12]);
        }

        // Error
        if let Some(ref err) = self.error {
            frame.render_widget(
                Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red)),
                chunks[14],
            );
        }

        // Hint
        let hint = Paragraph::new("Tab: next field  Enter: save  Esc: cancel")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hint, chunks[15]);
    }

    fn render_masked_input(&self, frame: &mut Frame, area: Rect, input: &TextInput) {
        let masked: String = "\u{2022}".repeat(input.value.len());
        let cursor_pos = input.cursor;

        let display = if input.value.is_empty() {
            Line::from(Span::styled("_", Style::default().fg(BRAND_COLOR)))
        } else {
            let before = &masked[..cursor_pos.min(masked.len())];
            let cursor_char = if cursor_pos < masked.len() { "\u{2022}" } else { " " };
            let after_start = (cursor_pos + 1).min(masked.len());
            let after = if after_start <= masked.len() { &masked[after_start..] } else { "" };

            Line::from(vec![
                Span::raw(before.to_string()),
                Span::styled(cursor_char, Style::default().bg(BRAND_COLOR).fg(Color::Black)),
                Span::raw(after.to_string()),
            ])
        };

        frame.render_widget(Paragraph::new(display), area);
    }
}
```

**Step 2: Register the view module**

Add to `src/views/mod.rs`:

```rust
pub mod vault_view;
```

**Step 3: Verify it compiles**

Run: `cargo check`
Expected: Success (or warnings about unused imports — that's fine, wiring comes next)

**Step 4: Commit**

```bash
git add src/views/vault_view.rs src/views/mod.rs
git commit -m "feat(vault): implement VaultView with all modes and rendering"
```

---

### Task 8: Wire vault into the app

**Files:**
- Modify: `src/app.rs`

This is the main integration task. It connects the vault view to the event loop, draw function, and input handling.

**Step 1: Add imports**

At the top of `src/app.rs`, add these imports alongside the existing view imports:

```rust
use crate::views::vault_view::{VaultView, VaultAction, VaultMode};
use crate::vault::crypto;
```

**Step 2: Add vault_view to the App struct**

In the `App` struct (around line 52, in the sub-view states section), add:

```rust
    pub vault_view: VaultView,
```

**Step 3: Initialize vault_view in App::new()**

In the `App::new()` function, add alongside the other sub-view initializations:

```rust
            vault_view: VaultView::new(),
```

**Step 4: Add vault trigger detection to the event loop**

In the main event loop inside `pub fn run()`, add this block right before `// Draw` (around line 365, after the Claude splash countdown section):

```rust
        // Check for vault trigger file (from Ctrl+P tmux keybinding)
        {
            let trigger_path = config::vault_trigger_file();
            if trigger_path.exists() {
                let source_pane = std::fs::read_to_string(&trigger_path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let _ = std::fs::remove_file(&trigger_path);

                // Don't re-open if already in vault
                if !matches!(app.view, AppView::Vault { .. }) {
                    app.previous_view = Some(app.view.clone());
                    app.vault_view = VaultView::new();

                    if crypto::vault_exists(&config::vault_file()) {
                        // Existing vault: go to locked state
                        app.vault_view.enter_locked();
                    } else {
                        // No vault yet: go to create state
                        app.vault_view.enter_creating();
                    }

                    app.navigate(AppView::Vault { source_pane });
                }
            }
        }
```

**Step 5: Add vault idle timeout check**

In the `else` branch of the event poll (around line 578, where tick/refresh happens), add:

```rust
            // Vault idle timeout
            if matches!(app.view, AppView::Vault { .. }) && app.vault_view.is_idle_expired() {
                app.vault_view.enter_locked();
                app.go_back();
            }
```

**Step 6: Add vault to the input mode check**

In the `in_input_mode` boolean (around line 419), add a new line:

```rust
                    || matches!(app.view, AppView::Vault { .. });
```

This ensures ALL global keybinds are suppressed while in the vault.

**Step 7: Add vault input handling to the view match**

In the main `match &app.view` block for input handling (around line 447), add before the closing `}` of the match (or after the `AppView::Trail` arm):

```rust
                    AppView::Vault { ref source_pane } => {
                        let source_pane = source_pane.clone();
                        let action = app.vault_view.handle_input(key.code, key.modifiers);
                        handle_vault_action(&mut app, action, source_pane);
                    }
```

**Step 8: Add the vault action handler function**

Add this new function after the existing `handle_trail_input` function (or at the end of the input handler section):

```rust
fn handle_vault_action(app: &mut App, action: VaultAction, source_pane: Option<String>) {
    match action {
        VaultAction::None => {}
        VaultAction::Close => {
            app.vault_view.enter_locked();
            if let Some(ref pane) = source_pane {
                // Return to the pane the user was in before opening vault
                let _ = std::process::Command::new("tmux")
                    .args(["select-pane", "-t", pane])
                    .output();
                // Find and switch to the window containing that pane
                if let Ok(output) = std::process::Command::new("tmux")
                    .args(["display-message", "-t", pane, "-p", "#{window_index}"])
                    .output()
                {
                    let idx_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if let Ok(idx) = idx_str.parse::<u32>() {
                        tmux::switch_to_window(idx);
                    }
                }
            }
            app.go_back();
        }
        VaultAction::CreateVault(password) => {
            let path = config::vault_file();
            match crypto::create_vault(&path, &password) {
                Ok(()) => {
                    app.vault_view.enter_unlocked(vec![]);
                }
                Err(e) => {
                    app.vault_view.error = Some(format!("Failed to create vault: {}", e));
                }
            }
        }
        VaultAction::Unlock(password) => {
            let path = config::vault_file();
            match crypto::unlock_vault(&path, &password) {
                Ok(vault) => {
                    app.vault_view.enter_unlocked(vault.entries);
                }
                Err(e) => {
                    app.vault_view.error = Some(e.to_string());
                    app.vault_view.master_input = crate::components::text_input::TextInput::new("");
                }
            }
        }
        VaultAction::SaveEntry { name, username, password, notes, edit_index } => {
            let now = chrono::Utc::now().to_rfc3339();
            match edit_index {
                Some(idx) => {
                    if let Some(entry) = app.vault_view.entries.get_mut(idx) {
                        entry.name = name;
                        entry.username = username;
                        entry.password = password;
                        entry.notes = notes;
                        entry.updated_at = now;
                    }
                }
                None => {
                    app.vault_view.entries.push(crate::types::VaultEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        name,
                        username,
                        password,
                        notes,
                        created_at: now.clone(),
                        updated_at: now,
                    });
                }
            }

            // Save the vault - we need the master password
            // Re-save using the current entries. Since AES-GCM reuses the salt,
            // we need to re-encrypt. The master password is needed again.
            // DESIGN NOTE: We re-prompt for master password on save.
            // Actually, we can keep the master password in memory while unlocked.
            // Let's store it temporarily in the vault view.
            // For now, just update the in-memory entries and switch back to unlocked.
            app.vault_view.mode = VaultMode::Unlocked;

            // The actual save to disk happens via a persisted master password.
            // This is handled below — see the save_vault_to_disk call.
            save_vault_to_disk(app);
        }
        VaultAction::DeleteEntry(idx) => {
            if idx < app.vault_view.entries.len() {
                app.vault_view.entries.remove(idx);
                save_vault_to_disk(app);
            }
        }
        VaultAction::InjectPassword(password) => {
            if let Some(ref pane) = source_pane {
                // Use tmux send-keys to inject the password into the source pane
                let _ = std::process::Command::new("tmux")
                    .args(["send-keys", "-t", pane, "-l", &password])
                    .output();

                // Lock and return
                app.vault_view.enter_locked();

                // Switch back to source pane
                if let Ok(output) = std::process::Command::new("tmux")
                    .args(["display-message", "-t", pane, "-p", "#{window_index}"])
                    .output()
                {
                    let idx_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if let Ok(idx) = idx_str.parse::<u32>() {
                        tmux::switch_to_window(idx);
                    }
                }
                app.go_back();
            } else {
                app.vault_view.error = Some("No source pane — use [c] to copy instead".to_string());
            }
        }
        VaultAction::CopyPassword(password) => {
            // Copy to system clipboard (macOS)
            let result = std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(ref mut stdin) = child.stdin {
                        stdin.write_all(password.as_bytes())?;
                    }
                    child.wait()
                });

            match result {
                Ok(_) => {
                    app.vault_view.error = Some("Copied! (clipboard clears in 30s)".to_string());

                    // Spawn a thread to clear clipboard after 30 seconds
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_secs(30));
                        let _ = std::process::Command::new("pbcopy")
                            .stdin(std::process::Stdio::piped())
                            .spawn()
                            .and_then(|mut child| {
                                use std::io::Write;
                                if let Some(ref mut stdin) = child.stdin {
                                    stdin.write_all(b"")?;
                                }
                                child.wait()
                            });
                    });
                }
                Err(_) => {
                    app.vault_view.error = Some("Failed to copy to clipboard".to_string());
                }
            }
        }
    }
}
```

**Step 9: Add master password persistence and save helper**

The vault view needs to keep the master password in memory while unlocked so it can save after edits. Add a `master_password` field to `VaultView` in `src/views/vault_view.rs`:

In the `VaultView` struct, add:

```rust
    pub master_password: Option<String>,
```

Initialize it as `None` in `VaultView::new()`.

In `enter_locked()`, add: `self.master_password = None;`

Then in `handle_vault_action`, when `VaultAction::Unlock` succeeds, store it:

```rust
                    app.vault_view.master_password = Some(password);
```

And when `VaultAction::CreateVault` succeeds:

```rust
                    app.vault_view.master_password = Some(password.clone());
```

Add the save helper function in `app.rs`:

```rust
fn save_vault_to_disk(app: &mut App) {
    if let Some(ref master_pw) = app.vault_view.master_password {
        let vault = crate::types::Vault {
            entries: app.vault_view.entries.clone(),
        };
        let path = config::vault_file();
        if let Err(e) = crypto::save_vault(&path, &vault, master_pw) {
            app.vault_view.error = Some(format!("Failed to save vault: {}", e));
        }
    }
}
```

**Step 10: Add vault to render_view**

In the `render_view` function (around line 1539), add a new match arm:

```rust
        AppView::Vault { .. } => {
            app.vault_view.render(frame, area);
        }
```

**Step 11: Add vault to get_bottom_bar_items**

In `get_bottom_bar_items` (around line 1669), add a match arm:

```rust
        AppView::Vault { .. } => vec![
            ("Esc", "lock & close"),
        ],
```

**Step 12: Add vault to the help scope match**

In the `draw` function, the help scope match (around line 1516), add:

```rust
            AppView::Vault { .. } => "vault",
```

**Step 13: Add vault to the navigate match**

In `App::navigate()` (around line 182), the match block updates the status bar per view. Add:

```rust
            AppView::Vault { .. } => {
                tmux::update_status_bar(Some("Vault"));
            }
```

**Step 14: Verify it compiles**

Run: `cargo check`
Expected: Success (possibly some warnings)

**Step 15: Commit**

```bash
git add src/app.rs src/views/vault_view.rs
git commit -m "feat(vault): wire vault view into app event loop, input handling, and rendering"
```

---

### Task 9: Build and manual test

**Step 1: Build the release binary**

Run: `cargo build --release`
Expected: Clean build

**Step 2: Run the unit tests**

Run: `cargo test`
Expected: All vault crypto and generator tests pass

**Step 3: Manual testing checklist**

Test in an active Yeehaw session:

1. Press `Ctrl+P` from a bash window — should switch to window 0 and show "Create a new vault" screen
2. Enter and confirm a master password — should show empty entry list
3. Press `n` to add a new entry — should show the form
4. Fill in name, username, password (try `Ctrl+G` to generate), notes — Tab between fields, Enter to save
5. Verify entry appears in the list with masked password
6. Press `Enter` on the entry — should reveal the password
7. Press `Enter` again — should re-mask
8. Press `c` — should copy to clipboard, show "Copied!" message
9. Verify clipboard contains the password (Cmd+V somewhere)
10. Wait 30s, verify clipboard is cleared
11. Press `Esc` — should lock vault and return to previous pane
12. Press `Ctrl+P` again — should show "Enter master password" (not create)
13. Enter wrong password — should show "Wrong master password"
14. Enter correct password — should show your entry
15. Press `i` on an entry — should inject password into the pane you were in before
16. Press `e` to edit, `d` to delete — verify both work
17. Test `/` search — type to filter entries
18. Verify idle timeout works (wait 2 minutes with no input)

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat(vault): password manager complete — crypto, generator, view, tmux integration"
```
