use std::fs;
use std::process::Command;

/// Open content in user's editor and return the edited content.
/// Similar to how git opens commit messages.
pub fn edit_in_editor(initial_content: &str, filename: &str) -> Option<String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nano".to_string());

    let temp_dir = std::env::temp_dir();
    let random: u32 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let temp_file = temp_dir.join(format!("yeehaw-{:08x}-{}", random, filename));

    // Write initial content
    if fs::write(&temp_file, initial_content).is_err() {
        return None;
    }

    // Must leave raw mode before spawning editor
    let _ = crossterm::terminal::disable_raw_mode();

    // Open editor (blocks until closed)
    let result = Command::new(&editor)
        .arg(&temp_file)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    // Re-enter raw mode
    let _ = crossterm::terminal::enable_raw_mode();

    match result {
        Ok(status) if status.success() => {
            let content = fs::read_to_string(&temp_file).ok();
            let _ = fs::remove_file(&temp_file);
            content
        }
        _ => {
            let _ = fs::remove_file(&temp_file);
            None
        }
    }
}
