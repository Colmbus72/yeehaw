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
mod ranchhand_k8s;
mod ranchhand_terraform;
mod remote_grid;
mod signals;
mod ssh;
mod store;
mod sync;
#[cfg(test)]
mod testing;
mod trails;
mod tmux;
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
        Some("brand") => {
            handle_brand_subcommand(args);
            true
        }
        Some("sync") => {
            if let Err(e) = sync::engine::run_cli(&args[2..]) {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
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
                println!("\x1b[33mNote:\x1b[0m Sessions Yeehaw launches pass this config to");
                println!("`claude --settings` themselves, so they need nothing else — and nothing");
                println!("in ~/.claude/settings.json. This command is for Claude sessions you start");
                println!("yourself in a terminal, which Yeehaw has no hand in launching.");

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
        println!("Note: Sessions Yeehaw launches already pass these hooks to `claude --settings`");
        println!("on the command line. This is only for Claude sessions you start yourself.");
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

/// `yeehaw brand <name>`: brand this machine as barn `<name>`, so livestock
/// recorded here names a real barn rather than the synthetic `local`.
fn handle_brand_subcommand(args: &[String]) {
    let Some(name) = args.get(2) else {
        println!("Usage: yeehaw brand <name>");
        println!();
        println!("Brand this machine as barn <name>: records it in barns/<name>.yaml and");
        println!("pins every livestock that lives here to that barn.");
        return;
    };
    // `{:#}`, not `{}`: `adopt_this_machine`'s refusals are the whole of the
    // user's guidance — an unparseable project's path, or the barn that turns
    // out to be some other machine. Plain `{}` renders only the outermost
    // context and throws every one of them away.
    match migrate::adopt_this_machine(name) {
        Ok(report) => {
            println!("This machine is now barn '{}'.", name);
            if report.barn_created {
                println!("  created barns/{}.yaml", name);
            }
            if report.livestock_reassigned > 0 {
                println!(
                    "  pinned {} livestock to '{}' across {} project(s)",
                    report.livestock_reassigned,
                    name,
                    report.projects_touched.len()
                );
            }
        }
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m {:#}", e);
            std::process::exit(1);
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

    /// `yeehaw brand <name>` is the one way in to adoption: it has to leave this
    /// machine named, with its own barn record.
    #[test]
    fn brand_names_this_machine_as_the_barn_given() {
        let _ranch = testing::temp_ranch();
        let args: Vec<String> = ["yeehaw", "brand", "imac"].iter().map(|s| s.to_string()).collect();

        assert!(handle_subcommands(&args), "`brand` is not a subcommand");

        assert_eq!(config::this_barn_name().as_deref(), Some("imac"));
        assert!(
            config::load_barns().iter().any(|b| b.name == "imac"),
            "branding did not persist this machine's barn"
        );
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
