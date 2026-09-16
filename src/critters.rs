use std::process::Command;

use crate::config;
use crate::ssh;
use crate::tmux::shell_escape;
use crate::types::Barn;

/// Services interesting for developers
const INTERESTING_SERVICE_PATTERNS: &[&str] = &[
    // Databases
    "mysql", "mariadb", "postgres", "postgresql", "mongodb", "mongo",
    // Caches
    "redis", "memcached",
    // Web servers
    "nginx", "apache", "httpd", "caddy",
    // PHP
    "php-fpm", "php7", "php8",
    // Python
    "gunicorn", "uvicorn", "celery",
    // Node
    "node", "pm2",
    // Mail
    "postfix", "dovecot",
    // Queue
    "rabbitmq", "kafka",
    // Search
    "elasticsearch", "opensearch", "meilisearch",
    // Other
    "supervisor", "docker",
];

const SUPERVISOR_CONFIG_PATHS: &[&str] = &[
    "/etc/supervisor/conf.d",
    "/etc/supervisord.d",
    "/etc/supervisor.d",
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredCritter {
    pub service: String,
    pub suggested_name: String,
    pub command: Option<String>,
    pub binary: Option<String>,
    pub config_path: Option<String>,
    pub log_path: Option<String>,
    pub status: String, // "running", "stopped", "unknown"
    pub manager: String, // "systemd" or "supervisor"
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemService {
    pub name: String,
    pub state: String, // "running", "stopped", "unknown"
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceDetails {
    pub service_path: String,
    pub config_path: Option<String>,
    pub log_path: Option<String>,
    pub use_journald: bool,
}

fn is_interesting_service(name: &str) -> bool {
    let lower = name.to_lowercase();
    INTERESTING_SERVICE_PATTERNS.iter().any(|p| lower.contains(p))
}

fn extract_service_name(service: &str) -> String {
    service.strip_suffix(".service").unwrap_or(service).to_string()
}

fn is_supervisor_service(service: &str) -> bool {
    service.starts_with("supervisor:")
}

fn get_supervisor_program_name(service: &str) -> &str {
    service.strip_prefix("supervisor:").unwrap_or(service)
}

/// Run one discovery probe on `barn`.
///
/// `barn_is_this_machine`, not `is_local_barn`. The question is "do I run this
/// here or dial out", and after `migrate::adopt_this_machine` the record for
/// the machine this process is *on* carries that machine's real name. Asking
/// only about the synthetic `local` sent every probe aimed at the self-barn
/// down the ssh branch — to a record that deliberately has no host, because you
/// do not ssh to yourself — so `ssh_args` refused it, `run` came back `Err`,
/// and critter discovery on this machine silently found nothing.
fn run_command(barn: &Barn, cmd: &str) -> Option<String> {
    if config::barn_is_this_machine(barn) {
        Command::new("sh")
            .args(["-c", cmd])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    } else {
        // BatchMode: discovery runs unattended, so it must fail rather than
        // block on a password prompt. A barn with no identity_file is still
        // attempted — ssh-agent and ~/.ssh/config are valid ways to authenticate.
        ssh::run(barn, cmd, ssh::Opts { batch: true, ..Default::default() }).ok()
    }
}

fn run_command_allow_failure(barn: &Barn, cmd: &str) -> String {
    run_command(barn, cmd).unwrap_or_default()
}

fn parse_systemctl_show(output: &str) -> std::collections::HashMap<String, String> {
    let mut result = std::collections::HashMap::new();
    for line in output.lines() {
        if let Some(eq_idx) = line.find('=') {
            let key = line[..eq_idx].to_string();
            let value = line[eq_idx + 1..].to_string();
            result.insert(key, value);
        }
    }
    result
}

fn parse_supervisor_config(content: &str) -> std::collections::HashMap<String, std::collections::HashMap<String, String>> {
    let mut sections = std::collections::HashMap::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }

        // Section header
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].to_string();
            sections.entry(current_section.clone()).or_insert_with(std::collections::HashMap::new);
            continue;
        }

        // Key=value
        if !current_section.is_empty() {
            if let Some(eq_idx) = trimmed.find('=') {
                let key = trimmed[..eq_idx].trim().to_string();
                let value = trimmed[eq_idx + 1..].trim().to_string();
                if let Some(section) = sections.get_mut(&current_section) {
                    section.insert(key, value);
                }
            }
        }
    }

    sections
}

