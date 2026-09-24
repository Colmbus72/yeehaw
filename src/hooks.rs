use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::config;

// Embedded skill file
const SKILL_BYTES: &[u8] = include_bytes!("../skills/yeehaw-project-setup.skill");

const HOOK_SCRIPT_NAME: &str = "claude-hook";

const HOOK_SCRIPT_CONTENT: &str = r#"#!/bin/bash
# Yeehaw Claude Hook - writes session status for the CLI to read
STATUS="$1"
# Which ranch to report into, most specific first. $2 is what sessions Yeehaw
# launches pass: the hook runs in the tmux *server's* environment, which is not
# the environment the yeehaw process was started with, so YEEHAW_HOME does not
# reliably survive the trip and the launcher writes the answer into the command.
# The env var and the $HOME default are for sessions started by hand.
RANCH_DIR="${2:-${YEEHAW_HOME:-$HOME/.yeehaw}}"
PANE_ID="${TMUX_PANE:-unknown}"
SIGNAL_DIR="$RANCH_DIR/session-signals"
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
    // Write temp → chmod the temp → rename. The mode travels with the inode, so
    // the name never resolves to a script that exists and is not yet executable.
    //
    // This used to be `fs::write` + `set_permissions`, justified by being
    // reachable only from the one-shot `yeehaw hooks install`, where nothing
    // races the write. `ensure_config_dirs` now installs it too, so it runs on
    // every process start — a TUI, an mcp-server and a `ranch serve` can be in
    // here at once, and a claude hook firing in that window would exec a
    // half-written or non-executable file.
    crate::store::write_atomic_with_mode(&path, HOOK_SCRIPT_CONTENT.as_bytes(), 0o755)?;

    Ok(path)
}

/// Check if hook script exists
pub fn hook_script_exists() -> bool {
    hook_script_path().exists()
}

/// One entry in an event's hook list.
///
/// Claude Code's settings schema wants an **object** here — `{"type":
/// "command", "command": "..."}` — not the bare command string this used to
/// emit. A bare string is not an error you get told about: `claude`'s own
/// `--help` says settings that fail validation "are silently ignored" in
/// non-interactive mode, so the malformed config cost nothing at startup and
/// simply never fired. That is why the dashboard's working/waiting display
/// looked like a feature nobody had finished rather than a broken one.
fn hook_command(status: &str) -> serde_json::Value {
    // Both values are interpolated into a string a shell runs, so both are
    // quoted: a home directory with a space in it would otherwise arrive as two
    // arguments and the ranch would be read as a status.
    let command = format!(
        "{} {} {}",
        crate::tmux::single_quote(&hook_script_path().to_string_lossy()),
        status,
        crate::tmux::single_quote(&config::yeehaw_dir().to_string_lossy()),
    );
    serde_json::json!({ "type": "command", "command": command })
}

/// Get the Claude settings hooks configuration as JSON.
///
/// Passed to `claude --settings` for the sessions Yeehaw launches, and printed
/// by `yeehaw hooks install` for sessions the user starts themselves.
pub fn get_claude_hooks_config() -> serde_json::Value {
    serde_json::json!({
        "hooks": {
            // `*` is the match-everything matcher for a tool event: any tool
            // call means the session is doing something.
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [hook_command("working")],
            }],
            // Deliberately no `matcher`. Stop is one of the events that takes
            // none — it fires once, when the turn ends, and there is nothing to
            // discriminate on.
            "Stop": [{
                "hooks": [hook_command("waiting")],
            }],
            // Notification *does* take a matcher, keyed on the notification
            // kind; `idle_prompt` is the one that means "waiting on the human".
            "Notification": [{
                "matcher": "idle_prompt",
                "hooks": [hook_command("waiting")],
            }],
        }
    })
}

