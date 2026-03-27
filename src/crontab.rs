use std::io::Write;
use std::process::Command;

use anyhow::{Context, Result};

use crate::config;
use crate::types::Worm;

const BEGIN_MARKER: &str = "# BEGIN YEEHAW MANAGED - DO NOT EDIT";
const END_MARKER: &str = "# END YEEHAW MANAGED";

fn read_current_crontab() -> String {
    Command::new("crontab")
        .arg("-l")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn write_crontab(content: &str) -> Result<()> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn crontab")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .context("Failed to write to crontab stdin")?;
    }

    let status = child.wait().context("Failed to wait for crontab")?;
    if !status.success() {
        anyhow::bail!("crontab command failed");
    }
    Ok(())
}

fn yeehaw_binary() -> String {
    // Prefer `which yeehaw` to find the installed binary (mirrors TS version).
    // This avoids baking in a cargo target/ path which can trigger syspolicyd
    // loops on macOS when the dev binary is unsigned.
    if let Ok(output) = std::process::Command::new("which")
        .arg("yeehaw")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return path;
            }
        }
    }
    // Fallback to current executable path
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "yeehaw".to_string())
}

fn build_crontab_entry(worm: &Worm) -> String {
    let bin = yeehaw_binary();
    format!("{} {} worm exec {}", worm.schedule, bin, worm.name)
}

pub fn sync_crontab() -> Result<()> {
    let current = read_current_crontab();

    // Strip existing yeehaw section
    let mut lines: Vec<&str> = Vec::new();
    let mut in_section = false;
    for line in current.lines() {
        if line.trim() == BEGIN_MARKER {
            in_section = true;
            continue;
        }
        if line.trim() == END_MARKER {
            in_section = false;
            continue;
        }
        if !in_section {
            lines.push(line);
        }
    }

    // Remove trailing empty lines
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }

    // Build new section from enabled worms
    let worms = config::load_worms();
    let enabled: Vec<&Worm> = worms.iter().filter(|w| w.enabled).collect();

    let mut new_content = lines.join("\n");
    if !new_content.is_empty() {
        new_content.push('\n');
    }

    if !enabled.is_empty() {
        new_content.push('\n');
        new_content.push_str(BEGIN_MARKER);
        new_content.push('\n');
        for worm in &enabled {
            new_content.push_str(&build_crontab_entry(worm));
            new_content.push('\n');
        }
        new_content.push_str(END_MARKER);
        new_content.push('\n');
    }

    write_crontab(&new_content)
}