/// Whether discovery has any way to run a command on `barn` at all.
///
/// The bail-out earns its keep — supervisor discovery runs a dozen commands per
/// config directory, and a barn nothing can reach fails every one of them — but
/// it has to agree with [`run_command`] about what "reachable" means.
///
/// `barn_is_this_machine`, not `is_local_barn`: this machine is always
/// reachable, and its own record after adoption says `host: null` precisely
/// *because* there is nothing to dial. Read through `is_local_barn` that looked
/// identical to a remote barn missing its coordinates, so discovery gave up on
/// the machine it was running on before issuing a single command.
///
/// Named rather than inline so the decision can be asserted on directly: both
/// answers produce an empty result on a machine with no supervisor installed,
/// which is every developer machine this suite runs on.
fn can_run_commands(barn: &Barn) -> bool {
    config::barn_is_this_machine(barn) || (barn.host.is_some() && barn.user.is_some())
}

fn discover_supervisor_programs(barn: &Barn) -> Vec<DiscoveredCritter> {
    if !can_run_commands(barn) {
        return vec![];
    }

    let mut programs = Vec::new();

    for config_dir in SUPERVISOR_CONFIG_PATHS {
        let list_cmd = format!("ls -1 {}/*.conf {}/*.ini 2>/dev/null || true", config_dir, config_dir);
        let list_output = run_command_allow_failure(barn, &list_cmd);
        let config_files: Vec<&str> = list_output.lines().filter(|l| !l.trim().is_empty()).collect();

        for conf_path in config_files {
            let cat_cmd = format!("cat {} 2>/dev/null || true", shell_escape(conf_path));
            let config_content = run_command_allow_failure(barn, &cat_cmd);
            if config_content.trim().is_empty() {
                continue;
            }

            let sections = parse_supervisor_config(&config_content);

            for (section_name, section_data) in &sections {
                if !section_name.starts_with("program:") {
                    continue;
                }

                let program_name = section_name.strip_prefix("program:").unwrap_or(section_name);

                // Skip duplicates
                let service_id = format!("supervisor:{}", program_name);
                if programs.iter().any(|p: &DiscoveredCritter| p.service == service_id) {
                    continue;
                }

                let command = section_data.get("command").cloned();
                let binary = command.as_ref().and_then(|c| c.split_whitespace().next().map(|s| s.to_string()));

                let mut log_path = section_data.get("stdout_logfile")
                    .or_else(|| section_data.get("stderr_logfile"))
                    .or_else(|| section_data.get("logfile"))
                    .cloned();

                // Skip template paths
                if let Some(ref lp) = log_path {
                    if lp.contains("%(") || lp == "AUTO" || lp == "NONE" {
                        log_path = None;
                    }
                }

                programs.push(DiscoveredCritter {
                    service: service_id,
                    suggested_name: program_name.to_string(),
                    command,
                    binary,
                    config_path: Some(conf_path.to_string()),
                    log_path,
                    status: "unknown".to_string(),
                    manager: "supervisor".to_string(),
                });
            }
        }
    }

    programs
}

fn discover_systemd_services(barn: &Barn) -> Vec<DiscoveredCritter> {
    let list_cmd = "systemctl list-units --type=service --state=running --no-pager --plain 2>/dev/null || true";
    let output = run_command_allow_failure(barn, list_cmd);

    let mut discovered = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let service = parts[0];
        if !service.ends_with(".service") || !is_interesting_service(service) {
            continue;
        }

        let suggested_name = extract_service_name(service);

        // Try to get more details
        let mut binary = None;
        let mut config_path = None;

        let show_cmd = format!("systemctl show {} --property=ExecStart", shell_escape(service));
        if let Some(show_output) = run_command(barn, &show_cmd) {
            let props = parse_systemctl_show(&show_output);
            if let Some(exec_start) = props.get("ExecStart") {
                // Extract binary from ExecStart
                if let Some(cap) = exec_start.find("path=") {
                    let rest = &exec_start[cap + 5..];
                    if let Some(end) = rest.find(|c: char| c.is_whitespace() || c == ';') {
                        binary = Some(rest[..end].to_string());
                    }
                }
                // Try to find config flags
                if let Some(cap) = exec_start.find("--config") {
                    let rest = &exec_start[cap..];
                    let value_start = rest.find(|c: char| c == '=' || c == ' ').map(|i| i + 1).unwrap_or(0);
                    if value_start > 0 && value_start < rest.len() {
                        let value_rest = &rest[value_start..];
                        if let Some(end) = value_rest.find(|c: char| c.is_whitespace() || c == ';') {
                            config_path = Some(value_rest[..end].to_string());
                        } else {
                            config_path = Some(value_rest.to_string());
                        }
                    }
                }
            }
        }

        discovered.push(DiscoveredCritter {
            service: service.to_string(),
            suggested_name,
            command: None,
            binary,
            config_path,
            log_path: None,
            status: "running".to_string(),
            manager: "systemd".to_string(),
        });
    }

    discovered
}

