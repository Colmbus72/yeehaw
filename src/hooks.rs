use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::config;

// Embedded skill file
const SKILL_BYTES: &[u8] = include_bytes!("../skills/yeehaw-project-setup.skill");

const HOOK_SCRIPT_NAME: &str = "claude-hook";

const HOOK_SCRIPT_CONTENT: &str = r#"#!/bin/bash
# Yeehaw Claude Hook - writes session status for the CLI to read
STATUS="$1"
PANE_ID="${TMUX_PANE:-unknown}"
SIGNAL_DIR="$HOME/.yeehaw/session-signals"
SIGNAL_FILE="$SIGNAL_DIR/${PANE_ID//[^a-zA-Z0-9]/_}.json"

mkdir -p "$SIGNAL_DIR"
cat > "$SIGNAL_FILE" << EOF
{"status":"$STATUS","updated":$(date +%s)}
EOF
"#;

pub fn hooks_dir() -> PathBuf {
    config::yeehaw_dir().join("bin")
}

pub fn hook_script_path() -> PathBuf {
    hooks_dir().join(HOOK_SCRIPT_NAME)
}

/// Install the Claude hook script to ~/.yeehaw/bin/
pub fn install_hook_script() -> anyhow::Result<PathBuf> {
    let dir = hooks_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }

    let signals_dir = config::signals_dir();
    if !signals_dir.exists() {
        fs::create_dir_all(&signals_dir)?;
    }

    let path = hook_script_path();
    // Note the ordering rule this does *not* follow. The correct sequence for a
    // file that must be executable the instant it is reachable is: write a temp
    // file, chmod the temp file, then rename it into place — the mode travels
    // with the inode, so the name never resolves to a non-executable script.
    // Chmod-after-publish (either shape: `write` then chmod, as here, or rename
    // then chmod) leaves a window where the script exists and is not yet 0755.
    //
    // Left as-is deliberately: this is reachable only from the one-shot
    // `yeehaw hooks install` command, where nothing is racing the write.
    // `store::write_atomic_with_mode` is the helper to reach for if that ever
    // stops being true.
    fs::write(&path, HOOK_SCRIPT_CONTENT)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;

    Ok(path)
}

/// Check if hook script exists
pub fn hook_script_exists() -> bool {
    hook_script_path().exists()
}

/// Get the Claude settings.json hooks configuration as JSON
pub fn get_claude_hooks_config() -> serde_json::Value {
    let path = hook_script_path().to_string_lossy().to_string();

    serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [format!("{} working", path)],
            }],
            "Stop": [{
                "matcher": "*",
                "hooks": [format!("{} waiting", path)],
            }],
            "Notification": [{
                "matcher": "idle_prompt",
                "hooks": [format!("{} waiting", path)],
            }],
        }
    })
}

/// Install the bundled yeehaw-project-setup skill to ~/.yeehaw/skills/
pub fn install_skill() -> anyhow::Result<PathBuf> {
    let skills_dir = config::yeehaw_dir().join("skills");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir)?;
    }

    let path = skills_dir.join("yeehaw-project-setup.skill");
    // Temp-and-rename, not a bare write. `ensure_config_dirs()` calls this only
    // when the file is absent — it is guarded by `skill_installed()`, an
    // existence check — so in practice it runs on a fresh ranch or an explicit
    // `yeehaw skills install`, not on every load. The rename still earns its
    // keep: two processes hitting that first run at once would otherwise have
    // one truncate-in-place while the other's `read_skill_markdown` unzips the
    // same path, handing it a corrupt archive.
    crate::store::write_atomic_bytes(&path, SKILL_BYTES)?;
    Ok(path)
}

/// Check if the skill file exists
pub fn skill_installed() -> bool {
    config::yeehaw_dir().join("skills").join("yeehaw-project-setup.skill").exists()
}

/// Extract and cache the SKILL.md body from the embedded yeehaw-project-setup.skill archive.
///
/// The .skill file is a ZIP archive; we pull `yeehaw-project-setup/SKILL.md` out of it on
/// first call and stash the result in a process-wide cache. Used by the MCP prompt handler
/// to serve the skill content on demand.
pub fn read_skill_markdown() -> anyhow::Result<&'static str> {
    static CACHE: OnceLock<String> = OnceLock::new();
    if let Some(s) = CACHE.get() {
        return Ok(s.as_str());
    }
    let cursor = std::io::Cursor::new(SKILL_BYTES);
    let mut archive = zip::ZipArchive::new(cursor)?;
    let mut file = archive.by_name("yeehaw-project-setup/SKILL.md")?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    // If another thread won the race, prefer their copy — both are identical anyway.
    let _ = CACHE.set(content);
    Ok(CACHE.get().expect("cache populated above").as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_markdown_extracts_from_embedded_archive() {
        let md = read_skill_markdown().expect("should extract SKILL.md from embedded skill");
        assert!(
            md.contains("yeehaw-project-setup"),
            "skill markdown should reference its own name in frontmatter",
        );
        assert!(
            md.contains("# Yeehaw Project Setup"),
            "skill markdown should contain the H1 title",
        );
    }

    #[test]
    fn skill_markdown_is_cached() {
        let a = read_skill_markdown().unwrap();
        let b = read_skill_markdown().unwrap();
        // Same static slice on second call — pointer equality proves the cache hit.
        assert!(std::ptr::eq(a.as_ptr(), b.as_ptr()));
    }
}

/// Check if Claude hooks are already configured in ~/.claude/settings.json
pub fn check_claude_hooks_installed() -> bool {
    let claude_settings = dirs::home_dir()
        .map(|h| h.join(".claude").join("settings.json"))
        .unwrap_or_default();

    if !claude_settings.exists() {
        return false;
    }

    let content = match fs::read_to_string(&claude_settings) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    settings["hooks"]["PreToolUse"]
        .as_array()
        .map(|arr| {
            arr.iter().any(|h| {
                h["hooks"].as_array().map(|hooks| {
                    hooks.iter().any(|cmd| {
                        cmd.as_str().map(|s| s.contains("yeehaw")).unwrap_or(false)
                    })
                }).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
