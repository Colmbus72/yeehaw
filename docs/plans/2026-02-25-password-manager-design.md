# Password Manager (Vault) Design

**Goal:** Tmux-level password manager accessible from anywhere in a Yeehaw session via `Ctrl+P`, with encrypted local storage and direct context injection.

---

## Access & Trigger Mechanism

**Hotkey:** `Ctrl+P` registered as a tmux keybinding (`bind-key -n C-p`) alongside existing `C-y`, `C-h`, `C-l`.

**Trigger flow:**

1. User hits `Ctrl+P` from any pane (bash, Claude, SSH, TUI)
2. Tmux executes: `run-shell 'echo "#{pane_id}" > ~/.yeehaw/vault-trigger' \; select-window -t :0`
3. TUI event loop (250ms tick) detects `~/.yeehaw/vault-trigger`
4. Reads source pane ID, deletes trigger file
5. Saves current view to `previous_view`, navigates to `AppView::Vault`

**Return flow:**

- Lock vault, clear decrypted entries from memory
- If `source_pane` is set: `tmux select-window` + `select-pane` back to source
- If no `source_pane` (opened from within TUI): navigate to `previous_view`

The vault can be opened from within the TUI directly (no source pane) or via the tmux hotkey (with source pane for injection).

---

## Encryption & Storage

**File:** `~/.yeehaw/vault.enc` (binary, not YAML)

### Key Derivation (Argon2id)

- 32-byte random salt, generated once at vault creation
- Argon2id parameters: 19456 KiB memory, 2 iterations, 1 parallelism (OWASP minimums)
- Derives a 256-bit key from master password + salt

### Encryption (AES-256-GCM)

- 12-byte random nonce, regenerated on every save
- Produces ciphertext + 16-byte authentication tag

### File Format

```
[32 bytes: salt]
[12 bytes: nonce]
[remaining: AES-256-GCM ciphertext + auth tag]
```

### Decrypted Payload

YAML, deserialized to:

```rust
struct Vault {
    entries: Vec<VaultEntry>,
}

struct VaultEntry {
    id: String,              // UUID v4
    name: String,            // "Linear API", "SSH prod-server"
    username: Option<String>,
    password: String,
    notes: Option<String>,
    created_at: String,      // ISO 8601
    updated_at: String,
}
```

### Security Properties

- No password hash stored. Correct password = successful AES-GCM decryption. Wrong password = auth tag verification fails.
- Fresh nonce on every save. Salt persists for vault lifetime.
- Changing master password regenerates both salt and nonce (full re-encrypt).

---

## View Architecture

### AppView

```rust
AppView::Vault { source_pane: Option<String> }
```

### VaultView State Machine

```
Creating --> Locked --> Unlocked --> Adding
                          |            |
                       Revealed     Editing
```

- **Creating** - First-time setup: enter + confirm master password, creates vault file
- **Locked** - Masked master password input, Enter to decrypt
- **Unlocked** - Entry list with passwords masked (`••••••••`), hotkeys active
- **Revealed(index)** - One entry's password shown in plain text, any key re-masks
- **Adding** - Form: name, username (optional), password, notes (optional)
- **Editing(index)** - Same form, pre-filled with existing entry data

### Hotkeys (Unlocked Mode)

| Key | Action |
|-----|--------|
| `i` | Inject password into source pane via `tmux send-keys` |
| `c` | Copy password to system clipboard (auto-clear 30s) |
| `Enter` | Reveal/hide password in place |
| `e` | Edit selected entry |
| `d` | Delete selected entry (with confirm dialog) |
| `n` | New entry |
| `j/k` | Navigate entries up/down |
| `/` | Search/filter entries |
| `Esc` | Lock vault and return to previous context |

### Input Capture

The vault view always captures all input. Global keybinds (`?`, `Ctrl+R`, `v`) are suppressed while the vault is open. Only `Esc` exits.

---

## Password Generator

Available during add/edit mode. When the password field is focused, `Ctrl+G` fills it with a generated password.

- 20 characters
- Mixed case letters + digits + symbols (`!@#$%^&*-_+=`)
- `Ctrl+G` prefix avoids conflict with typing `g` as a literal password character

---

## Context Injection

After selecting an entry and pressing `i`:

1. Read `source_pane` from `AppView::Vault`
2. Execute `tmux send-keys -t <pane_id> -l '<password>'` (literal mode)
3. Lock vault, clear memory
4. Switch back to source pane window

If no `source_pane` (vault opened from within TUI), `i` is disabled and the user sees a hint to use `c` (clipboard) instead.

---

## Idle Timeout & Security

- **2-minute idle timer** - resets on any keypress. On expiry: clear entries, lock vault, return to source pane.
- **Memory clearing** - entries Vec explicitly cleared and dropped on lock. Master password string never stored beyond decrypt/encrypt call.
- **Clipboard auto-clear** - after `c` copy, background thread sleeps 30s then clears clipboard.
- **File permissions** - vault.enc created with mode `0600`. Warn on load if permissions are looser.

---

## New Dependencies

```toml
argon2 = "0.5"
aes-gcm = "0.10"
uuid = { version = "1", features = ["v4"] }
```

`rand` for salt/nonce generation (may already be a transitive dependency).

---

## Future Enhancements (Not in Scope)

- TOTP/2FA code generation
- Configurable generator (length, charset)
- Categories/folders
- Import from other password managers
- Backup/export (encrypted)