/// Discover critters (running services) on a barn.
/// Checks both systemd and Supervisor.
pub fn discover_critters(barn: &Barn) -> (Vec<DiscoveredCritter>, Option<String>) {
    let mut discovered = Vec::new();
    let mut errors = Vec::new();

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| discover_systemd_services(barn))) {
        Ok(critters) => discovered.extend(critters),
        Err(_) => errors.push("systemd: discovery panicked".to_string()),
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| discover_supervisor_programs(barn))) {
        Ok(critters) => discovered.extend(critters),
        Err(_) => errors.push("supervisor: discovery panicked".to_string()),
    }

    let error = if errors.is_empty() { None } else { Some(errors.join("; ")) };
    (discovered, error)
}

/// List systemd services on a barn
pub fn list_system_services(barn: &Barn, active_only: bool) -> (Vec<SystemService>, Option<String>) {
    let cmd = if active_only {
        "systemctl list-units --type=service --state=running --no-pager --no-legend"
    } else {
        "systemctl list-unit-files --type=service --no-pager --no-legend"
    };

    let output = match run_command(barn, cmd) {
        Some(o) => o,
        None => return (vec![], Some("Failed to list services".to_string())),
    };

    let mut services = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let name = parts[0];
        if !name.ends_with(".service") {
            continue;
        }

        if active_only {
            services.push(SystemService {
                name: name.to_string(),
                state: "running".to_string(),
                description: if parts.len() > 4 { Some(parts[4..].join(" ")) } else { None },
            });
        } else {
            let state_str = parts.get(1).unwrap_or(&"").to_lowercase();
            services.push(SystemService {
                name: name.to_string(),
                state: if state_str == "enabled" { "unknown".to_string() } else { "stopped".to_string() },
                description: None,
            });
        }
    }

    (services, None)
}

/// Get details about a service (systemd or Supervisor)
pub fn get_service_details(barn: &Barn, service_name: &str) -> Result<ServiceDetails, String> {
    if is_supervisor_service(service_name) {
        let program_name = get_supervisor_program_name(service_name);
        get_supervisor_program_details(barn, program_name)
    } else {
        get_systemd_service_details(barn, service_name)
    }
}

fn get_systemd_service_details(barn: &Barn, service_name: &str) -> Result<ServiceDetails, String> {
    let path_cmd = format!("systemctl show -p FragmentPath {} --value", shell_escape(service_name));
    let service_path = run_command(barn, &path_cmd)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("Could not find service unit file")?;

    let cat_cmd = format!("cat {}", shell_escape(&service_path));
    let content = run_command(barn, &cat_cmd)
        .ok_or("Failed to read service file")?;

    let mut config_path = None;
    let mut log_path = None;
    let mut use_journald = true;

    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(exec_line) = trimmed.strip_prefix("ExecStart=") {
            let config_patterns = [
                "--config=", "--config ", "--defaults-file=", "--defaults-file ",
                "-c ", "--conf=", "--conf ",
            ];
            for pat in &config_patterns {
                if let Some(idx) = exec_line.find(pat) {
                    let value_start = idx + pat.len();
                    let rest = &exec_line[value_start..];
                    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                    config_path = Some(rest[..end].to_string());
                    break;
                }
            }
        }

        if trimmed.starts_with("StandardOutput=") || trimmed.starts_with("StandardError=") {
            if let Some(value) = trimmed.split('=').nth(1) {
                if !value.starts_with("journal") && !value.starts_with("inherit") {
                    if let Some(rest) = value.strip_prefix("file:").or_else(|| value.strip_prefix("append:")) {
                        log_path = Some(rest.to_string());
                        use_journald = false;
                    }
                }
            }
        }
    }

    Ok(ServiceDetails { service_path, config_path, log_path, use_journald })
}

