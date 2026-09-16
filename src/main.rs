mod app;
mod config;
mod connect;
mod context;
mod critters;
mod crontab;
mod editor;
mod git;
mod hooks;
mod issues;
mod mcp_server;
mod migrate;
mod ranch;
mod ranchhand_k8s;
mod ranchhand_terraform;
mod remote_grid;
mod signals;
mod ssh;
mod store;
#[cfg(test)]
mod testing;
mod trails;
mod tmux;
mod tombstones;
mod types;
mod update_check;
mod vault;
mod watcher;
mod components;
mod views;

use anyhow::Result;

fn main() -> Result<()> {
    // Ensure config directories exist.
    //
    // Must stay before `ratatui::init()` and the panic hook installed in
    // `run_tui()`: this is the first call to resolve `YEEHAW_HOME`, and a
    // non-absolute value panics here. Running it on a clean terminal means that
    // message prints legibly instead of being scrambled across a raw-mode
    // alternate screen.
    config::ensure_config_dirs();

    let args: Vec<String> = std::env::args().collect();

    // Subcommand routing
    if handle_subcommands(&args) {
        return Ok(());
    }

    // Default: run the TUI
    if !tmux::has_tmux() {
        eprintln!("Error: tmux is required but not installed");
        eprintln!("Install tmux and try again");
        std::process::exit(1);
    }

    // Check for updates (non-blocking, uses cache)
    if let Some(info) = update_check::check_for_updates() {
        if info.update_available {
            eprintln!("\x1b[33m{}\x1b[0m\n", update_check::format_update_message(&info));
        }
    }

    if tmux::is_inside_yeehaw_session() {
        // Re-source ~/.yeehaw/tmux.conf on every TUI start. tmux keybindings and
        // key tables live in the running server's memory, and the config is
        // otherwise only sourced when the yeehaw session is first *created* — so
        // upgrading yeehaw in place leaves a server bound to whatever the previous
        // version installed, missing anything new (the `yeehaw-remote` table among
        // them). Idempotent: everything in the config is a `set -g` or a
        // `bind-key`, both last-write-wins, and session-local options are
        // untouched. Best-effort here; `connect_to_barn` re-sources and checks.
        tmux::refresh_tmux_config();
        return run_tui();
    }

    if !tmux::yeehaw_session_exists() {
        tmux::create_yeehaw_session()?;
    }

    tmux::attach_to_yeehaw();
    Ok(())
}

fn handle_subcommands(args: &[String]) -> bool {
    match args.get(1).map(|s| s.as_str()) {
        Some("mcp-server") => {
            if let Err(e) = run_mcp_server() {
                eprintln!("MCP server error: {}", e);
                std::process::exit(1);
            }
            true
        }
        Some("hooks") => {
            handle_hooks_subcommand(args);
            true
        }
        Some("worm") => {
            handle_worm_subcommand(args);
            true
        }
        Some("trail") => {
            handle_trail_subcommand(args);
            true
        }
        Some("skills") => {
            handle_skills_subcommand(args);
            true
        }
        Some("ranch") => {
            handle_ranch_subcommand(args);
            true
        }
        Some("connect") => {
            match args.get(2) {
                Some(name) => {
                    if let Err(e) = connect::run(name) {
                        eprintln!("\x1b[31mError:\x1b[0m {}", e);
                        std::process::exit(1);
                    }
                }
                None => {
                    eprintln!("Usage: yeehaw connect <barn>");
                    std::process::exit(1);
                }
            }
            true
        }
        _ => false,
    }
}

