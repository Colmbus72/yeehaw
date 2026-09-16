use anyhow::Result;
use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{
    prompt, prompt_handler, prompt_router, tool, tool_handler, tool_router, ErrorData as McpError,
    RoleServer, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::critters;
use crate::crontab;
use crate::hooks;
use crate::ranchhand_k8s;
use crate::ranchhand_terraform;
use crate::types;

// ============================================================================
// MCP server-level instructions
// ============================================================================
//
// This text is injected into Claude's system prompt every time a client connects to
// the Yeehaw MCP server. Keep it tight: it pays per-token on every session.
const YEEHAW_INSTRUCTIONS: &str = "Yeehaw is the user's ranch — the source of truth for their projects, infrastructure, and Claude sessions. Treat it as the main brain of this user's setup. Whenever the user references *their own* projects, servers, deployments, or running services, check Yeehaw before assuming context is missing.\n\n\
Vocabulary maps to real things the user owns:\n\
- Projects — codebases / products. Each has a wiki with long-term context (architecture, conventions, commands, gotchas, common tasks) — read it via get_wiki / get_wiki_section before asking the user to re-explain their codebase.\n\
- Barns — servers / hosts. (\"the server\", \"production\", \"staging\" → look here.)\n\
- Livestock — deployments / processes the user ships and runs on barns. (\"the API\", \"the app\", \"the worker\" → look here.)\n\
- Critters — system-level processes that support livestock (MySQL, php-fpm, nginx, redis, etc.). (\"the database\", \"the web server\", \"the queue\" → look here.)\n\
- Herds — groups of related livestock (a service tier, an environment).\n\
- Ranch Hands — infrastructure automation runners (k8s, terraform); discover and sync resources from them.\n\
- Worms — scheduled jobs / cron triggers.\n\
- Trails — multi-step automations the user has saved.\n\n\
Default to list_projects / get_project / list_barns / list_herds early in any task that touches the user's own systems. The yeehaw-project-setup prompt is available for configuring a new project's metadata and wiki.";

// ============================================================================
// Parameter structs
// ============================================================================

#[derive(Deserialize, JsonSchema)]
struct NameParam {
    /// Entity name
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct ProjectNameParam {
    /// Project name
    project: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateProjectParams {
    /// Project name
    name: String,
    /// Local path to project
    path: String,
    /// Short description
    summary: Option<String>,
    /// Hex color
    color: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateProjectParams {
    /// Project name to update
    name: String,
    /// New summary
    summary: Option<String>,
    /// New hex color
    color: Option<String>,
    /// New path
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct DeleteConfirmParams {
    /// Name to delete
    name: String,
    /// Must match name to confirm deletion
    confirm: String,
}

#[derive(Deserialize, JsonSchema)]
struct AddLivestockParams {
    /// Project name
    project: String,
    /// Livestock name
    name: String,
    /// Path (local or remote)
    path: String,
    /// Barn name for remote livestock
    barn: Option<String>,
    /// Git repository URL
    repo: Option<String>,
    /// Git branch
    branch: Option<String>,
    /// Path to logs relative to livestock path
    log_path: Option<String>,
    /// Path to env file relative to livestock path
    env_path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct RemoveLivestockParams {
    /// Project name
    project: String,
    /// Livestock name to remove
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct ReadLogsParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Last N lines (default: 100)
    lines: Option<u32>,
    /// Grep pattern to filter logs
    pattern: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadEnvParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Show values (default: false)
    show_values: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct CreateBarnParams {
    /// Barn name
    name: String,
    /// Hostname or IP address
    host: String,
    /// SSH username
    user: String,
    /// SSH port (default: 22)
    port: Option<u16>,
    /// Path to SSH private key
    identity_file: String,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateBarnParams {
    /// Barn name to update
    name: String,
    /// New hostname
    host: Option<String>,
    /// New SSH username
    user: Option<String>,
    /// New SSH port
    port: Option<u16>,
    /// New SSH key path
    identity_file: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct WikiSectionParams {
    /// Project name
    project: String,
    /// Section title
    title: String,
}

#[derive(Deserialize, JsonSchema)]
struct AddWikiSectionParams {
    /// Project name
    project: String,
    /// Section title
    title: String,
    /// Section content (markdown)
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateWikiSectionParams {
    /// Project name
    project: String,
    /// Section title to update
    title: String,
    /// New title
    new_title: Option<String>,
    /// New content
    content: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct AddCritterParams {
    /// Barn name
    barn: String,
    /// Critter name
    name: String,
    /// systemd service name
    service: String,
    /// Path to config file
    config_path: Option<String>,
    /// Custom log path
    log_path: Option<String>,
    /// Use journalctl for logs (default: true)
    use_journald: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct RemoveCritterParams {
    /// Barn name
    barn: String,
    /// Critter name
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct ReadCritterLogsParams {
    /// Barn name
    barn: String,
    /// Critter name
    critter: String,
    /// Last N lines (default: 100)
    lines: Option<u32>,
    /// Grep pattern
    pattern: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct HerdNameParams {
    /// Project name
    project: String,
    /// Herd name
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct HerdLivestockParams {
    /// Project name
    project: String,
    /// Herd name
    herd: String,
    /// Livestock name
    livestock: String,
}

#[derive(Deserialize, JsonSchema)]
struct HerdCritterParams {
    /// Project name
    project: String,
    /// Herd name
    herd: String,
    /// Barn name
    barn: String,
    /// Critter name
    critter: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateWormParams {
    /// Worm name
    name: String,
    /// Command (shell) or prompt (claude)
    command: String,
    /// Cron expression
    schedule: String,
    /// Worm type: shell or claude
    worm_type: String,
    /// Project association
    project: Option<String>,
    /// Working directory
    working_dir: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateWormParams {
    /// Worm name to update
    name: String,
    /// New command/prompt
    command: Option<String>,
    /// New cron expression
    schedule: Option<String>,
    /// New project association
    project: Option<String>,
    /// New working directory
    working_dir: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ToggleWormParams {
    /// Worm name
    name: String,
    /// Set enabled state (omit to toggle)
    enabled: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct ListWormRunsParams {
    /// Worm name
    name: String,
    /// Max runs to return (default: 20)
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadWormRunLogParams {
    /// Worm name
    worm: String,
    /// Run started_at timestamp
    run_timestamp: String,
}

// RanchHand parameter structs

#[derive(Deserialize, JsonSchema)]
struct DiscoverCrittersParams {
    /// Barn name to discover services on
    barn: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateRanchHandParams {
    /// RanchHand name
    name: String,
    /// Project name
    project: String,
    /// Type: kubernetes or terraform
    rh_type: String,
    /// Config (YAML object) - context, kubeconfig_path, etc.
    config: serde_json::Value,
    /// Herd name to sync into
    herd: String,
}

#[derive(Deserialize, JsonSchema)]
struct DiscoverRanchHandResourcesParams {
    /// RanchHand name
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct SyncRanchHandParams {
    /// RanchHand name
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct SelectRanchHandHerdsParams {
    /// RanchHand name
    name: String,
    /// Comma-separated list of herd/namespace names to sync
    herds: String,
}

#[derive(Deserialize, JsonSchema)]
struct AssignRanchHandResourceToHerdParams {
    /// RanchHand name
    ranchhand: String,
    /// Resource ID (e.g., aws_db_instance.postgres)
    resource_id: String,
    /// Herd name to assign to
    herd: String,
}

#[derive(Deserialize, JsonSchema)]
struct GetKubectlContextsParams {
    /// Path to kubeconfig file (optional)
    kubeconfig_path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ListTerraformStateFilesParams {
    /// S3 bucket name
    bucket: String,
    /// S3 key prefix
    prefix: String,
    /// AWS region
    region: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateTrailParams {
    /// Trail name
    name: String,
    /// Full trail YAML content (GHA-compatible format)
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateTrailParams {
    /// Trail name to update
    name: String,
    /// New trail YAML content
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct LinkTrailParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
}

#[derive(Deserialize, JsonSchema)]
struct RunTrailParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
}

#[derive(Deserialize, JsonSchema)]
struct ListTrailRunsParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
    /// Max runs to return (default: 20)
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct GetTrailRunParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
    /// Run timestamp (from list_trail_runs started_at field)
    run_timestamp: String,
}

#[derive(Deserialize, JsonSchema)]
struct ReadTrailStepLogParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
    /// Run timestamp
    run_timestamp: String,
    /// Step index (0-based)
    step: usize,
}

// ============================================================================
// MCP Server
// ============================================================================

#[derive(Clone)]
pub struct YeehawServer {
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

fn ok_text(text: &str) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

fn ok_json<T: Serialize>(val: &T) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(val).unwrap_or_default();
    ok_text(&text)
}

fn err_text(text: &str) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![Content::text(text)]))
}

fn find_project(name: &str) -> Option<types::Project> {
    config::load_projects().into_iter().find(|p| p.name == name)
}

fn find_barn(name: &str) -> Option<types::Barn> {
    config::load_barns().into_iter().find(|b| b.name == name)
}

fn find_worm(name: &str) -> Option<types::Worm> {
    config::load_worms().into_iter().find(|w| w.name == name)
}

fn find_ranchhand(name: &str) -> Option<types::RanchHand> {
    config::load_ranchhands().into_iter().find(|rh| rh.name == name)
}

#[tool_router]
impl YeehawServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    // === Project Tools ===

    #[tool(description = "List all Yeehaw projects")]
    async fn list_projects(&self) -> Result<CallToolResult, McpError> {
        let projects = config::load_projects();
        let simplified: Vec<serde_json::Value> = projects.iter().map(|p| {
            serde_json::json!({
                "name": p.name,
                "path": p.path,
                "summary": p.summary,
                "color": p.color,
                "livestock": p.livestock.iter().map(|l| &l.name).collect::<Vec<_>>(),
            })
        }).collect();
        ok_json(&simplified)
    }

    #[tool(description = "Get details of a specific project including its livestock")]
    async fn get_project(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        match find_project(&params.0.name) {
            Some(project) => ok_json(&project),
            None => err_text(&format!("Project '{}' not found", params.0.name)),
        }
    }

    #[tool(description = "Create a new project")]
    async fn create_project(&self, params: Parameters<CreateProjectParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = types::Project {
            name: p.name, path: p.path, summary: p.summary, color: p.color,
            gradient_spread: None, gradient_inverted: None,
            livestock: vec![], herds: vec![], wiki: vec![],
            issue_provider: None, wiki_provider: None,
            id: None, created_at: None, updated_at: None,
        };
        // `create_*`, not `save_*`: a plain save writes over whatever is at
        // that name, and this struct's empty livestock/herds/wiki plus a fresh
        // uuid would replace the existing entity outright.
        match config::create_project(&mut project) {
            Ok(()) => ok_json(&project),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Update an existing project")]
    async fn update_project(&self, params: Parameters<UpdateProjectParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.name) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.name)),
        };
        if let Some(summary) = p.summary { project.summary = Some(summary); }
        if let Some(color) = p.color { project.color = Some(color); }
        if let Some(path) = p.path { project.path = path; }
        match config::save_project(&mut project) {
            Ok(()) => ok_json(&project),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Delete a project (requires confirmation)")]
    async fn delete_project(&self, params: Parameters<DeleteConfirmParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.name != p.confirm {
            return err_text("Confirmation name does not match");
        }
        match config::delete_project(&p.name) {
            Ok(true) => ok_text(&format!("Project '{}' deleted", p.name)),
            Ok(false) => err_text(&format!("Project '{}' not found", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    // === Livestock Tools ===

    #[tool(description = "Add livestock (deployed app instance) to a project")]
    async fn add_livestock(&self, params: Parameters<AddLivestockParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        // The other writer of `livestock.barn`, and the one that can be handed
        // the literal `local` by an agent. Same rule as the TUI's picker: it is
        // machine-relative with a different spelling than `None`.
        let barn = p.barn.as_deref().and_then(config::stored_barn_name);
        let livestock = types::Livestock {
            name: p.name, path: p.path, barn, repo: p.repo,
            branch: p.branch, log_path: p.log_path, env_path: p.env_path,
            source: None, k8s_metadata: None, trails: vec![],
        };
        match config::add_livestock_to_project(&p.project, &livestock) {
            Ok(()) => ok_json(&livestock),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Remove livestock from a project")]
    async fn remove_livestock(&self, params: Parameters<RemoveLivestockParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let before = project.livestock.len();
        project.livestock.retain(|l| l.name != p.name);
        if project.livestock.len() == before {
            return err_text(&format!("Livestock '{}' not found", p.name));
        }
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Livestock '{}' removed", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Read log files from a livestock deployment")]
    async fn read_livestock_logs(&self, params: Parameters<ReadLogsParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let livestock = match project.livestock.iter().find(|l| l.name == p.livestock) {
            Some(l) => l,
            None => return err_text(&format!("Livestock '{}' not found", p.livestock)),
        };
        let log_path = match &livestock.log_path {
            Some(lp) => lp.clone(),
            None => return err_text("No log_path configured"),
        };
        let full_path = if log_path.starts_with('/') { log_path } else { format!("{}/{}", livestock.path, log_path) };
        let lines = p.lines.unwrap_or(100);
        // Resolved, not read raw: after adoption this machine's own livestock
        // names a real barn record with no host, and reading its logs over ssh
        // fails outright for a file sitting right here.
        let barn = config::resolve_livestock_barn(livestock).and_then(find_barn);

        let output = if let Some(barn) = barn.filter(|b| !config::barn_is_this_machine(b)) {
            // No stat available over ssh, so the trailing slash is the only signal.
            let cmd = build_log_command(&full_path, lines, p.pattern.as_deref(), full_path.ends_with('/'));
            read_remote_output(&barn, &cmd)
        } else {
            read_local_logs(&full_path, lines, p.pattern.as_deref())
        };
        ok_text(&output)
    }

    #[tool(description = "Read environment config from a livestock deployment")]
    async fn read_livestock_env(&self, params: Parameters<ReadEnvParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let livestock = match project.livestock.iter().find(|l| l.name == p.livestock) {
            Some(l) => l,
            None => return err_text(&format!("Livestock '{}' not found", p.livestock)),
        };
        let env_path = match &livestock.env_path {
            Some(ep) => ep.clone(),
            None => return err_text("No env_path configured"),
        };
        let full_path = if env_path.starts_with('/') { env_path } else { format!("{}/{}", livestock.path, env_path) };
        let show_values = p.show_values.unwrap_or(false);

        match std::fs::read_to_string(&full_path) {
            Ok(content) => {
                if show_values { ok_text(&content) } else {
                    let keys: Vec<String> = content.lines()
                        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                        .filter_map(|l| l.split('=').next().map(|k| k.to_string()))
                        .collect();
                    ok_text(&keys.join("\n"))
                }
            }
            Err(e) => err_text(&format!("Failed to read env: {}", e)),
        }
    }

    // === Barn Tools ===

    #[tool(description = "List all Yeehaw barns (servers)")]
    async fn list_barns(&self) -> Result<CallToolResult, McpError> {
        let barns = config::load_barns();
        let simplified: Vec<serde_json::Value> = barns.iter().map(|b| {
            serde_json::json!({
                "name": b.name, "host": b.host, "user": b.user, "port": b.port,
                "critters": b.critters.iter().map(|c| &c.name).collect::<Vec<_>>(),
            })
        }).collect();
        ok_json(&simplified)
    }

    #[tool(description = "Get details of a specific barn including deployed livestock")]
    async fn get_barn(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        let barn = match find_barn(&params.0.name) {
            Some(b) => b,
            None => return err_text(&format!("Barn '{}' not found", params.0.name)),
        };
        let livestock = config::get_livestock_for_barn(&barn.name);
        let result = serde_json::json!({
            "name": barn.name, "host": barn.host, "user": barn.user,
            "port": barn.port, "identity_file": barn.identity_file,
            "critters": barn.critters,
            "deployed_livestock": livestock.iter().map(|(proj, ls)| {
                serde_json::json!({"project": proj.name, "name": ls.name, "path": ls.path})
            }).collect::<Vec<_>>(),
        });
        ok_json(&result)
    }

    #[tool(description = "Create a new barn (server)")]
    async fn create_barn(&self, params: Parameters<CreateBarnParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.name == config::LOCAL_BARN_NAME { return err_text("Cannot create a barn named 'local'"); }
        let mut barn = types::Barn {
            name: p.name, host: Some(p.host), user: Some(p.user),
            port: Some(p.port.unwrap_or(22)), identity_file: Some(p.identity_file),
            ..Default::default()
        };
        match config::create_barn(&mut barn) {
            Ok(()) => ok_json(&barn),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Update an existing barn")]
    async fn update_barn(&self, params: Parameters<UpdateBarnParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.name == config::LOCAL_BARN_NAME { return err_text("Cannot update the local barn"); }
        let mut barn = match find_barn(&p.name) {
            Some(b) => b,
            None => return err_text(&format!("Barn '{}' not found", p.name)),
        };
        if let Some(host) = p.host { barn.host = Some(host); }
        if let Some(user) = p.user { barn.user = Some(user); }
        if let Some(port) = p.port { barn.port = Some(port); }
        if let Some(key) = p.identity_file { barn.identity_file = Some(key); }
        match config::save_barn(&mut barn) {
            Ok(()) => ok_json(&barn),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Delete a barn (requires confirmation)")]
    async fn delete_barn(&self, params: Parameters<DeleteConfirmParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.name != p.confirm { return err_text("Confirmation does not match"); }
        if p.name == config::LOCAL_BARN_NAME { return err_text("Cannot delete the local barn"); }
        match config::delete_barn(&p.name) {
            Ok(true) => ok_text(&format!("Barn '{}' deleted", p.name)),
            Ok(false) => err_text(&format!("Barn '{}' not found", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    // === Wiki Tools ===

    #[tool(description = "Get all wiki section titles for a project")]
    async fn get_wiki(&self, params: Parameters<ProjectNameParam>) -> Result<CallToolResult, McpError> {
        let project = match find_project(&params.0.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", params.0.project)),
        };
        let titles: Vec<&str> = project.wiki.iter().map(|s| s.title.as_str()).collect();
        ok_json(&titles)
    }

    #[tool(description = "Get the content of a specific wiki section")]
    async fn get_wiki_section(&self, params: Parameters<WikiSectionParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        match project.wiki.iter().find(|s| s.title == p.title) {
            Some(section) => ok_text(&section.content),
            None => err_text(&format!("Wiki section '{}' not found", p.title)),
        }
    }

    #[tool(description = "Add a new wiki section to a project")]
    async fn add_wiki_section(&self, params: Parameters<AddWikiSectionParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        if project.wiki.iter().any(|s| s.title == p.title) {
            return err_text(&format!("Section '{}' already exists", p.title));
        }
        project.wiki.push(types::WikiSection { title: p.title.clone(), content: p.content });
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Section '{}' added", p.title)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Update an existing wiki section")]
    async fn update_wiki_section(&self, params: Parameters<UpdateWikiSectionParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let section = match project.wiki.iter_mut().find(|s| s.title == p.title) {
            Some(s) => s,
            None => return err_text(&format!("Section '{}' not found", p.title)),
        };
        if let Some(new_title) = p.new_title { section.title = new_title; }
        if let Some(content) = p.content { section.content = content; }
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Section '{}' updated", p.title)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Delete a wiki section from a project")]
    async fn delete_wiki_section(&self, params: Parameters<WikiSectionParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let before = project.wiki.len();
        project.wiki.retain(|s| s.title != p.title);
        if project.wiki.len() == before {
            return err_text(&format!("Section '{}' not found", p.title));
        }
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Section '{}' deleted", p.title)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    // === Critter Tools ===

    #[tool(description = "Add a critter (system service) to a barn")]
    async fn add_critter(&self, params: Parameters<AddCritterParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut barn = match find_barn(&p.barn) {
            Some(b) => b,
            None => return err_text(&format!("Barn '{}' not found", p.barn)),
        };
        if barn.critters.iter().any(|c| c.name == p.name) {
            return err_text(&format!("Critter '{}' already exists", p.name));
        }
        barn.critters.push(types::Critter {
            name: p.name.clone(), service: p.service, service_path: None,
            config_path: p.config_path, log_path: p.log_path,
            use_journald: Some(p.use_journald.unwrap_or(true)),
            source: None, endpoint: None, port: None,
            k8s_metadata: None, tf_metadata: None,
        });
        match config::save_barn(&mut barn) {
            Ok(()) => ok_text(&format!("Critter '{}' added to barn '{}'", p.name, p.barn)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Remove a critter from a barn")]
    async fn remove_critter(&self, params: Parameters<RemoveCritterParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut barn = match find_barn(&p.barn) {
            Some(b) => b,
            None => return err_text(&format!("Barn '{}' not found", p.barn)),
        };
        let before = barn.critters.len();
        barn.critters.retain(|c| c.name != p.name);
        if barn.critters.len() == before {
            return err_text(&format!("Critter '{}' not found", p.name));
        }
        match config::save_barn(&mut barn) {
            Ok(()) => ok_text(&format!("Critter '{}' removed", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Read logs from a critter (via journald or custom path)")]
    async fn read_critter_logs(&self, params: Parameters<ReadCritterLogsParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let barn = match find_barn(&p.barn) {
            Some(b) => b,
            None => return err_text(&format!("Barn '{}' not found", p.barn)),
        };
        let critter = match barn.critters.iter().find(|c| c.name == p.critter) {
            Some(c) => c,
            None => return err_text(&format!("Critter '{}' not found", p.critter)),
        };
        let lines = p.lines.unwrap_or(100);
        let use_journald = critter.use_journald.unwrap_or(true);

        let cmd = if use_journald {
            // A systemd unit name is never home-relative, so a leading `~/` in it
            // should stay literal: single_quote, not shell_escape.
            let base = format!(
                "journalctl -u {} -n {} --no-pager",
                crate::tmux::single_quote(&critter.service),
                lines
            );
            append_grep(base, p.pattern.as_deref())
        } else if let Some(log_path) = &critter.log_path {
            // A critter log_path has always been read as a single file; keep that.
            build_log_command(log_path, lines, p.pattern.as_deref(), false)
        } else {
            return err_text("No log source configured");
        };

        // "Is this the machine I am running on?" — run the log command here, or
        // over ssh. The same question `read_livestock_logs` above already asks
        // with `barn_is_this_machine`; asking only about the synthetic `local`
        // sent a critter on this machine's own adopted barn over ssh to a record
        // with no host.
        if config::barn_is_this_machine(&barn) {
            let output = std::process::Command::new("sh").args(["-c", &cmd]).output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|e| format!("Failed: {}", e));
            ok_text(&output)
        } else {
            ok_text(&read_remote_output(&barn, &cmd))
        }
    }

    // === Herd Tools ===

    #[tool(description = "List all herds in a project")]
    async fn list_herds(&self, params: Parameters<ProjectNameParam>) -> Result<CallToolResult, McpError> {
        let project = match find_project(&params.0.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", params.0.project)),
        };
        let herds: Vec<serde_json::Value> = project.herds.iter().map(|h| {
            serde_json::json!({
                "name": h.name, "livestock": h.livestock,
                "critters": h.critters.iter().map(|c| format!("{}/{}", c.barn, c.critter)).collect::<Vec<_>>(),
            })
        }).collect();
        ok_json(&herds)
    }

    #[tool(description = "Get details of a specific herd")]
    async fn get_herd(&self, params: Parameters<HerdNameParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        match project.herds.iter().find(|h| h.name == p.name) {
            Some(herd) => ok_json(herd),
            None => err_text(&format!("Herd '{}' not found", p.name)),
        }
    }

    #[tool(description = "Create a new herd in a project")]
    async fn create_herd(&self, params: Parameters<HerdNameParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        if project.herds.iter().any(|h| h.name == p.name) {
            return err_text(&format!("Herd '{}' already exists", p.name));
        }
        project.herds.push(types::Herd {
            name: p.name.clone(), livestock: vec![], critters: vec![], connections: vec![],
        });
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Herd '{}' created", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Delete a herd from a project")]
    async fn delete_herd(&self, params: Parameters<HerdNameParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let before = project.herds.len();
        project.herds.retain(|h| h.name != p.name);
        if project.herds.len() == before {
            return err_text(&format!("Herd '{}' not found", p.name));
        }
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Herd '{}' deleted", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Add a livestock to a herd")]
    async fn add_livestock_to_herd(&self, params: Parameters<HerdLivestockParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        if !project.livestock.iter().any(|l| l.name == p.livestock) {
            return err_text(&format!("Livestock '{}' not found", p.livestock));
        }
        for herd in &project.herds {
            if herd.livestock.contains(&p.livestock) {
                return err_text(&format!("Livestock '{}' already in herd '{}'", p.livestock, herd.name));
            }
        }
        let herd = match project.herds.iter_mut().find(|h| h.name == p.herd) {
            Some(h) => h,
            None => return err_text(&format!("Herd '{}' not found", p.herd)),
        };
        herd.livestock.push(p.livestock.clone());
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Livestock '{}' added to herd '{}'", p.livestock, p.herd)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Remove a livestock from a herd")]
    async fn remove_livestock_from_herd(&self, params: Parameters<HerdLivestockParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let herd = match project.herds.iter_mut().find(|h| h.name == p.herd) {
            Some(h) => h,
            None => return err_text(&format!("Herd '{}' not found", p.herd)),
        };
        let before = herd.livestock.len();
        herd.livestock.retain(|l| l != &p.livestock);
        if herd.livestock.len() == before {
            return err_text(&format!("Livestock '{}' not in herd '{}'", p.livestock, p.herd));
        }
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Livestock '{}' removed from herd '{}'", p.livestock, p.herd)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Add a critter reference to a herd")]
    async fn add_critter_to_herd(&self, params: Parameters<HerdCritterParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let herd = match project.herds.iter_mut().find(|h| h.name == p.herd) {
            Some(h) => h,
            None => return err_text(&format!("Herd '{}' not found", p.herd)),
        };
        if herd.critters.iter().any(|c| c.barn == p.barn && c.critter == p.critter) {
            return err_text("Critter already in herd");
        }
        herd.critters.push(types::HerdCritterRef { barn: p.barn.clone(), critter: p.critter.clone() });
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Critter '{}/{}' added to herd '{}'", p.barn, p.critter, p.herd)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Remove a critter reference from a herd")]
    async fn remove_critter_from_herd(&self, params: Parameters<HerdCritterParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut project = match find_project(&p.project) {
            Some(proj) => proj,
            None => return err_text(&format!("Project '{}' not found", p.project)),
        };
        let herd = match project.herds.iter_mut().find(|h| h.name == p.herd) {
            Some(h) => h,
            None => return err_text(&format!("Herd '{}' not found", p.herd)),
        };
        let before = herd.critters.len();
        herd.critters.retain(|c| !(c.barn == p.barn && c.critter == p.critter));
        if herd.critters.len() == before {
            return err_text("Critter not in herd");
        }
        match config::save_project(&mut project) {
            Ok(()) => ok_text(&format!("Critter removed from herd '{}'", p.herd)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    // === Worm Tools ===

    #[tool(description = "List all worms (scheduled commands)")]
    async fn list_worms(&self) -> Result<CallToolResult, McpError> {
        let worms = config::load_worms();
        let simplified: Vec<serde_json::Value> = worms.iter().map(|w| {
            let cmd_preview: String = w.command.chars().take(100).collect();
            serde_json::json!({
                "name": w.name, "type": w.worm_type, "schedule": w.schedule,
                "enabled": w.enabled, "command": cmd_preview, "project": w.project,
            })
        }).collect();
        ok_json(&simplified)
    }

    #[tool(description = "Get details of a specific worm")]
    async fn get_worm(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        match find_worm(&params.0.name) {
            Some(worm) => ok_json(&worm),
            None => err_text(&format!("Worm '{}' not found", params.0.name)),
        }
    }

    #[tool(description = "Create a new worm (scheduled command)")]
    async fn create_worm(&self, params: Parameters<CreateWormParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.worm_type != "shell" && p.worm_type != "claude" {
            return err_text("Worm type must be 'shell' or 'claude'");
        }
        let mut worm = types::Worm {
            name: p.name, command: p.command, schedule: p.schedule,
            worm_type: p.worm_type, enabled: true, project: p.project, working_dir: p.working_dir,
            id: None, created_at: None, updated_at: None,
        };
        match config::create_worm(&mut worm) {
            Ok(()) => { let _ = crontab::sync_crontab(); ok_json(&worm) }
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Update an existing worm")]
    async fn update_worm(&self, params: Parameters<UpdateWormParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut worm = match find_worm(&p.name) {
            Some(w) => w,
            None => return err_text(&format!("Worm '{}' not found", p.name)),
        };
        if let Some(command) = p.command { worm.command = command; }
        if let Some(schedule) = p.schedule { worm.schedule = schedule; }
        if let Some(project) = p.project { worm.project = Some(project); }
        if let Some(working_dir) = p.working_dir { worm.working_dir = Some(working_dir); }
        match config::save_worm(&mut worm) {
            Ok(()) => { let _ = crontab::sync_crontab(); ok_json(&worm) }
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Delete a worm")]
    async fn delete_worm(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        match config::delete_worm(&params.0.name) {
            Ok(true) => { let _ = crontab::sync_crontab(); ok_text(&format!("Worm '{}' deleted", params.0.name)) }
            Ok(false) => err_text(&format!("Worm '{}' not found", params.0.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Enable or disable a worm")]
    async fn toggle_worm(&self, params: Parameters<ToggleWormParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut worm = match find_worm(&p.name) {
            Some(w) => w,
            None => return err_text(&format!("Worm '{}' not found", p.name)),
        };
        worm.enabled = p.enabled.unwrap_or(!worm.enabled);
        match config::save_worm(&mut worm) {
            Ok(()) => {
                let _ = crontab::sync_crontab();
                let state = if worm.enabled { "enabled" } else { "disabled" };
                ok_text(&format!("Worm '{}' {}", worm.name, state))
            }
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Get run history for a worm")]
    async fn list_worm_runs(&self, params: Parameters<ListWormRunsParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let runs = config::load_worm_runs(&p.name);
        let limit = p.limit.unwrap_or(20);
        let limited: Vec<_> = runs.into_iter().take(limit).collect();
        ok_json(&limited)
    }

    #[tool(description = "Read the output log of a specific worm run")]
    async fn read_worm_run_log(&self, params: Parameters<ReadWormRunLogParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let runs = config::load_worm_runs(&p.worm);
        let run = match runs.iter().find(|r| r.started_at == p.run_timestamp) {
            Some(r) => r,
            None => return err_text("Run not found"),
        };
        let log_path = config::worm_runs_for(&p.worm).join(&run.log_file);
        match std::fs::read_to_string(&log_path) {
            Ok(content) => ok_text(&content),
            Err(e) => err_text(&format!("Failed to read log: {}", e)),
        }
    }

    #[tool(description = "Manually trigger a worm to run immediately")]
    async fn run_worm_now(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        if find_worm(&params.0.name).is_none() {
            return err_text(&format!("Worm '{}' not found", params.0.name));
        }
        let now = chrono::Utc::now();
        let filename = format!("{}-{}.json", params.0.name, now.format("%Y-%m-%dT%H-%M-%S"));
        let trigger_path = config::worm_triggers_dir().join(&filename);
        let trigger = serde_json::json!({
            "worm": params.0.name,
            "triggered_at": now.to_rfc3339(),
            "trigger": "manual"
        });
        // Bare `fs::write`, never `store::write_atomic`: the watcher would consume
        // and delete the temp file before the rename could publish it. See the
        // invariant on `config::worm_triggers_dir()`.
        match std::fs::write(&trigger_path, trigger.to_string()) {
            Ok(()) => ok_text(&format!("Worm '{}' triggered", params.0.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    // === Critter Discovery ===

    #[tool(description = "Discover running services (critters) on a barn")]
    async fn discover_critters(&self, params: Parameters<DiscoverCrittersParams>) -> Result<CallToolResult, McpError> {
        let barn = match find_barn(&params.0.barn) {
            Some(b) => b,
            None => return err_text(&format!("Barn '{}' not found", params.0.barn)),
        };
        let (discovered, error) = critters::discover_critters(&barn);
        let mut result = serde_json::json!({ "critters": discovered });
        if let Some(err) = error {
            result["warning"] = serde_json::Value::String(err);
        }
        ok_json(&result)
    }

    // === RanchHand Tools ===

    #[tool(description = "List all ranch hands for a project")]
    async fn list_ranchhands(&self, params: Parameters<ProjectNameParam>) -> Result<CallToolResult, McpError> {
        let ranchhands = config::load_ranchhands_for_project(&params.0.project);
        ok_json(&ranchhands)
    }

    #[tool(description = "Get details of a specific ranch hand")]
    async fn get_ranchhand(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        match find_ranchhand(&params.0.name) {
            Some(rh) => ok_json(&rh),
            None => err_text(&format!("RanchHand '{}' not found", params.0.name)),
        }
    }

    #[tool(description = "Create a new ranch hand (K8s or Terraform sync)")]
    async fn create_ranchhand(&self, params: Parameters<CreateRanchHandParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.rh_type != "kubernetes" && p.rh_type != "terraform" {
            return err_text("RanchHand type must be 'kubernetes' or 'terraform'");
        }
        // Convert JSON config to YAML value
        let config_yaml: serde_yaml::Value = serde_json::from_value(
            serde_json::to_value(&p.config).unwrap_or_default()
        ).unwrap_or_default();

        let mut rh = types::RanchHand {
            name: p.name,
            project: p.project,
            rh_type: p.rh_type,
            config: config_yaml,
            sync_settings: types::RanchHandSyncSettings {
                auto_sync: false,
                interval_minutes: None,
            },
            herd: p.herd,
            resource_mappings: vec![],
            last_sync: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        match config::create_ranchhand(&mut rh) {
            Ok(()) => ok_json(&rh),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Delete a ranch hand")]
    async fn delete_ranchhand(&self, params: Parameters<DeleteConfirmParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.name != p.confirm {
            return err_text("Confirmation does not match");
        }
        match config::delete_ranchhand(&p.name) {
            Ok(true) => ok_text(&format!("RanchHand '{}' deleted", p.name)),
            Ok(false) => err_text(&format!("RanchHand '{}' not found", p.name)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Discover resources from a ranch hand's infrastructure")]
    async fn discover_ranchhand_resources(&self, params: Parameters<DiscoverRanchHandResourcesParams>) -> Result<CallToolResult, McpError> {
        let rh = match find_ranchhand(&params.0.name) {
            Some(rh) => rh,
            None => return err_text(&format!("RanchHand '{}' not found", params.0.name)),
        };

        if rh.rh_type == "kubernetes" {
            let context = rh.config["context"].as_str().unwrap_or("");
            let kubeconfig = rh.config["kubeconfig_path"].as_str();
            let registries: Vec<String> = rh.config["private_registries"]
                .as_sequence()
                .map(|seq| seq.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            match ranchhand_k8s::discover_k8s_resources(context, kubeconfig, &registries) {
                Ok(result) => ok_json(&result),
                Err(e) => err_text(&format!("K8s discovery failed: {}", e)),
            }
        } else if rh.rh_type == "terraform" {
            let project = match find_project(&rh.project) {
                Some(p) => p,
                None => return err_text(&format!("Project '{}' not found", rh.project)),
            };
            let herds: Vec<String> = project.herds.iter().map(|h| h.name.clone()).collect();
            match ranchhand_terraform::discover_terraform_resources(&rh.config, &herds) {
                Ok(result) => ok_json(&result),
                Err(e) => err_text(&format!("Terraform discovery failed: {}", e)),
            }
        } else {
            err_text(&format!("Unknown ranchhand type: {}", rh.rh_type))
        }
    }

    #[tool(description = "Select which herds/namespaces to sync from a ranch hand")]
    async fn select_ranchhand_herds(&self, params: Parameters<SelectRanchHandHerdsParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let selected_herds: Vec<&str> = p.herds.split(',').map(|h| h.trim()).filter(|h| !h.is_empty()).collect();
        let mut rh = match find_ranchhand(&p.name) {
            Some(rh) => rh,
            None => return err_text(&format!("Ranch hand not found: {}", p.name)),
        };
        rh.herd = selected_herds.first().map(|s| s.to_string()).unwrap_or_default();
        match config::save_ranchhand(&mut rh) {
            Ok(()) => ok_text(&format!("Updated ranch hand '{}' to sync herd: {}", p.name, if rh.herd.is_empty() { "(none)" } else { &rh.herd })),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Assign a Terraform resource to a specific herd (for resources that could not be auto-matched)")]
    async fn assign_ranchhand_resource_to_herd(&self, params: Parameters<AssignRanchHandResourceToHerdParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        match config::add_ranchhand_resource_mapping(&p.ranchhand, &p.resource_id, &p.herd) {
            Ok(()) => ok_text(&format!("Assigned resource '{}' to herd '{}'", p.resource_id, p.herd)),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "Sync resources from a ranch hand into the project")]
    async fn sync_ranchhand(&self, params: Parameters<SyncRanchHandParams>) -> Result<CallToolResult, McpError> {
        let rh_name = params.0.name.clone();
        let rh = match find_ranchhand(&rh_name) {
            Some(rh) => rh,
            None => return err_text(&format!("RanchHand '{}' not found", rh_name)),
        };

        if rh.herd.is_empty() {
            return err_text(&format!("Ranch hand '{}' has no herd assigned. Use select_ranchhand_herds first.", rh_name));
        }

        let mut project = match find_project(&rh.project) {
            Some(p) => p,
            None => return err_text(&format!("Project '{}' not found", rh.project)),
        };

        let sync_summary;

        if rh.rh_type == "kubernetes" {
            let mut result = match ranchhand_k8s::sync_k8s_resources(&rh) {
                Ok(r) => r,
                Err(e) => return err_text(&format!("K8s sync failed: {}", e)),
            };

            // Save barns (create if not exists). Iterated mutably so
            // `save_barn` stamps the barn that stays in `result` too, rather
            // than a throwaway clone.
            for barn in &mut result.barns {
                if find_barn(&barn.name).is_none() {
                    let _ = config::save_barn(barn);
                }
            }

            // Add livestock to project (if not already there)
            let existing_ls_names: Vec<String> = project.livestock.iter().map(|l| l.name.clone()).collect();
            for ls in &result.livestock {
                if !existing_ls_names.contains(&ls.name) {
                    project.livestock.push(ls.clone());
                }
            }

            // Add/update herds
            for herd in &result.herds {
                if let Some(existing) = project.herds.iter_mut().find(|h| h.name == herd.name) {
                    // Merge livestock
                    for ls_name in &herd.livestock {
                        if !existing.livestock.contains(ls_name) {
                            existing.livestock.push(ls_name.clone());
                        }
                    }
                    // Merge critters
                    for cr_ref in &herd.critters {
                        if !existing.critters.iter().any(|c| c.critter == cr_ref.critter && c.barn == cr_ref.barn) {
                            existing.critters.push(cr_ref.clone());
                        }
                    }
                } else {
                    project.herds.push(herd.clone());
                }
            }

            let _ = config::save_project(&mut project);
            let _ = config::update_ranchhand_last_sync(&rh_name);

            sync_summary = format!("Synced from K8s: {} barns, {} livestock, {} critters, {} herds",
                result.barns.len(), result.livestock.len(), result.critters.len(), result.herds.len());
        } else if rh.rh_type == "terraform" {
            let mut result = match ranchhand_terraform::sync_terraform_resources(&rh) {
                Ok(r) => r,
                Err(e) => return err_text(&format!("Terraform sync failed: {}", e)),
            };

            // See the K8s branch: mutable so the stamp lands on the barn that
            // `result` keeps holding.
            for barn in &mut result.barns {
                if find_barn(&barn.name).is_none() {
                    let _ = config::save_barn(barn);
                }
            }

            // For Terraform critters, add to a synthetic "terraform-managed" barn
            if !result.critters.is_empty() {
                let mut tf_barn = find_barn("terraform-managed").unwrap_or_else(|| types::Barn {
                    name: "terraform-managed".to_string(),
                    host: None,
                    user: None,
                    port: None,
                    identity_file: None,
                    critters: vec![],
                    source: Some(format!("ranchhand:{}", rh_name)),
                    connection_type: Some("terraform".to_string()),
                    connection_config: None,
                    connectable: Some(false),
                    ..Default::default()
                });
                for critter in &result.critters {
                    if !tf_barn.critters.iter().any(|c| c.name == critter.name) {
                        tf_barn.critters.push(critter.clone());
                    }
                }
                let _ = config::save_barn(&mut tf_barn);
            }

            let _ = config::save_project(&mut project);
            let _ = config::update_ranchhand_last_sync(&rh_name);

            sync_summary = format!("Synced from Terraform: {} barns, {} critters",
                result.barns.len(), result.critters.len());
        } else {
            return err_text(&format!("Unknown ranchhand type: {}", rh.rh_type));
        }

        ok_text(&sync_summary)
    }

    #[tool(description = "Get available kubectl contexts from kubeconfig")]
    async fn get_kubectl_contexts(&self, params: Parameters<GetKubectlContextsParams>) -> Result<CallToolResult, McpError> {
        match ranchhand_k8s::get_kubectl_contexts(params.0.kubeconfig_path.as_deref()) {
            Ok(contexts) => ok_json(&contexts),
            Err(e) => err_text(&format!("Failed: {}", e)),
        }
    }

    #[tool(description = "List Terraform state files in an S3 bucket")]
    async fn list_terraform_state_files(&self, params: Parameters<ListTerraformStateFilesParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let files = ranchhand_terraform::list_s3_state_files(&p.bucket, &p.prefix, &p.region);
        ok_json(&files)
    }

    // ========================================================================
    // Trails
    // ========================================================================

    #[tool(description = "List all trail definitions")]
    async fn list_trails(&self) -> Result<CallToolResult, McpError> {
        let trails = config::load_all_trails();
        let names: Vec<&str> = trails.iter().map(|t| t.name.as_str()).collect();
        ok_json(&names)
    }

    #[tool(description = "Get trail YAML content and metadata")]
    async fn get_trail(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        match config::load_trail(&p.name) {
            Some(trail) => ok_json(&trail),
            None => err_text(&format!("Trail '{}' not found", p.name)),
        }
    }

    #[tool(description = "Create a new trail from GHA-compatible YAML content")]
    async fn create_trail(&self, params: Parameters<CreateTrailParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut trail: crate::trails::Trail = match serde_yaml::from_str(&p.content) {
            Ok(t) => t,
            Err(e) => return err_text(&format!("Invalid trail YAML: {}", e)),
        };
        if trail.name != p.name {
            return err_text(&format!(
                "Trail name in YAML ('{}') doesn't match parameter ('{}')",
                trail.name, p.name
            ));
        }
        if trail.jobs.is_empty() {
            return err_text("Trail must have at least one job");
        }
        match config::create_trail(&mut trail) {
            Ok(_) => ok_text(&format!("Trail '{}' created", p.name)),
            Err(e) => err_text(&format!("Failed to save trail: {}", e)),
        }
    }

    #[tool(description = "Update an existing trail with new YAML content")]
    async fn update_trail(&self, params: Parameters<UpdateTrailParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let existing = match config::load_trail(&p.name) {
            Some(t) => t,
            None => return err_text(&format!("Trail '{}' not found", p.name)),
        };
        let mut trail: crate::trails::Trail = match serde_yaml::from_str(&p.content) {
            Ok(t) => t,
            Err(e) => return err_text(&format!("Invalid trail YAML: {}", e)),
        };
        // The same guard `create_trail` has. `save_trail` writes to
        // `trails/<yaml name>.yaml`, so without it an update whose YAML names a
        // different trail is an unguarded rename: it writes a second file
        // carrying this trail's uuid while the original file keeps it too.
        // Renaming a trail is not something this tool offers; say so.
        if trail.name != p.name {
            return err_text(&format!(
                "Trail name in YAML ('{}') doesn't match parameter ('{}')",
                trail.name, p.name
            ));
        }
        // This replaces the file wholesale from caller-supplied YAML, which
        // normally carries no meta fields. Without the carry-forward the update
        // would mint a fresh uuid and read downstream as a delete plus a
        // create.
        //
        // `existing` first, deliberately. The other order lets the payload win,
        // so YAML copied from another trail silently re-parents this one onto
        // that trail's uuid. Identity belongs to the file on disk; the caller
        // is supplying content, not identity.
        trail.id = existing.id.or(trail.id);
        trail.created_at = existing.created_at.or(trail.created_at);
        match config::save_trail(&mut trail) {
            Ok(_) => ok_text(&format!("Trail '{}' updated", p.name)),
            Err(e) => err_text(&format!("Failed to update trail: {}", e)),
        }
    }

    #[tool(description = "Delete a trail definition")]
    async fn delete_trail(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        match config::delete_trail(&p.name) {
            Ok(true) => ok_text(&format!("Trail '{}' deleted", p.name)),
            Ok(false) => err_text(&format!("Trail '{}' not found", p.name)),
            Err(e) => err_text(&format!("Failed to delete trail: {}", e)),
        }
    }

    #[tool(description = "Link a trail to a livestock (attaches trail for execution on that livestock's barn)")]
    async fn link_trail(&self, params: Parameters<LinkTrailParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        match config::link_trail_to_livestock(&p.project, &p.livestock, &p.trail) {
            Ok(_) => ok_text(&format!("Trail '{}' linked to '{}'", p.trail, p.livestock)),
            Err(e) => err_text(&format!("Failed to link trail: {}", e)),
        }
    }

    #[tool(description = "Unlink a trail from a livestock")]
    async fn unlink_trail(&self, params: Parameters<LinkTrailParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        match config::unlink_trail_from_livestock(&p.project, &p.livestock, &p.trail) {
            Ok(_) => ok_text(&format!("Trail '{}' unlinked from '{}'", p.trail, p.livestock)),
            Err(e) => err_text(&format!("Failed to unlink trail: {}", e)),
        }
    }

    #[tool(description = "Trigger a trail run on a livestock (executes via SSH on the livestock's barn)")]
    async fn run_trail(&self, params: Parameters<RunTrailParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let now = chrono::Utc::now();
        let filename = format!(
            "mcp-trail-{}--{}--{}.json",
            p.livestock, p.trail, now.format("%Y-%m-%dT%H-%M-%S")
        );
        let trigger = serde_json::json!({
            "worm": format!("trail--{}--{}", p.livestock, p.trail),
            "triggered_at": now.to_rfc3339(),
            "trigger": "mcp",
            "livestock": p.livestock,
            "trail": p.trail,
            "project": p.project,
        });
        let trigger_path = config::worm_triggers_dir().join(&filename);
        std::fs::create_dir_all(config::worm_triggers_dir())
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Bare `fs::write`, never `store::write_atomic`: the watcher would consume
        // and delete the temp file before the rename could publish it. See the
        // invariant on `config::worm_triggers_dir()`.
        std::fs::write(&trigger_path, serde_json::to_string_pretty(&trigger).unwrap())
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        ok_text(&format!(
            "Trail '{}' triggered on '{}'. Run ID: {}",
            p.trail, p.livestock, now.format("%Y-%m-%dT%H-%M-%S")
        ))
    }

    #[tool(description = "List trail run history for a specific livestock and trail")]
    async fn list_trail_runs(&self, params: Parameters<ListTrailRunsParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let mut runs = config::load_trail_runs(&p.livestock, &p.trail);
        let limit = p.limit.unwrap_or(20) as usize;
        runs.truncate(limit);
        ok_json(&runs)
    }

    #[tool(description = "Get details of a specific trail run (step statuses, timing, exit codes)")]
    async fn get_trail_run(&self, params: Parameters<GetTrailRunParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let run_dir = config::trail_run_dir_for(&p.livestock, &p.trail, &p.run_timestamp);
        let run_path = run_dir.join("run.json");
        match std::fs::read_to_string(&run_path) {
            Ok(content) => {
                let run: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
                ok_json(&run)
            }
            Err(_) => err_text(&format!(
                "Run not found: {}/{}/{}",
                p.livestock, p.trail, p.run_timestamp
            )),
        }
    }

    #[tool(description = "Read stdout/stderr log for a specific step in a trail run")]
    async fn read_trail_step_log(&self, params: Parameters<ReadTrailStepLogParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let run_dir = config::trail_run_dir_for(&p.livestock, &p.trail, &p.run_timestamp);
        match config::load_trail_step_log(&run_dir, p.step) {
            Some(log) => ok_text(&log),
            None => err_text(&format!(
                "Step log not found: step {} in {}/{}/{}",
                p.step, p.livestock, p.trail, p.run_timestamp
            )),
        }
    }
}

// ============================================================================
// ServerHandler
// ============================================================================

// ============================================================================
// Prompts
// ============================================================================

#[prompt_router]
impl YeehawServer {
    /// Configure a Yeehaw project's metadata and wiki by exploring the codebase.
    ///
    /// Returns the bundled `yeehaw-project-setup` skill body as a user-role prompt
    /// message — when invoked, the model receives the SKILL.md instructions as if
    /// the user pasted them, and follows the workflow (codebase exploration, color
    /// discovery, summary generation, wiki population).
    #[prompt(
        name = "yeehaw-project-setup",
        description = "Configure a Yeehaw project with an auto-generated summary, brand color, and wiki sections. Use when the user has created a Yeehaw project and wants to populate its metadata and wiki from the codebase."
    )]
    async fn yeehaw_project_setup_prompt(&self) -> Vec<PromptMessage> {
        let body = match hooks::read_skill_markdown() {
            Ok(s) => s.to_string(),
            Err(e) => format!(
                "Failed to load yeehaw-project-setup skill from embedded archive: {e}"
            ),
        };
        vec![PromptMessage::new_text(PromptMessageRole::User, body)]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for YeehawServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(YEEHAW_INSTRUCTIONS.into()),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Append `| grep -i <pattern>` to a log-reading pipeline, safely.
///
/// The pattern comes straight off an MCP tool parameter, so it is attacker text:
/// whichever agent is driving this server chooses it. It is quoted with
/// `single_quote` rather than `shell_escape` on purpose — a grep pattern that
/// begins with `~/` must match the literal characters `~/`, not expand to the
/// home directory the way a path would.
///
/// The `--` stops grep from reading a pattern like `-r` as an option; quoting
/// alone makes the value one word but says nothing about how grep parses it.
fn append_grep(base: String, pattern: Option<&str>) -> String {
    match pattern {
        Some(pat) => format!("{} | grep -i -- {}", base, crate::tmux::single_quote(pat)),
        None => base,
    }
}

/// Build the shell pipeline that reads the last `lines` lines of logs at `path`,
/// optionally filtered by `pattern`.
///
/// Shared by the local (`sh -c`) and remote (ssh) readers so both get identical
/// quoting. `path` goes through `shell_escape`, which keeps a leading `~/`
/// working as a home-relative path — livestock and critter paths are routinely
/// written that way — while making every other byte inert. `lines` is a `u32`,
/// so it can only ever render as digits and needs no escaping.
///
/// `treat_as_dir` is the caller's decision, not something inferred here: the
/// local reader can stat the path, the remote one only has the trailing slash.
fn build_log_command(path: &str, lines: u32, pattern: Option<&str>, treat_as_dir: bool) -> String {
    let path = crate::tmux::shell_escape(path);
    let base = if treat_as_dir {
        format!(
            "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n {} 2>/dev/null",
            path, lines
        )
    } else {
        format!("tail -n {} {}", lines, path)
    };
    append_grep(base, pattern)
}

fn read_local_logs(path: &str, lines: u32, pattern: Option<&str>) -> String {
    let treat_as_dir = path.ends_with('/') || std::path::Path::new(path).is_dir();
    let cmd = build_log_command(path, lines, pattern, treat_as_dir);
    std::process::Command::new("sh").args(["-c", &cmd]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_else(|e| format!("Failed: {}", e))
}

fn read_remote_output(barn: &types::Barn, cmd: &str) -> String {
    // BatchMode: the MCP server has no terminal, so an auth prompt would hang it.
    // allow_failure: every caller is a log read whose pipeline can end in `grep`,
    // which exits 1 on no matches. read_local_logs above ignores the exit status
    // entirely, so without this the same empty search renders "" locally and
    // "SSH failed" remotely.
    match crate::ssh::run(barn, cmd, crate::ssh::Opts { batch: true, allow_failure: true, ..Default::default() }) {
        Ok(stdout) => stdout,
        Err(e) => format!("SSH failed: {}", e),
    }
}

// ============================================================================
// Entry point
// ============================================================================

pub async fn run() -> Result<()> {
    let server = YeehawServer::new();
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // === MCP tool harness ==================================================

    /// Drives one async tool body to completion on the *calling* thread.
    ///
    /// A current-thread runtime on purpose: `crate::testing`'s ranch override
    /// is thread-local, so a multi-thread runtime would run the tool on a
    /// worker that never inherited it and `yeehaw_dir()` would panic.
    fn call_tool<F>(future: F) -> CallToolResult
    where
        F: std::future::Future<Output = Result<CallToolResult, McpError>>,
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(future)
            .expect("a tool must return a result, not a protocol error")
    }

    fn is_error(result: &CallToolResult) -> bool {
        result.is_error.unwrap_or(false)
    }

    fn text_of(result: &CallToolResult) -> String {
        serde_json::to_string(&result.content).unwrap_or_default()
    }

    fn trail_yaml(name: &str, extra: &str) -> String {
        format!(
            "name: {name}\n{extra}jobs:\n  build:\n    steps:\n      - name: echo\n        run: echo hi\n"
        )
    }

    fn trail_files() -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(config::trails_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".yaml"))
            .collect();
        names.sort();
        names
    }

    /// `create_trail` checks that the YAML's `name` matches the `name`
    /// parameter; `update_trail` did not. Updating "deploy" with YAML naming
    /// "deploy-v2" therefore wrote a *second* file carrying deploy's uuid while
    /// `deploy.yaml` kept it too — an unguarded rename that leaves one identity
    /// on two files.
    #[test]
    fn update_trail_refuses_yaml_that_renames_the_trail() {
        let _ranch = crate::testing::temp_ranch();

        let mut trail: crate::trails::Trail =
            serde_yaml::from_str(&trail_yaml("deploy", "")).unwrap();
        config::save_trail(&mut trail).unwrap();
        let id = trail.id.clone().unwrap();

        let result = call_tool(YeehawServer::new().update_trail(Parameters(UpdateTrailParams {
            name: "deploy".into(),
            content: trail_yaml("deploy-v2", ""),
        })));

        assert!(
            is_error(&result),
            "a rename disguised as an update must be refused, got: {}",
            text_of(&result)
        );
        assert_eq!(
            trail_files(),
            vec!["deploy.yaml".to_string()],
            "no second file may be created"
        );
        assert_eq!(
            config::load_trail("deploy").unwrap().id.as_ref(),
            Some(&id),
            "the original must keep its identity"
        );
    }

    /// The identity carry-forward has to read the *file*, not the payload.
    /// `trail.id.or(existing.id)` lets caller-supplied YAML win, so pasting a
    /// trail copied from somewhere else silently re-parents this one onto that
    /// trail's uuid — two files, one identity, again.
    #[test]
    fn update_trail_keeps_the_stored_identity_over_one_supplied_by_the_caller() {
        let _ranch = crate::testing::temp_ranch();

        let mut trail: crate::trails::Trail =
            serde_yaml::from_str(&trail_yaml("deploy", "")).unwrap();
        config::save_trail(&mut trail).unwrap();
        let stored_id = trail.id.clone().unwrap();
        let stored_created_at = trail.created_at.clone().unwrap();

        let result = call_tool(YeehawServer::new().update_trail(Parameters(UpdateTrailParams {
            name: "deploy".into(),
            content: trail_yaml(
                "deploy",
                "id: 00000000-0000-4000-8000-000000000000\ncreated_at: '2000-01-01T00:00:00+00:00'\n",
            ),
        })));

        assert!(!is_error(&result), "the update itself must succeed: {}", text_of(&result));

        let loaded = config::load_trail("deploy").unwrap();
        assert_eq!(
            loaded.id.as_ref(),
            Some(&stored_id),
            "the file on disk is authoritative for identity"
        );
        assert_eq!(
            loaded.created_at.as_ref(),
            Some(&stored_created_at),
            "created_at is identity too and must not be overridable"
        );
    }

    // === create must not clobber ==========================================
    //
    // Only the refusal path is driven through the MCP tools here. The happy
    // path for `create_worm` calls `crontab::sync_crontab()`, which rewrites
    // the developer's real crontab — no temp ranch protects that. Successful
    // creation is covered in `config::tests` instead, where it stays on disk.

    /// The likeliest source of the two `barn: local` records in the real
    /// ranch. An agent can pass any string here, and `local` is the obvious one
    /// to reach for — but it is machine-relative with a different spelling than
    /// `None`, which is exactly the pin the ranch cannot sync.
    #[test]
    fn add_livestock_never_stores_the_literal_local() {
        let _ranch = crate::testing::temp_ranch();

        let mut project = types::Project {
            name: "api".into(),
            path: "/tmp/api".into(),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_project(&mut project).unwrap();

        let add = |name: &str, barn: Option<&str>| {
            call_tool(YeehawServer::new().add_livestock(Parameters(AddLivestockParams {
                project: "api".into(),
                name: name.into(),
                path: format!("/tmp/{name}"),
                barn: barn.map(str::to_string),
                repo: None,
                branch: None,
                log_path: None,
                env_path: None,
            })));
        };

        add("web", Some("local"));
        add("worker", Some("pi"));
        add("cron", None);

        let stored = |name: &str| -> Option<String> {
            config::load_projects()
                .into_iter()
                .find(|p| p.name == "api")
                .unwrap()
                .livestock
                .into_iter()
                .find(|l| l.name == name)
                .unwrap()
                .barn
        };

        assert_eq!(stored("web"), None, "`local` is not a barn to pin to");
        assert_eq!(stored("worker"), Some("pi".to_string()));
        assert_eq!(stored("cron"), None);

        crate::migrate::adopt_this_machine("imac").unwrap();
        add("api", Some("local"));

        assert_eq!(
            stored("api"),
            Some("imac".to_string()),
            "an adopted machine writes its real name"
        );
    }

    /// `read_livestock_logs` picked its barn straight off `livestock.barn`, so
    /// after adoption a log file sitting on this very machine is read over ssh
    /// — to a self-barn with no host, which fails outright. The livestock has
    /// not moved; only its spelling changed.
    #[test]
    fn logs_for_a_livestock_here_are_read_locally_after_adoption() {
        let _ranch = crate::testing::temp_ranch();
        let (_dir, log_path) = log_fixture("the local log line\n");

        let mut project = types::Project {
            name: "api".into(),
            path: "/tmp/api".into(),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![types::Livestock {
                name: "web".into(),
                path: "/tmp/web".into(),
                barn: None,
                repo: None,
                branch: None,
                log_path: Some(log_path.clone()),
                env_path: None,
                source: None,
                k8s_metadata: None,
                trails: vec![],
            }],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_project(&mut project).unwrap();

        let read = || {
            text_of(&call_tool(YeehawServer::new().read_livestock_logs(Parameters(
                ReadLogsParams {
                    project: "api".into(),
                    livestock: "web".into(),
                    lines: None,
                    pattern: None,
                },
            ))))
        };

        let before = read();
        assert!(before.contains("the local log line"), "got: {before}");

        crate::migrate::adopt_this_machine("imac").unwrap();

        let after = read();
        assert!(
            after.contains("the local log line"),
            "adoption must not turn a local log read into ssh, got: {after}"
        );
    }

    /// `create_project` builds a project with empty `livestock`, `herds` and
    /// `wiki` and no id. Landing that on an existing file destroys the content
    /// *and* mints a fresh uuid over the old one: the entity is gone and, to
    /// anything syncing on the uuid, a different entity now wears its name.
    #[test]
    fn create_project_refuses_to_overwrite_an_existing_project() {
        let _ranch = crate::testing::temp_ranch();

        let mut existing = types::Project {
            name: "api".into(),
            path: "/tmp/api".into(),
            summary: Some("the real one".into()),
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![types::Livestock {
                name: "web".into(),
                path: "/tmp/web".into(),
                barn: None,
                repo: None,
                branch: None,
                log_path: None,
                env_path: None,
                source: None,
                k8s_metadata: None,
                trails: vec![],
            }],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_project(&mut existing).unwrap();
        let id = existing.id.clone().unwrap();

        let result = call_tool(YeehawServer::new().create_project(Parameters(CreateProjectParams {
            name: "api".into(),
            path: "/tmp/elsewhere".into(),
            summary: None,
            color: None,
        })));

        assert!(
            is_error(&result),
            "creating over an existing project must be refused, got: {}",
            text_of(&result)
        );
        let loaded = config::load_projects();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id.as_ref(), Some(&id), "the uuid must not be re-minted");
        assert_eq!(loaded[0].livestock.len(), 1, "the livestock must survive");
        assert_eq!(loaded[0].summary.as_deref(), Some("the real one"));
    }

    /// The other four creators, each through its own tool. Every one writes its
    /// own file, so the guard on one proves nothing about the rest.
    #[test]
    fn every_mcp_creator_refuses_to_overwrite_an_existing_entity() {
        let _ranch = crate::testing::temp_ranch();
        let server = YeehawServer::new();

        let mut barn = types::Barn {
            name: "pi".into(),
            host: Some("10.0.0.2".into()),
            user: Some("forge".into()),
            port: Some(22),
            identity_file: None,
            critters: vec![],
            ..Default::default()
        };
        config::save_barn(&mut barn).unwrap();
        let barn_id = barn.id.clone().unwrap();
        let result = call_tool(server.create_barn(Parameters(CreateBarnParams {
            name: "pi".into(),
            host: "192.168.0.9".into(),
            user: "root".into(),
            port: None,
            identity_file: "/dev/null".into(),
        })));
        assert!(is_error(&result), "create_barn must refuse: {}", text_of(&result));
        let reloaded = config::load_barns().into_iter().find(|b| b.name == "pi").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&barn_id), "barn uuid must not be re-minted");
        assert_eq!(reloaded.host.as_deref(), Some("10.0.0.2"), "barn must not be rewritten");

        let mut worm = types::Worm {
            name: "nightly".into(),
            command: "echo original".into(),
            schedule: "* * * * *".into(),
            worm_type: "shell".into(),
            enabled: true,
            project: None,
            working_dir: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_worm(&mut worm).unwrap();
        let worm_id = worm.id.clone().unwrap();
        let result = call_tool(server.create_worm(Parameters(CreateWormParams {
            name: "nightly".into(),
            command: "rm -rf /".into(),
            schedule: "0 0 * * *".into(),
            worm_type: "shell".into(),
            project: None,
            working_dir: None,
        })));
        assert!(is_error(&result), "create_worm must refuse: {}", text_of(&result));
        let reloaded = config::load_worms().into_iter().find(|w| w.name == "nightly").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&worm_id), "worm uuid must not be re-minted");
        assert_eq!(reloaded.command, "echo original", "worm must not be rewritten");

        let mut rh = types::RanchHand {
            name: "cluster".into(),
            project: "api".into(),
            rh_type: "kubernetes".into(),
            config: serde_yaml::Value::Null,
            sync_settings: types::RanchHandSyncSettings { auto_sync: false, interval_minutes: None },
            herd: "infra".into(),
            resource_mappings: vec![],
            last_sync: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        config::save_ranchhand(&mut rh).unwrap();
        let rh_id = rh.id.clone().unwrap();
        let result = call_tool(server.create_ranchhand(Parameters(CreateRanchHandParams {
            name: "cluster".into(),
            project: "other".into(),
            rh_type: "terraform".into(),
            config: serde_json::Value::Null,
            herd: "elsewhere".into(),
        })));
        assert!(is_error(&result), "create_ranchhand must refuse: {}", text_of(&result));
        let reloaded = config::load_ranchhands().into_iter().find(|r| r.name == "cluster").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&rh_id), "ranchhand uuid must not be re-minted");
        assert_eq!(reloaded.herd, "infra", "ranchhand must not be rewritten");

        let mut trail: crate::trails::Trail =
            serde_yaml::from_str(&trail_yaml("deploy", "")).unwrap();
        config::save_trail(&mut trail).unwrap();
        let trail_id = trail.id.clone().unwrap();
        let result = call_tool(server.create_trail(Parameters(CreateTrailParams {
            name: "deploy".into(),
            content: trail_yaml("deploy", "env:\n  STAGE: clobbered\n"),
        })));
        assert!(is_error(&result), "create_trail must refuse: {}", text_of(&result));
        let reloaded = config::load_trail("deploy").unwrap();
        assert_eq!(reloaded.id.as_ref(), Some(&trail_id), "trail uuid must not be re-minted");
        assert!(reloaded.env.is_none(), "trail must not be rewritten");
    }

    /// An update that carries no meta fields at all — the ordinary case — must
    /// still inherit the stored identity rather than mint a fresh uuid.
    #[test]
    fn update_trail_carries_identity_forward_when_the_yaml_has_none() {
        let _ranch = crate::testing::temp_ranch();

        let mut trail: crate::trails::Trail =
            serde_yaml::from_str(&trail_yaml("deploy", "")).unwrap();
        config::save_trail(&mut trail).unwrap();
        let stored_id = trail.id.clone().unwrap();

        let result = call_tool(YeehawServer::new().update_trail(Parameters(UpdateTrailParams {
            name: "deploy".into(),
            content: trail_yaml("deploy", "env:\n  STAGE: prod\n"),
        })));

        assert!(!is_error(&result), "expected success: {}", text_of(&result));
        let loaded = config::load_trail("deploy").unwrap();
        assert_eq!(loaded.id.as_ref(), Some(&stored_id));
        assert_eq!(
            loaded.env.as_ref().and_then(|e| e.get("STAGE")).map(String::as_str),
            Some("prod"),
            "the update must still apply"
        );
    }

    // === log command construction ==========================================
    //
    // These commands are handed to `sh -c` locally and to a login shell on the
    // barn over ssh, and every value in them but `lines` is attacker-chosen (an
    // MCP tool parameter) or config-chosen. So the tests run the real command
    // through a real `sh` and assert the payload did not execute, rather than
    // comparing the string to a hand-written expectation that could be wrong in
    // the same way the code is.

    /// Every one of these runs `id` if any interpolation escapes its quoting.
    /// `';id;'` is the payload that was verified to fire before this fix.
    const INJECTIONS: &[&str] = &[
        "';id;'",
        "'; id; '",
        "$(id)",
        "`id`",
        "x' ; id ; #",
        "'|id|'",
        "'\nid\n'",
        "'; id > /dev/stderr; '",
    ];

    /// Run `script` through a real `sh`, returning stdout and stderr together so
    /// a payload cannot hide by writing to the stream we forgot to look at.
    fn sh_capture(script: &str, home: Option<&Path>, path_prefix: Option<&Path>) -> String {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", script]);
        if let Some(h) = home {
            cmd.env("HOME", h);
        }
        if let Some(p) = path_prefix {
            let existing = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{}:{}", p.display(), existing));
        }
        let out = cmd.output().expect("sh should be runnable");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    /// `id` prints `uid=NNN(name)`. Its absence is the proof nothing executed.
    fn assert_did_not_execute(output: &str, script: &str) {
        assert!(
            !output.contains("uid="),
            "injected command ran.\n  script: {script}\n  output: {output}"
        );
    }

    /// A temp dir holding one log file, returned as (dir, absolute log path).
    fn log_fixture(contents: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("app.log");
        std::fs::write(&log, contents).expect("write log");
        let path = log.to_string_lossy().to_string();
        (dir, path)
    }

    #[test]
    fn the_pre_fix_command_shape_really_did_execute_the_payload() {
        // A canary for every assert_did_not_execute above. Those tests only mean
        // something if this harness can actually observe an injection, so this
        // rebuilds the exact pre-fix `format!` — hand-written '{}' around the
        // pattern — and asserts the payload DOES fire. If this ever stops
        // reporting uid=, the other tests have gone vacuous and must be re-armed.
        let (_dir, path) = log_fixture("alpha\n");
        let script = format!("tail -n {} {} | grep -i '{}'", 100, path, "';id;'");
        let out = sh_capture(&script, None, None);
        assert!(
            out.contains("uid="),
            "the injection detector no longer detects the original bug: {out:?}"
        );
    }

    #[test]
    fn a_hostile_grep_pattern_cannot_run_a_command() {
        let (_dir, path) = log_fixture("alpha\nbeta\n");
        for payload in INJECTIONS {
            let script = build_log_command(&path, 100, Some(payload), false);
            assert_did_not_execute(&sh_capture(&script, None, None), &script);
        }
    }

    #[test]
    fn a_hostile_grep_pattern_cannot_run_a_command_in_the_directory_branch() {
        // The `find | xargs tail | grep` shape is a separate format! site and was
        // separately vulnerable.
        let (dir, _path) = log_fixture("alpha\nbeta\n");
        let dir_path = format!("{}/", dir.path().display());
        for payload in INJECTIONS {
            let script = build_log_command(&dir_path, 100, Some(payload), true);
            assert_did_not_execute(&sh_capture(&script, None, None), &script);
        }
    }

    #[test]
    fn a_hostile_log_path_cannot_run_a_command() {
        // log_path / path come from project + barn config, which sync and
        // discovery tools can write. Escape them like any other untrusted value.
        let (dir, _) = log_fixture("alpha\n");
        for suffix in [";id;", "$(id)", "`id`", " ; id ; ", "'; id; '"] {
            let hostile = format!("{}/app.log{}", dir.path().display(), suffix);
            for treat_as_dir in [false, true] {
                let script = build_log_command(&hostile, 100, None, treat_as_dir);
                assert_did_not_execute(&sh_capture(&script, None, None), &script);
                let script = build_log_command(&hostile, 100, Some("alpha"), treat_as_dir);
                assert_did_not_execute(&sh_capture(&script, None, None), &script);
            }
        }
    }

    #[test]
    fn a_hostile_journald_service_name_cannot_run_a_command() {
        // Stand in a fake `journalctl` on PATH that just echoes its arguments, so
        // the real read_critter_logs pipeline can run on a machine without systemd.
        let bin = tempfile::tempdir().expect("tempdir");
        let stub = bin.path().join("journalctl");
        std::fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        for payload in INJECTIONS {
            // Same shape as the use_journald branch of read_critter_logs.
            let script = format!(
                "journalctl -u {} -n {} --no-pager",
                crate::tmux::single_quote(payload),
                100
            );
            let out = sh_capture(&script, None, Some(bin.path()));
            assert_did_not_execute(&out, &script);
            assert!(
                out.contains(*payload) || out.contains(payload.trim()),
                "the unit name should reach journalctl verbatim.\n  script: {script}\n  output: {out}"
            );

            // And with a grep appended, which is the other half of that branch.
            let script = append_grep(script, Some(payload));
            assert_did_not_execute(&sh_capture(&script, None, Some(bin.path())), &script);
        }
    }

    #[test]
    fn a_hostile_pattern_and_path_together_cannot_run_a_command() {
        let (dir, _) = log_fixture("alpha\n");
        let hostile_path = format!("{}/app.log';id;'", dir.path().display());
        let script = build_log_command(&hostile_path, 100, Some("';id;'"), false);
        assert_did_not_execute(&sh_capture(&script, None, None), &script);
    }

    #[test]
    fn an_ordinary_pattern_still_filters() {
        // Escaping is worthless if it broke the feature it protects.
        let (_dir, path) = log_fixture("alpha one\nbeta two\nALPHA three\n");
        let script = build_log_command(&path, 100, Some("alpha"), false);
        let out = sh_capture(&script, None, None);
        assert!(out.contains("alpha one"), "expected the match, got {out:?}");
        assert!(out.contains("ALPHA three"), "-i should still fold case, got {out:?}");
        assert!(!out.contains("beta two"), "non-matching line leaked: {out:?}");
    }

    #[test]
    fn a_pattern_that_looks_like_a_flag_is_searched_for_not_obeyed() {
        // Without the `--`, `grep -i '-v'` inverts the match and returns the very
        // lines the caller asked to exclude.
        let (_dir, path) = log_fixture("alpha\nbeta -v gamma\n");
        let script = build_log_command(&path, 100, Some("-v"), false);
        let out = sh_capture(&script, None, None);
        assert!(out.contains("beta -v gamma"), "expected the literal match, got {out:?}");
        assert!(!out.contains("alpha"), "grep treated the pattern as -v: {out:?}");
    }

    #[test]
    fn a_pattern_starting_with_a_tilde_stays_literal() {
        // This is why the pattern uses single_quote and not shell_escape:
        // shell_escape would turn `~/app` into "$HOME"'/app' and search for the
        // expanded home directory instead of the two characters the caller typed.
        let (_dir, path) = log_fixture("config lives in ~/app/here\nunrelated\n");
        let script = build_log_command(&path, 100, Some("~/app"), false);
        let out = sh_capture(&script, Some(Path::new("/home/nobody")), None);
        assert!(out.contains("~/app/here"), "tilde pattern did not match literally: {out:?}");
    }

    #[test]
    fn a_path_starting_with_a_tilde_still_resolves_to_home() {
        // The mirror image: paths *should* expand, and livestock paths are
        // routinely stored as ~/sites/app.
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(home.path().join("app.log"), "hello-from-home\n").expect("write log");
        let script = build_log_command("~/app.log", 100, None, false);
        assert!(
            script.contains("\"$HOME\""),
            "a leading tilde should become $HOME, got {script:?}"
        );
        let out = sh_capture(&script, Some(home.path()), None);
        assert!(out.contains("hello-from-home"), "tilde path did not resolve: {out:?}");
    }

    #[test]
    fn read_local_logs_itself_rejects_the_verified_payload() {
        // End to end through the real helper, with the exact payload that was
        // observed executing before this fix.
        let (_dir, path) = log_fixture("alpha\n");
        let out = read_local_logs(&path, 100, Some("';id;'"));
        assert!(!out.contains("uid="), "injected command ran: {out:?}");
    }

    #[test]
    fn every_interpolated_value_becomes_exactly_one_shell_word() {
        // A value that splits into two words is an argument-injection bug even
        // when it is not a command-injection bug.
        for value in ["a b", "';id;'", "$(id)", "", "*", "~/x y"] {
            let script = format!(
                "set -- {} {}; printf %s $#",
                crate::tmux::single_quote(value),
                crate::tmux::shell_escape(value)
            );
            assert_eq!(sh_capture(&script, Some(Path::new("/home/nobody")), None), "2",
                "{value:?} did not stay one word per escaper");
        }
    }
}