fn get_supervisor_program_details(barn: &Barn, program_name: &str) -> Result<ServiceDetails, String> {
    // Try direct file name match first
    for config_dir in SUPERVISOR_CONFIG_PATHS {
        for ext in &["conf", "ini"] {
            let conf_path = format!("{}/{}.{}", config_dir, program_name, ext);
            let cat_cmd = format!("cat {} 2>/dev/null", shell_escape(&conf_path));
            if let Some(content) = run_command(barn, &cat_cmd) {
                if !content.trim().is_empty() {
                    let sections = parse_supervisor_config(&content);
                    let key = format!("program:{}", program_name);
                    if let Some(section) = sections.get(&key) {
                        return Ok(extract_details_from_program_section(&conf_path, section));
                    }
                }
            }
        }
    }

    // Fallback: scan all config files
    for config_dir in SUPERVISOR_CONFIG_PATHS {
        let list_cmd = format!("ls -1 {}/*.conf {}/*.ini 2>/dev/null || true", config_dir, config_dir);
        let list_output = run_command_allow_failure(barn, &list_cmd);

        for conf_path in list_output.lines().filter(|l| !l.trim().is_empty()) {
            let cat_cmd = format!("cat {} 2>/dev/null || true", shell_escape(conf_path));
            let content = run_command_allow_failure(barn, &cat_cmd);
            if content.trim().is_empty() {
                continue;
            }

            let sections = parse_supervisor_config(&content);
            let key = format!("program:{}", program_name);
            if let Some(section) = sections.get(&key) {
                return Ok(extract_details_from_program_section(conf_path, section));
            }
        }
    }

    Err(format!("Could not find config file for Supervisor program '{}'", program_name))
}

fn extract_details_from_program_section(
    conf_path: &str,
    section: &std::collections::HashMap<String, String>,
) -> ServiceDetails {
    let mut config_path = None;

    // Check command for config flags
    if let Some(command) = section.get("command") {
        let config_patterns = [
            "--config=", "--config ", "-c ", "--conf=", "--conf ", "--settings=", "--settings ",
        ];
        for pat in &config_patterns {
            if let Some(idx) = command.find(pat) {
                let value_start = idx + pat.len();
                let rest = &command[value_start..];
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                config_path = Some(rest[..end].to_string());
                break;
            }
        }
    }

    // Get log path
    let mut log_path = section.get("stdout_logfile")
        .or_else(|| section.get("stderr_logfile"))
        .or_else(|| section.get("logfile"))
        .cloned();

    // Skip template paths
    if let Some(ref lp) = log_path {
        if lp.contains("%(") || lp == "AUTO" || lp == "NONE" {
            log_path = None;
        }
    }

    ServiceDetails {
        service_path: conf_path.to_string(),
        config_path,
        log_path,
        use_journald: false,
    }
}

