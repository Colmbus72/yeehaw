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
        let project = types::Project {
            name: p.name, path: p.path, summary: p.summary, color: p.color,
            gradient_spread: None, gradient_inverted: None,
            livestock: vec![], herds: vec![], wiki: vec![],
            issue_provider: None, wiki_provider: None,
        };
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        let livestock = types::Livestock {
            name: p.name, path: p.path, barn: p.barn, repo: p.repo,
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
        match config::save_project(&project) {
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
        let barn = livestock.barn.as_ref().and_then(|bn| find_barn(bn));

        let output = if let Some(barn) = barn.filter(|b| !config::is_local_barn(b)) {
            if let (Some(host), Some(user), Some(port), Some(key)) = (&barn.host, &barn.user, barn.port, &barn.identity_file) {
                let cmd = if full_path.ends_with('/') {
                    match &p.pattern {
                        Some(pat) => format!(
                            "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n {} 2>/dev/null | grep -i '{}'",
                            full_path, lines, pat
                        ),
                        None => format!(
                            "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n {} 2>/dev/null",
                            full_path, lines
                        ),
                    }
                } else {
                    match &p.pattern {
                        Some(pat) => format!("tail -n {} {} | grep -i '{}'", lines, full_path, pat),
                        None => format!("tail -n {} {}", lines, full_path),
                    }
                };
                read_remote_output(host, user, port, key, &cmd)
            } else {
                return err_text("Barn SSH config incomplete");
            }
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
        let barn = types::Barn {
            name: p.name, host: Some(p.host), user: Some(p.user),
            port: Some(p.port.unwrap_or(22)), identity_file: Some(p.identity_file),
            critters: vec![], source: None, connection_type: None,
            connection_config: None, connectable: None,
        };
        match config::save_barn(&barn) {
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
        match config::save_barn(&barn) {
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        match config::save_barn(&barn) {
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
        match config::save_barn(&barn) {
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
            let base = format!("journalctl -u {} -n {} --no-pager", critter.service, lines);
            match &p.pattern {
                Some(pat) => format!("{} | grep -i '{}'", base, pat),
                None => base,
            }
        } else if let Some(log_path) = &critter.log_path {
            match &p.pattern {
                Some(pat) => format!("tail -n {} {} | grep -i '{}'", lines, log_path, pat),
                None => format!("tail -n {} {}", lines, log_path),
            }
        } else {
            return err_text("No log source configured");
        };

        if config::is_local_barn(&barn) {
            let output = std::process::Command::new("sh").args(["-c", &cmd]).output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|e| format!("Failed: {}", e));
            ok_text(&output)
        } else if let (Some(host), Some(user), Some(port), Some(key)) = (&barn.host, &barn.user, barn.port, &barn.identity_file) {
            ok_text(&read_remote_output(host, user, port, key, &cmd))
        } else {
            err_text("Barn SSH config incomplete")
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        match config::save_project(&project) {
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
        let worm = types::Worm {
            name: p.name, command: p.command, schedule: p.schedule,
            worm_type: p.worm_type, enabled: true, project: p.project, working_dir: p.working_dir,
        };
        match config::save_worm(&worm) {
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
        match config::save_worm(&worm) {
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
        match config::save_worm(&worm) {
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

        let rh = types::RanchHand {
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
        };
        match config::save_ranchhand(&rh) {
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
        match config::save_ranchhand(&rh) {
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
            let result = match ranchhand_k8s::sync_k8s_resources(&rh) {
                Ok(r) => r,
                Err(e) => return err_text(&format!("K8s sync failed: {}", e)),
            };

            // Save barns (create if not exists)
            for barn in &result.barns {
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

            let _ = config::save_project(&project);
            let _ = config::update_ranchhand_last_sync(&rh_name);

            sync_summary = format!("Synced from K8s: {} barns, {} livestock, {} critters, {} herds",
                result.barns.len(), result.livestock.len(), result.critters.len(), result.herds.len());
        } else if rh.rh_type == "terraform" {
            let result = match ranchhand_terraform::sync_terraform_resources(&rh) {
                Ok(r) => r,
                Err(e) => return err_text(&format!("Terraform sync failed: {}", e)),
            };

            // Save barns (create if not exists)
            for barn in &result.barns {
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
                });
                for critter in &result.critters {
                    if !tf_barn.critters.iter().any(|c| c.name == critter.name) {
                        tf_barn.critters.push(critter.clone());
                    }
                }
                let _ = config::save_barn(&tf_barn);
            }

            let _ = config::save_project(&project);
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
        let trail: crate::trails::Trail = match serde_yaml::from_str(&p.content) {
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
        match config::save_trail(&trail) {
            Ok(_) => ok_text(&format!("Trail '{}' created", p.name)),
            Err(e) => err_text(&format!("Failed to save trail: {}", e)),
        }
    }

    #[tool(description = "Update an existing trail with new YAML content")]
    async fn update_trail(&self, params: Parameters<UpdateTrailParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if config::load_trail(&p.name).is_none() {
            return err_text(&format!("Trail '{}' not found", p.name));
        }
        let trail: crate::trails::Trail = match serde_yaml::from_str(&p.content) {
            Ok(t) => t,
            Err(e) => return err_text(&format!("Invalid trail YAML: {}", e)),
        };
        match config::save_trail(&trail) {
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

fn read_local_logs(path: &str, lines: u32, pattern: Option<&str>) -> String {
    let cmd = if path.ends_with('/') || std::path::Path::new(path).is_dir() {
        match pattern {
            Some(pat) => format!(
                "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n {} 2>/dev/null | grep -i '{}'",
                path, lines, pat
            ),
            None => format!(
                "find {} -name '*.log' -type f 2>/dev/null | xargs tail -n {} 2>/dev/null",
                path, lines
            ),
        }
    } else {
        match pattern {
            Some(pat) => format!("tail -n {} {} | grep -i '{}'", lines, path, pat),
            None => format!("tail -n {} {}", lines, path),
        }
    };
    std::process::Command::new("sh").args(["-c", &cmd]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_else(|e| format!("Failed: {}", e))
}

fn read_remote_output(host: &str, user: &str, port: u16, identity_file: &str, cmd: &str) -> String {
    std::process::Command::new("ssh")
        .args(["-p", &port.to_string(), "-i", identity_file,
               "-o", "StrictHostKeyChecking=no", "-o", "ConnectTimeout=10",
               &format!("{}@{}", user, host), cmd])
        .output()
        .map(|o| {
            if o.status.success() { String::from_utf8_lossy(&o.stdout).to_string() }
            else { format!("SSH failed: {}", String::from_utf8_lossy(&o.stderr)) }
        })
        .unwrap_or_else(|e| format!("SSH failed: {}", e))
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