/// The hooks config as the single argument `claude --settings` takes.
///
/// `--settings` accepts "a settings JSON file or a JSON string", so this goes
/// on the command line inline, the way `--mcp-config` already does. A file
/// would have to be written somewhere, kept alive for the life of the session,
/// and cleaned up after a session that may outlive the process that launched
/// it; the string has none of that and is a few hundred bytes.
pub fn claude_settings_json() -> String {
    get_claude_hooks_config().to_string()
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
    /// A session launched from Yeehaw points its hook config at
    /// `bin/claude-hook`. If the script is not there the hook execs nothing, no
    /// signal is ever written, and every claude window falls back to a relative
    /// timestamp — the working/waiting/idle display looks removed rather than
    /// unconfigured. `ensure_config_dirs` is the only thing every entry point
    /// shares, so it is where the script has to land.
    #[test]
    fn opening_the_config_installs_the_claude_hook_script() {
        crate::testing::with_temp_ranch(|_| {
            assert!(!super::hook_script_exists(), "fixture should start clean");

            crate::config::ensure_config_dirs();

            let path = super::hook_script_path();
            assert!(path.exists(), "a session's hook would exec a missing file");

            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "the hook must be executable to be a hook");
        });
    }

    /// The script is rewritten by whichever process starts first, and a TUI, an
    /// mcp-server and a `ranch serve` can all be in here at once. Temp-then-
    /// rename is what keeps a concurrent hook from exec'ing a partial file.
    #[test]
    fn reinstalling_leaves_no_window_where_the_hook_is_unusable() {
        crate::testing::with_temp_ranch(|_| {
            crate::config::ensure_config_dirs();
            let path = super::hook_script_path();
            let first = std::fs::read_to_string(&path).unwrap();

            super::install_hook_script().unwrap();

            assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
            let leftovers: Vec<String> = std::fs::read_dir(crate::config::bin_dir())
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n != super::HOOK_SCRIPT_NAME)
                .collect();
            assert!(leftovers.is_empty(), "temp files left behind: {:?}", leftovers);
        });
    }

    /// Claude Code's settings schema puts **objects** in the inner `hooks`
    /// array — `{"type": "command", "command": "..."}` — not bare command
    /// strings. A bare string is not a schema violation Claude complains
    /// about: `claude -p` "silently ignores" settings that fail validation
    /// (its own `--help` says so), so the hook simply never ran and every
    /// session fell back to a relative timestamp.
    #[test]
    fn every_hook_entry_is_the_command_object_claude_code_parses() {
        crate::testing::with_temp_ranch(|_| {
            let config = super::get_claude_hooks_config();
            let events = config["hooks"].as_object().expect("hooks is an object");
            assert!(!events.is_empty(), "no events configured");

            for (event, entries) in events {
                for entry in entries.as_array().expect("each event holds an array") {
                    let hooks = entry["hooks"]
                        .as_array()
                        .unwrap_or_else(|| panic!("{event}: entry has no hooks array"));
                    assert!(!hooks.is_empty(), "{event}: no hooks");
                    for hook in hooks {
                        assert_eq!(
                            hook["type"].as_str(),
                            Some("command"),
                            "{event}: a hook needs type=command, got {hook}",
                        );
                        assert!(
                            hook["command"].as_str().is_some_and(|c| !c.is_empty()),
                            "{event}: a hook needs a command string, got {hook}",
                        );
                    }
                }
            }
        });
    }

    /// `Stop` is one of the events that takes no matcher at all. The three
    /// events used here are the ones the dashboard's working/waiting display
    /// is built on, so if one were renamed out from under us the status would
    /// go quiet without anything failing.
    #[test]
    fn the_events_are_the_ones_claude_code_emits() {
        crate::testing::with_temp_ranch(|_| {
            let config = super::get_claude_hooks_config();
            let events = config["hooks"].as_object().unwrap();

            let mut names: Vec<&str> = events.keys().map(|k| k.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, ["Notification", "PreToolUse", "Stop"]);

            assert_eq!(events["PreToolUse"][0]["matcher"].as_str(), Some("*"));
            assert_eq!(events["Notification"][0]["matcher"].as_str(), Some("idle_prompt"));
            assert!(
                events["Stop"][0].get("matcher").is_none(),
                "Stop takes no matcher",
            );
        });
    }

    /// The hook runs in whatever environment the tmux *server* was started
    /// with, which is not the environment that launched the yeehaw process —
    /// so `YEEHAW_HOME` does not reliably survive the trip. The ranch the
    /// dashboard actually reads is therefore written into the command itself,
    /// and both it and the script path are quoted: a home directory with a
    /// space in it would otherwise split into two arguments.
    #[test]
    fn the_command_names_the_ranch_the_dashboard_reads() {
        crate::testing::with_temp_ranch(|ranch| {
            let config = super::get_claude_hooks_config();
            let command = config["hooks"]["Stop"][0]["hooks"][0]["command"]
                .as_str()
                .expect("Stop hook has a command")
                .to_string();

            let expected = format!(
                "{} waiting {}",
                crate::tmux::single_quote(&super::hook_script_path().to_string_lossy()),
                crate::tmux::single_quote(&ranch.dir.path().to_string_lossy()),
            );
            assert_eq!(command, expected);
        });
    }

    /// End of the chain: the script the command names has to actually land a
    /// file where `signals::read_signal` looks for it, with a body
    /// `signals::parse_signal` accepts. A file that exists but does not parse
    /// is the same as no file at all.
    #[test]
    fn running_the_installed_script_writes_a_signal_the_dashboard_can_read() {
        crate::testing::with_temp_ranch(|ranch| {
            crate::config::ensure_config_dirs();
            let ranch_dir = ranch.dir.path().to_string_lossy().to_string();

            let out = std::process::Command::new(super::hook_script_path())
                .args(["working", &ranch_dir])
                // The script falls back to `$HOME/.yeehaw` when it is given no
                // ranch. Pointing HOME at the fixture too means a regression
                // that ignores the argument writes here, not into the
                // developer's real ranch.
                .env("HOME", ranch.dir.path())
                .env_remove("YEEHAW_HOME")
                .env("TMUX_PANE", "%7")
                .output()
                .expect("the installed hook should be runnable");
            assert!(out.status.success(), "hook exited {:?}", out.status);

            let signal = crate::signals::read_signal("%7")
                .expect("the dashboard should be able to read the signal just written");
            assert_eq!(signal.status, crate::signals::SessionStatus::Working);
        });
    }

    /// `hooks install` prints the config for the user to paste, then checks
    /// whether it is already there. Reading it back with the wrong shape means
    /// the check reports "not configured" forever and the user pastes a
    /// duplicate every time.
    #[test]
    fn the_installed_check_recognizes_the_config_this_module_prints() {
        crate::testing::with_temp_ranch(|_| {
            let config = super::get_claude_hooks_config();
            assert!(
                super::settings_contain_yeehaw_hooks(&config),
                "the config we tell users to paste is not recognized once pasted",
            );
            assert!(
                !super::settings_contain_yeehaw_hooks(&serde_json::json!({})),
                "empty settings must not count as configured",
            );
        });
    }

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

    settings_contain_yeehaw_hooks(&settings)
}

/// Does this settings document already point `PreToolUse` at our hook?
///
/// Reads the `{"type": "command", "command": ...}` form, and still recognizes
/// the bare string this module used to emit. Not for the user's sake — that
/// config never worked — but so someone who pasted it years ago is told their
/// hooks are present and then finds them replaced, rather than being handed a
/// second copy to paste alongside the dead one.
pub(crate) fn settings_contain_yeehaw_hooks(settings: &serde_json::Value) -> bool {
    settings["hooks"]["PreToolUse"]
        .as_array()
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry["hooks"].as_array().is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        let command = hook["command"].as_str().or_else(|| hook.as_str());
                        // The script's *name*, not "yeehaw": the ranch it lives
                        // under moves with `YEEHAW_HOME`, and matching the
                        // directory would report "not configured" for anyone
                        // who relocated it.
                        command.is_some_and(|s| s.contains(HOOK_SCRIPT_NAME))
                    })
                })
            })
        })
}