/// Read logs from a critter
pub fn read_critter_logs(
    critter: &crate::types::Critter,
    barn: &Barn,
    lines: u32,
    pattern: Option<&str>,
) -> Result<String, String> {
    let lines_str = lines.to_string();

    let cmd = if is_supervisor_service(&critter.service) {
        let program_name = get_supervisor_program_name(&critter.service);
        if let Some(ref lp) = critter.log_path {
            let base = format!("tail -n {} {}", lines_str, shell_escape(lp));
            match pattern {
                Some(pat) => format!("{} | grep -i {} || true", base, shell_escape(pat)),
                None => base,
            }
        } else {
            let base = format!("supervisorctl tail -{} {}", lines_str, shell_escape(program_name));
            match pattern {
                Some(pat) => format!("{} | grep -i {} || true", base, shell_escape(pat)),
                None => base,
            }
        }
    } else if critter.use_journald.unwrap_or(true) {
        let base = format!("journalctl -u {} -n {} --no-pager", shell_escape(&critter.service), lines_str);
        match pattern {
            Some(pat) => format!("{} | grep -i {} || true", base, shell_escape(pat)),
            None => base,
        }
    } else if let Some(ref lp) = critter.log_path {
        let base = format!("tail -n {} {}", lines_str, shell_escape(lp));
        match pattern {
            Some(pat) => format!("{} | grep -i {} || true", base, shell_escape(pat)),
            None => base,
        }
    } else {
        return Err("Critter has no log_path and use_journald is disabled".to_string());
    };

    run_command(barn, &cmd).ok_or_else(|| format!("Failed to read logs for {}", critter.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This machine's own barn record, as critter discovery receives it after
    /// `migrate::adopt_this_machine`.
    ///
    /// `addresses` is emptied on purpose, and the emptying is what makes the
    /// tests below safe to run on a live ranch. That list holds what a *peer*
    /// dials to reach this machine — `ssh::dial_host` falls back to it — so a
    /// test that took the ssh branch by mistake would open a real ssh
    /// connection to the developer's own machine and write its host key into
    /// their `~/.ssh/known_hosts`. Emptied, the ssh branch has nothing to dial
    /// and fails before spawning anything, which is what lets an assertion
    /// about *which branch ran* mean something.
    ///
    /// The state is not invented: the live ranch's self-barn was exactly this —
    /// `host: null`, no addresses — until `advertise_this_machine` was added.
    fn adopted_self_barn(name: &str) -> Barn {
        crate::migrate::adopt_this_machine(name).unwrap();
        let mut barn = config::load_barns()
            .into_iter()
            .find(|b| b.name == name)
            .expect("adoption mints this machine's record");
        barn.addresses.clear();
        barn
    }

    /// THE BUG. Critter discovery runs its probes through `run_command`, and
    /// after adoption the barn for the machine it is probing carries that
    /// machine's real name. `is_local_barn` only ever knew the synthetic
    /// `local`, so every probe aimed at this machine took the ssh branch — at a
    /// record that deliberately has no host, because you do not ssh to
    /// yourself. Discovery on the self-barn found nothing at all.
    #[test]
    fn a_probe_at_this_machines_own_barn_runs_here_instead_of_ssh_to_itself() {
        let _ranch = crate::testing::temp_ranch();
        let imac = adopted_self_barn("imac");
        // Both halves of the control. The first says this barn *is* the ssh
        // branch under the old rule; the second says that branch has nothing to
        // dial, so it cannot accidentally succeed.
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");
        assert!(crate::ssh::dial_host(&imac).is_none(), "control: nothing to dial");

        let out = run_command(&imac, "echo yeehaw-ran-here")
            .expect("a probe at this machine has to run on this machine");
        assert_eq!(out.trim(), "yeehaw-ran-here");
    }

    /// The synthetic row is still this machine on a ranch nobody has adopted,
    /// which is the only state it survives into.
    #[test]
    fn a_probe_at_the_synthetic_local_barn_still_runs_here() {
        let _ranch = crate::testing::temp_ranch();

        let out = run_command(&config::local_barn(), "echo yeehaw-ran-here")
            .expect("`local` has always meant this machine");
        assert_eq!(out.trim(), "yeehaw-ran-here");
    }

    /// And another machine's barn still goes over ssh, or discovery would
    /// report this machine's services as some remote host's.
    ///
    /// The barn has nothing to dial, so "took the ssh branch" is observable as
    /// `None` without a network round trip — had the command run here instead,
    /// it would have come back `Some("not-here")`.
    #[test]
    fn a_probe_at_another_machine_still_goes_over_ssh() {
        let _ranch = crate::testing::temp_ranch();
        let _imac = adopted_self_barn("imac");

        let pi = Barn { name: "pi".into(), ..Default::default() };
        assert!(
            run_command(&pi, "echo not-here").is_none(),
            "a probe aimed at another machine must not run on this one"
        );
    }

    /// The same bug one layer up. Supervisor discovery bails early when it has
    /// no way in, and it read "no way in" as "no host" — which is precisely
    /// what this machine's own record says about itself. Discovery of local
    /// supervisor programs stopped before it ran a single command.
    #[test]
    fn discovery_still_has_a_way_into_this_machines_own_barn() {
        let _ranch = crate::testing::temp_ranch();
        let imac = adopted_self_barn("imac");
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");
        assert!(imac.host.is_none(), "control: a self-barn has no host to bail on");

        assert!(
            can_run_commands(&imac),
            "discovery gave up on the machine it is running on"
        );
    }

    /// And the bail-out still bails where it was meant to: a remote barn with
    /// no coordinates cannot be probed, and running a dozen commands that will
    /// each fail is worse than returning nothing.
    #[test]
    fn a_remote_barn_with_no_way_in_is_still_skipped() {
        let _ranch = crate::testing::temp_ranch();

        let unreachable = Barn { name: "pi".into(), ..Default::default() };
        assert!(!can_run_commands(&unreachable));

        let half = Barn { name: "pi".into(), host: Some("pi.local".into()), ..Default::default() };
        assert!(!can_run_commands(&half), "a host with no user is still no way in");

        let reachable = Barn {
            name: "pi".into(),
            host: Some("pi.local".into()),
            user: Some("cam".into()),
            ..Default::default()
        };
        assert!(can_run_commands(&reachable));
    }
}
