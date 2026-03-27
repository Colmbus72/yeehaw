use serde::{Deserialize, Serialize};

// ============================================================================
// Core Config Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    pub default_project: Option<String>,
    #[serde(default = "default_editor")]
    pub editor: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub show_activity: bool,
    #[serde(default)]
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub tmux: TmuxConfig,
    #[serde(default)]
    pub slack: Option<SlackConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            default_project: None,
            editor: "vim".to_string(),
            theme: "dark".to_string(),
            show_activity: true,
            claude: ClaudeConfig::default(),
            tmux: TmuxConfig::default(),
            slack: None,
        }
    }
}

fn default_version() -> u32 { 1 }
fn default_editor() -> String { "vim".to_string() }
fn default_theme() -> String { "dark".to_string() }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    #[serde(default = "default_claude_model")]
    pub model: String,
    #[serde(default = "default_true")]
    pub auto_attach: bool,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".to_string(),
            auto_attach: true,
        }
    }
}

fn default_claude_model() -> String { "claude-sonnet-4-20250514".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmuxConfig {
    #[serde(default = "default_session_prefix")]
    pub session_prefix: String,
    #[serde(default = "default_shell")]
    pub default_shell: String,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            session_prefix: "yh-".to_string(),
            default_shell: "/bin/zsh".to_string(),
        }
    }
}

fn default_session_prefix() -> String { "yh-".to_string() }
fn default_shell() -> String { "/bin/zsh".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    pub enabled: bool,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    pub default_project: Option<String>,
    pub channel_projects: Option<std::collections::HashMap<String, String>>,
    pub system_prompt: Option<String>,
}

// ============================================================================
// Project Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    pub summary: Option<String>,
    pub color: Option<String>,
    #[serde(rename = "gradientSpread")]
    pub gradient_spread: Option<f64>,
    #[serde(rename = "gradientInverted")]
    pub gradient_inverted: Option<bool>,
    #[serde(default)]
    pub livestock: Vec<Livestock>,
    #[serde(default)]
    pub herds: Vec<Herd>,
    #[serde(default)]
    pub wiki: Vec<WikiSection>,
    #[serde(rename = "issueProvider")]
    pub issue_provider: Option<IssueProviderConfig>,
    #[serde(rename = "wikiProvider")]
    pub wiki_provider: Option<WikiProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiSection {
    pub title: String,
    pub content: String,
}

// ============================================================================
// Livestock Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Livestock {
    pub name: String,
    pub path: String,
    pub barn: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub log_path: Option<String>,
    pub env_path: Option<String>,
    pub source: Option<String>,
    pub k8s_metadata: Option<K8sLivestockMetadata>,
    #[serde(default)]
    pub trails: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sLivestockMetadata {
    pub namespace: String,
    pub pod_name: String,
    pub deployment: Option<String>,
    pub image: String,
    pub image_tag: Option<String>,
}

// ============================================================================
// Barn Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Barn {
    pub name: String,
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    #[serde(default)]
    pub critters: Vec<Critter>,
    pub source: Option<String>,
    pub connection_type: Option<String>,
    pub connection_config: Option<K8sBarnConnectionConfig>,
    pub connectable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sBarnConnectionConfig {
    pub context: String,
    pub node: String,
}

// ============================================================================
// Critter Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Critter {
    pub name: String,
    pub service: String,
    pub service_path: Option<String>,
    pub config_path: Option<String>,
    pub log_path: Option<String>,
    pub use_journald: Option<bool>,
    pub source: Option<String>,
    pub endpoint: Option<String>,
    pub port: Option<u16>,
    pub k8s_metadata: Option<K8sCritterMetadata>,
    pub tf_metadata: Option<TerraformCritterMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sCritterMetadata {
    pub namespace: String,
    pub pod_name: String,
    pub image: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerraformCritterMetadata {
    pub resource_type: String,
    pub resource_name: String,
}

// ============================================================================
// Herd Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Herd {
    pub name: String,
    #[serde(default)]
    pub livestock: Vec<String>,
    #[serde(default)]
    pub critters: Vec<HerdCritterRef>,
    #[serde(default)]
    pub connections: Vec<HerdConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HerdCritterRef {
    pub barn: String,
    pub critter: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HerdConnection {
    pub livestock: String,
    pub critter: String,
    pub barn: String,
}

// ============================================================================
// Ranch Hand Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RanchHand {
    pub name: String,
    pub project: String,
    #[serde(rename = "type")]
    pub rh_type: String,
    pub config: serde_yaml::Value,
    pub sync_settings: RanchHandSyncSettings,
    pub herd: String,
    #[serde(default)]
    pub resource_mappings: Vec<ResourceMapping>,
    pub last_sync: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RanchHandSyncSettings {
    pub auto_sync: bool,
    pub interval_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMapping {
    pub resource_id: String,
    pub herd_name: String,
}

// ============================================================================
// Worm Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worm {
    pub name: String,
    pub command: String,
    pub schedule: String,
    #[serde(rename = "type")]
    pub worm_type: String,
    pub enabled: bool,
    pub project: Option<String>,
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WormRun {
    pub worm: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub log_file: String,
    pub trigger: String,
    pub status: Option<String>,
    pub skip_reason: Option<String>,
}

// ============================================================================
// Issue / Wiki Provider Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IssueProviderConfig {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "linear")]
    Linear {
        #[serde(rename = "teamId")]
        team_id: Option<String>,
        #[serde(rename = "teamName")]
        team_name: Option<String>,
    },
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WikiProviderConfig {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "linear")]
    Linear {
        #[serde(rename = "teamId")]
        team_id: Option<String>,
        #[serde(rename = "teamName")]
        team_name: Option<String>,
    },
}

// ============================================================================
// Session / View Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(rename = "type")]
    pub session_type: String,
    pub project: Option<String>,
    pub livestock: Option<String>,
    pub barn: Option<String>,
    pub tmux_session: String,
    pub tmux_window: Option<u32>,
    pub started_at: String,
    pub working_directory: String,
    pub notes: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppView {
    Global,
    Project { project: Project },
    Barn { barn: Barn },
    Wiki { project: Project },
    Issues { project: Project },
    Livestock {
        project: Project,
        livestock: Livestock,
        source: String,
        source_barn: Option<Barn>,
    },
    Logs {
        project: Project,
        livestock: Livestock,
        source: String,
        source_barn: Option<Barn>,
    },
    Critter { barn: Barn, critter: Critter },
    CritterLogs { barn: Barn, critter: Critter },
    Herd { project: Project, herd: Herd },
    RanchHand { project: Project, ranchhand: RanchHand },
    Worm { worm: Worm },
    WormRunLog { worm: Worm, run: WormRun },
    Trail {
        project: Project,
        livestock: Livestock,
        trail: crate::trails::Trail,
        source: String,
        source_barn: Option<Barn>,
    },
    NightSky,
    Vault { source_pane: Option<String> },
}

// ============================================================================
// Vault Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub entries: Vec<VaultEntry>,
}

impl Vault {
    pub fn new() -> Self {
        Self { entries: vec![] }
    }
}

// Implement PartialEq manually for types that need it
impl PartialEq for Project {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for Barn {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for Livestock {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for Critter {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for Herd {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for RanchHand {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for Worm {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}
impl PartialEq for WormRun {
    fn eq(&self, other: &Self) -> bool {
        self.worm == other.worm && self.started_at == other.started_at
    }
}