fn handle_hooks_subcommand(args: &[String]) {
    if args.get(2).map(|s| s.as_str()) == Some("install") {
        match hooks::install_hook_script() {
            Ok(path) => {
                println!("\x1b[32m✓\x1b[0m Hook script installed: {}", path.display());
                println!();
                println!("\x1b[33mNote:\x1b[0m Claude sessions started from Yeehaw already have hooks enabled.");
                println!("This command is only needed for Claude sessions started outside Yeehaw.");

                if hooks::check_claude_hooks_installed() {
                    println!("\n\x1b[32m✓\x1b[0m Claude hooks already configured in ~/.claude/settings.json");
                } else {
                    println!("\nTo enable status tracking for external Claude sessions,");
                    println!("add this to ~/.claude/settings.json:");
                    let config = hooks::get_claude_hooks_config();
                    println!("{}", serde_json::to_string_pretty(&config).unwrap_or_default());
                }
            }
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m Failed to install hooks: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        println!("Usage: yeehaw hooks install");
        println!();
        println!("Install Claude hooks for session status tracking.");
        println!("Note: Sessions started from Yeehaw already have hooks enabled automatically.");
        println!("This is only needed for Claude sessions started outside Yeehaw.");
    }
}

fn handle_skills_subcommand(args: &[String]) {
    match args.get(2).map(|s| s.as_str()) {
        Some("install") => {
            match hooks::install_skill() {
                Ok(path) => {
                    println!("\x1b[32m✓\x1b[0m Skill installed: {}", path.display());
                    println!();
                    println!("To use this skill, add it to Claude Code:");
                    println!("  claude /install-skill {}", path.display());
                }
                Err(e) => {
                    eprintln!("\x1b[31mError:\x1b[0m Failed to install skill: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some("path") => {
            let path = config::skills_dir().join("yeehaw-project-setup.skill");
            println!("{}", path.display());
        }
        _ => {
            println!("Usage: yeehaw skills <command>");
            println!();
            println!("Commands:");
            println!("  install   Install bundled skills to ~/.yeehaw/skills/");
            println!("  path      Print the path to the installed skill file");
        }
    }
}

fn handle_ranch_subcommand(args: &[String]) {
    match ranch::parse_args(args) {
        ranch::RanchCommand::Serve => {
            // Not a `RanchCommand` field: `serve` is a verb, `--selftest` is a
            // flag on it, and keeping the enum a plain description of the verb
            // keeps `parse_args` free of flag handling.
            let selftest = args.iter().any(|a| a == "--selftest");
            if let Err(e) = ranch::serve(selftest) {
                // stderr, never stdout: stdout is the protocol stream.
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        ranch::RanchCommand::Init { name } => {
            // `{:#}` throughout this arm, not `{}`: `ranch init` surfaces
            // `migrate::adopt_this_machine`'s refusals, and those are the whole
            // of the user's guidance — an unparseable project's path, or the
            // barn that turns out to be some other machine. Plain `{}` renders
            // only the outermost context and throws every one of them away.
            match ranch::init(name) {
                Ok(report) => {
                    println!("This machine is now the Ranch House, as barn '{}'.", report.barn);
                    if report.adoption.barn_created {
                        println!("  created barns/{}.yaml", report.barn);
                    }
                    if report.adoption.livestock_reassigned > 0 {
                        println!(
                            "  pinned {} livestock to '{}' across {} project(s)",
                            report.adoption.livestock_reassigned,
                            report.barn,
                            report.adoption.projects_touched.len()
                        );
                    }
                    println!("  stamped {} entities and recorded a sync base for each", report.stamped);
                    println!("  brand: {}", report.brand);
                    println!("\nOn another machine, run: yeehaw ranch join <this machine>");
                }
                Err(e) => {
                    eprintln!("\x1b[31mError:\x1b[0m {:#}", e);
                    std::process::exit(1);
                }
            }
        }
        ranch::RanchCommand::Join { target, name } => match ranch::join(&target, name) {
            Ok(outcome) => {
                // Before the apply lines, because it happened before them and
                // because it is the one thing a join changes about *this*
                // machine: the name came from the house's roster, not from this
                // hostname, and every livestock here now names it.
                // Before everything, and on its own line: the house handed this
                // machine a record it already had. Almost always right — a barn
                // made by hand in the TUI, finally running yeehaw — but the other
                // reading is a mistyped `--as` merging this machine into a record
                // that describes some other host, and only the user can tell
                // which. Silence here is what would make that expensive.
                if let Some(claimed) = &outcome.claimed_existing_barn {
                    println!(
                        "\nAdopted the existing barn '{}' — it had no brand, so this machine \
                         has taken over that record rather than creating a second one.",
                        claimed
                    );
                }
                if let Some(adoption) = &outcome.adopted {
                    if adoption.barn_created || adoption.livestock_reassigned > 0 {
                        println!("\nThis machine is barn '{}' on that ranch.", outcome.barn);
                        if adoption.barn_created {
                            println!("  created barns/{}.yaml", outcome.barn);
                        }
                        if adoption.livestock_reassigned > 0 {
                            println!(
                                "  pinned {} livestock to '{}' across {} project(s)",
                                adoption.livestock_reassigned,
                                outcome.barn,
                                adoption.projects_touched.len()
                            );
                        }
                    }
                }
                match outcome.applied {
                    Some(applied) => {
                        println!(
                            "\nJoined the ranch at '{}': {} written, {} deleted.",
                            outcome.peer, applied.written, applied.deleted
                        );
                        if let Some(backup) = &outcome.backup {
                            println!("  the ranch as it was: {}", backup.display());
                        }
                        if outcome.brand_pushed {
                            println!("  this machine's brand is installed on '{}'", target);
                        }
                        for warning in &outcome.warnings {
                            eprintln!("\x1b[33mWarning:\x1b[0m {}", warning);
                        }
                    }
                    None => println!("\nNothing applied."),
                }
            }
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m {:#}", e);
                std::process::exit(1);
            }
        },
        ranch::RanchCommand::Status => {
            eprintln!("\x1b[31mError:\x1b[0m `yeehaw ranch status` is not implemented yet");
            std::process::exit(1);
        }
        ranch::RanchCommand::Usage => {
            println!("Usage:");
            println!("  yeehaw ranch init [name]    Make this machine the Ranch House");
            println!("  yeehaw ranch join <target> [--as <name>]");
            println!("                              Join the ranch at <target> (user@host).");
            println!("                              --as proposes what this machine is called;");
            println!("                              the Ranch House has the final say.");
            println!("  yeehaw ranch serve          Speak the sync protocol on stdio (run over ssh)");
            println!("  yeehaw ranch status         Show this machine's ranch membership");
        }
    }
}

fn handle_worm_subcommand(args: &[String]) {
    match args.get(2).map(|s| s.as_str()) {
        Some("exec") => {
            if let Some(worm_name) = args.get(3) {
                if let Err(e) = run_worm_exec(worm_name) {
                    eprintln!("\x1b[31mError:\x1b[0m {}", e);
                    std::process::exit(1);
                }
            } else {
                eprintln!("Usage: yeehaw worm exec <name>");
                std::process::exit(1);
            }
        }
        Some("sync") => {
            match crontab::sync_crontab() {
                Ok(()) => println!("\x1b[32m✓\x1b[0m Crontab synced"),
                Err(e) => {
                    eprintln!("\x1b[31mError:\x1b[0m Failed to sync crontab: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some("list") => {
            let worms = config::load_worms();
            if worms.is_empty() {
                println!("No worms configured");
            } else {
                for worm in &worms {
                    let status = if worm.enabled { "\x1b[32m✓\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
                    let cmd_preview: String = worm.command.chars().take(40).collect();
                    println!("{} {:<20} {:<15} {:<8} {}",
                        status, worm.name, worm.schedule, worm.worm_type, cmd_preview);
                }
            }
        }
        _ => {
            println!("Usage:");
            println!("  yeehaw worm exec <name>   Trigger a worm (called by cron)");
            println!("  yeehaw worm sync          Sync crontab with worm configs");
            println!("  yeehaw worm list          List all worms");
        }
    }
}

fn handle_trail_subcommand(args: &[String]) {
    match args.get(2).map(|s| s.as_str()) {
        Some("poll") => {
            let livestock_name = match args.get(3) {
                Some(name) => name,
                None => {
                    eprintln!("Usage: yeehaw trail poll <livestock> <trail>");
                    std::process::exit(1);
                }
            };
            let trail_name = match args.get(4) {
                Some(name) => name,
                None => {
                    eprintln!("Usage: yeehaw trail poll <livestock> <trail>");
                    std::process::exit(1);
                }
            };
            handle_trail_poll(livestock_name, trail_name);
        }
        _ => {
            println!("Usage:");
            println!("  yeehaw trail poll <livestock> <trail>   Poll for new commits and trigger trail");
        }
    }
}

/// The livestock named `livestock_name`, the barn to poll it on, and its
/// project.
///
/// The poller drives a trail on the machine the checkout lives on, over ssh, so
/// a livestock that lives *here* has no barn to poll it from and never had one:
/// before adoption it said `barn: None` and this search skipped it. Resolving
/// keeps that true afterwards, rather than letting adoption quietly point the
/// poller at this machine's own self-barn — a record with no host, which would
/// turn a clean "has no barn" into an ssh failure inside the run.
fn find_pollable_livestock(
    livestock_name: &str,
) -> Option<(types::Livestock, types::Barn, String)> {
    let barns = config::load_barns();
    for project in config::load_projects() {
        let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) else {
            continue;
        };
        let Some(barn_name) = config::resolve_livestock_barn(ls) else {
            continue;
        };
        if let Some(barn) = barns.iter().find(|b| b.name == barn_name) {
            return Some((ls.clone(), barn.clone(), project.name.clone()));
        }
    }
    None
}

fn handle_trail_poll(livestock_name: &str, trail_name: &str) {
    // 1. Find livestock across all projects
    let (livestock, barn, _project) = match find_pollable_livestock(livestock_name) {
        Some(f) => f,
        None => {
            eprintln!("Livestock '{}' not found or has no barn", livestock_name);
            std::process::exit(1);
        }
    };

    // 2. Load the trail
    let trail = match config::load_trail(trail_name) {
        Some(t) => t,
        None => {
            eprintln!("Trail '{}' not found", trail_name);
            std::process::exit(1);
        }
    };

    // 3. Get repo URL from livestock
    let repo_url = match livestock.repo.as_deref() {
        Some(url) => url,
        None => {
            eprintln!("Livestock '{}' has no repo URL configured", livestock_name);
            std::process::exit(1);
        }
    };

    // 4. Get branch - from trail trigger config, or livestock, or default "main"
    let branch = trail.push_branches()
        .and_then(|b| b.first())
        .map(|s| s.as_str())
        .or(livestock.branch.as_deref())
        .unwrap_or("main");

    // 5. Call check_and_trigger
    match trails::polling::check_and_trigger(
        livestock_name,
        trail_name,
        repo_url,
        branch,
        &barn,
    ) {
        Ok(true) => println!("New commits detected, trail triggered"),
        Ok(false) => println!("No new commits"),
        Err(e) => {
            eprintln!("Poll error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_tui() -> Result<()> {
    let mut terminal = ratatui::init();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        ratatui::restore();
        original_hook(panic_info);
    }));

    views::splash::run_splash(&mut terminal);
    let result = app::run(&mut terminal);
    ratatui::restore();
    result
}

fn run_mcp_server() -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(mcp_server::run())
}

fn run_worm_exec(worm_name: &str) -> Result<()> {
    // Write trigger file for the TUI to pick up
    let now = chrono::Utc::now();
    let filename = format!("{}-{}.json", worm_name, now.format("%Y-%m-%dT%H-%M-%S"));
    let trigger_path = config::worm_triggers_dir().join(&filename);

    let trigger = serde_json::json!({
        "worm": worm_name,
        "triggered_at": now.to_rfc3339(),
        "trigger": "cron"
    });

    // Bare `fs::write`, never `store::write_atomic` — and Task 11 of the Phase 1
    // plan is wrong to say "`grep -n 'fs::write' cli/src/main.rs` must return
    // nothing". This one has to stay: the watcher matches any path under
    // `worm-triggers/` and the handler deletes what it reads, so a temp file
    // here is consumed before the rename can publish it. See the invariant on
    // `config::worm_triggers_dir()`.
    std::fs::write(&trigger_path, trigger.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn livestock(name: &str, barn: Option<&str>) -> types::Livestock {
        types::Livestock {
            name: name.into(),
            path: format!("/tmp/{name}"),
            barn: barn.map(str::to_string),
            repo: Some("https://github.com/acme/api".into()),
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    fn barn(name: &str) -> types::Barn {
        types::Barn {
            name: name.into(),
            host: Some("10.0.0.2".into()),
            user: Some("forge".into()),
            port: None,
            identity_file: None,
            critters: vec![],
            ..Default::default()
        }
    }

    /// `yeehaw trail poll` drives a trail on the machine the checkout lives on,
    /// over ssh. A livestock that lives *here* has never been pollable — it
    /// said `barn: None` and this search skipped it — and adoption must not
    /// change that by quietly pointing the poller at this machine's own
    /// hostless self-barn.
    #[test]
    fn the_trail_poller_answers_the_same_before_and_after_adoption() {
        let _ranch = testing::temp_ranch();

        let mut pi = barn("pi");
        config::save_barn(&mut pi).unwrap();

        let mut project = types::Project {
            name: "api".into(),
            path: "/tmp/api".into(),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![livestock("web", None), livestock("worker", Some("pi"))],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_project(&mut project).unwrap();

        assert!(
            find_pollable_livestock("web").is_none(),
            "a livestock on this machine has no barn to poll it on"
        );
        assert_eq!(
            find_pollable_livestock("worker").map(|(_, b, _)| b.name),
            Some("pi".to_string())
        );

        // Rewrites `web` on disk to `barn: imac` and persists the self-barn.
        migrate::adopt_this_machine("imac").unwrap();

        assert!(
            find_pollable_livestock("web").is_none(),
            "adoption renamed the livestock; it did not give it somewhere to be polled from"
        );
        assert_eq!(
            find_pollable_livestock("worker").map(|(_, b, _)| b.name),
            Some("pi".to_string()),
            "a real remote barn is untouched by adoption"
        );
    }
}
