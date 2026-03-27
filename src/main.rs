mod app;
mod config;
mod context;
mod critters;
mod crontab;
mod detection;
mod editor;
mod git;
mod hooks;
mod issues;
mod mcp_server;
mod ranchhand_k8s;
mod ranchhand_terraform;
mod signals;
mod slack;
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
    // Ensure config directories exist
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
        Some("slack") => {
            handle_slack_subcommand(args);
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

fn handle_slack_subcommand(args: &[String]) {
    match args.get(2).map(|s| s.as_str()) {
        Some("auth") => {
            run_slack_auth();
        }
        Some("status") => {
            let cfg = config::load_config();
            let tokens = issues::auth::get_slack_tokens();

            println!("\x1b[36mSlack Integration Status\x1b[0m");
            println!();
            let enabled = cfg.slack.as_ref().map(|s| s.enabled).unwrap_or(false);
            println!("  Enabled:        {}", if enabled { "\x1b[32myes\x1b[0m" } else { "\x1b[31mno\x1b[0m" });
            println!("  Auth:           {}", if tokens.is_some() { "\x1b[32mconfigured\x1b[0m" } else { "\x1b[31mnot configured\x1b[0m" });
            if let Some(ref t) = tokens {
                if let Some(ref uid) = t.user_id {
                    println!("  Bot User ID:    {}", uid);
                }
            }
            let allowed = cfg.slack.as_ref().map(|s| s.allowed_users.join(", ")).unwrap_or_default();
            println!("  Allowed Users:  {}", if allowed.is_empty() { "none".to_string() } else { allowed });
            if let Some(ref s) = cfg.slack {
                if let Some(ref dp) = s.default_project {
                    println!("  Default Project: {}", dp);
                }
            }
        }
        Some("enable") => {
            let mut cfg = config::load_config();
            match cfg.slack.as_mut() {
                Some(s) => s.enabled = true,
                None => {
                    cfg.slack = Some(types::SlackConfig {
                        enabled: true,
                        allowed_users: vec![],
                        default_project: None,
                        channel_projects: None,
                        system_prompt: None,
                    });
                }
            }
            let content = serde_yaml::to_string(&cfg).unwrap_or_default();
            let _ = std::fs::write(config::config_file(), content);
            println!("\x1b[32m✓\x1b[0m Slack integration enabled");
            println!("Restart Yeehaw for changes to take effect.");
        }
        Some("disable") => {
            let mut cfg = config::load_config();
            if let Some(ref mut s) = cfg.slack {
                s.enabled = false;
            }
            let content = serde_yaml::to_string(&cfg).unwrap_or_default();
            let _ = std::fs::write(config::config_file(), content);
            println!("\x1b[32m✓\x1b[0m Slack integration disabled");
        }
        _ => {
            println!("Usage:");
            println!("  yeehaw slack auth         Configure Slack bot tokens");
            println!("  yeehaw slack status       Show Slack integration status");
            println!("  yeehaw slack enable       Enable Slack bot");
            println!("  yeehaw slack disable      Disable Slack bot");
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

fn handle_trail_poll(livestock_name: &str, trail_name: &str) {
    // 1. Find livestock across all projects
    let projects = config::load_projects();
    let mut found = None;
    for project in &projects {
        if let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) {
            if let Some(barn_name) = &ls.barn {
                if let Some(barn) = config::load_barns().into_iter().find(|b| &b.name == barn_name) {
                    found = Some((ls.clone(), barn, project.name.clone()));
                    break;
                }
            }
        }
    }

    let (livestock, barn, _project) = match found {
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
        barn.host.as_deref().unwrap_or(&barn.name),
        barn.user.as_deref().unwrap_or("root"),
        barn.port.unwrap_or(22),
        barn.identity_file.as_deref().unwrap_or(""),
    ) {
        Ok(true) => println!("New commits detected, trail triggered"),
        Ok(false) => println!("No new commits"),
        Err(e) => {
            eprintln!("Poll error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_slack_auth() {
    use std::io::{self, BufRead, Write};

    println!("\x1b[36mSlack Bot Setup\x1b[0m");
    println!();
    println!("You need two tokens from your Slack app:");
    println!("  1. Bot Token (xoxb-...) — from OAuth & Permissions");
    println!("  2. App Token (xapp-...) — from Socket Mode settings");
    println!();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    print!("Bot Token (xoxb-...): ");
    let _ = stdout.flush();
    let mut bot_token = String::new();
    stdin.lock().read_line(&mut bot_token).unwrap_or(0);
    let bot_token = bot_token.trim().to_string();

    if !bot_token.starts_with("xoxb-") {
        eprintln!("\x1b[31mError:\x1b[0m Bot token must start with xoxb-");
        std::process::exit(1);
    }

    print!("App Token (xapp-...): ");
    let _ = stdout.flush();
    let mut app_token = String::new();
    stdin.lock().read_line(&mut app_token).unwrap_or(0);
    let app_token = app_token.trim().to_string();

    if !app_token.starts_with("xapp-") {
        eprintln!("\x1b[31mError:\x1b[0m App token must start with xapp-");
        std::process::exit(1);
    }

    println!("\nValidating tokens...");

    // Validate via Slack auth.test API
    match ureq::post("https://slack.com/api/auth.test")
        .set("Authorization", &format!("Bearer {}", bot_token))
        .set("Content-Type", "application/json")
        .send_string("{}")
    {
        Ok(resp) => {
            if let Ok(data) = resp.into_json::<serde_json::Value>() {
                if data["ok"].as_bool() != Some(true) {
                    eprintln!("\x1b[31mError:\x1b[0m Slack API error: {}", data["error"].as_str().unwrap_or("unknown"));
                    std::process::exit(1);
                }

                let bot_user_id = data["user_id"].as_str().unwrap_or("");
                let user_name = data["user"].as_str().unwrap_or("unknown");
                println!("\x1b[32m✓\x1b[0m Bot authenticated as: {} ({})", user_name, bot_user_id);

                issues::auth::set_slack_tokens(&bot_token, &app_token, if bot_user_id.is_empty() { None } else { Some(bot_user_id) });
                println!("\x1b[32m✓\x1b[0m Tokens saved");

                println!();
                println!("\x1b[33mNote:\x1b[0m You need to add YOUR Slack user ID to the allowed list.");
                println!("To find your ID: click your profile in Slack → \"...\" → \"Copy member ID\"");
                println!();

                print!("Your Slack User ID (U...): ");
                let _ = stdout.flush();
                let mut human_id = String::new();
                stdin.lock().read_line(&mut human_id).unwrap_or(0);
                let human_id = human_id.trim().to_string();

                let mut cfg = config::load_config();
                if cfg.slack.is_none() {
                    cfg.slack = Some(types::SlackConfig {
                        enabled: false,
                        allowed_users: vec![],
                        default_project: None,
                        channel_projects: None,
                        system_prompt: None,
                    });
                }

                if human_id.starts_with('U') {
                    if let Some(ref mut s) = cfg.slack {
                        if !s.allowed_users.contains(&human_id) {
                            s.allowed_users.push(human_id.clone());
                            println!("\x1b[32m✓\x1b[0m Added {} to allowed_users", human_id);
                        } else {
                            println!("\x1b[32m✓\x1b[0m {} already in allowed_users", human_id);
                        }
                    }
                } else {
                    println!("\x1b[33mSkipped:\x1b[0m No valid user ID provided. Add it manually to ~/.yeehaw/config.yaml");
                }

                let content = serde_yaml::to_string(&cfg).unwrap_or_default();
                let _ = std::fs::write(config::config_file(), content);
                println!("\nNext: run \x1b[36myeehaw slack enable\x1b[0m to activate the bot");
            }
        }
        Err(e) => {
            eprintln!("\x1b[31mError:\x1b[0m Failed to validate: {}", e);
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

    std::fs::write(&trigger_path, trigger.to_string())?;
    Ok(())
}
