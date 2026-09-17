use serde::{Deserialize, Serialize};

// ============================================================================
// Stable identity
// ============================================================================

/// Entities that carry a stable identity for synchronization.
///
/// The filename remains the human-facing key; the UUID is the sync key. Without
/// it, a rename and a delete-plus-create are indistinguishable on disk.
pub trait Identified {
    fn id(&self) -> Option<&str>;
    fn set_id(&mut self, id: String);
    fn created_at(&self) -> Option<&str>;
    fn set_created_at(&mut self, ts: String);
    fn updated_at(&self) -> Option<&str>;
    fn set_updated_at(&mut self, ts: String);

    /// Assigns an id and creation time if absent, and advances `updated_at`.
    /// Call immediately before persisting.
    ///
    /// The three fields are not independent, and a half-populated record is
    /// not hypothetical once a merge starts taking records from peers. The
    /// rules:
    ///
    /// - **No id.** Mint one. There is nothing else to do, and it is done
    ///   whatever the timestamps say — an entity with no uuid cannot be
    ///   matched across machines at all.
    /// - **No `created_at`.** Backfill it from `updated_at` when there is one,
    ///   and only fall back to `now` when there is not. A record that already
    ///   carries an `updated_at` demonstrably existed before this instant, and
    ///   dating it to `now` would make an old entity look newly created.
    /// - **`updated_at`.** `max(now, stored)`. A clock stepped backwards by
    ///   NTP would otherwise make a later write carry an earlier timestamp,
    ///   inverting last-write-wins.
    fn stamp(&mut self) {
        let now = chrono::Utc::now();

        if self.id().is_none() {
            self.set_id(uuid::Uuid::new_v4().to_string());
        }

        // Read before `set_updated_at` below overwrites the value it reads.
        if self.created_at().is_none() {
            let created = self
                .updated_at()
                .map(|ts| ts.to_string())
                .unwrap_or_else(|| now.to_rfc3339());
            self.set_created_at(created);
        }

        // Parsed, never compared as strings. RFC 3339 admits a `Z` suffix —
        // which is what chrono's own serde impl emits — and any UTC offset,
        // and neither sorts chronologically: `'Z'` is above every digit, and
        // a western offset puts a *later* instant at a *lower* wall-clock
        // hour. A record that fails to parse is treated as absent.
        let stored = self
            .updated_at()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let next = match stored {
            Some(previous) if previous > now => previous,
            _ => now,
        };
        self.set_updated_at(next.to_rfc3339());
    }
}

/// Implements [`Identified`] for a struct carrying the three meta fields.
///
/// `#[macro_export]` and `$crate`-qualified on purpose: `Trail` lives in
/// `crate::trails`, so the macro has to be usable from outside this module
/// without dragging the trait into scope there.
#[macro_export]
macro_rules! impl_identified {
    ($t:ty) => {
        impl $crate::types::Identified for $t {
            fn id(&self) -> Option<&str> { self.id.as_deref() }
            fn set_id(&mut self, id: String) { self.id = Some(id); }
            fn created_at(&self) -> Option<&str> { self.created_at.as_deref() }
            fn set_created_at(&mut self, ts: String) { self.created_at = Some(ts); }
            fn updated_at(&self) -> Option<&str> { self.updated_at.as_deref() }
            fn set_updated_at(&mut self, ts: String) { self.updated_at = Some(ts); }
        }
    };
}

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
    /// The name of the barn this machine is. `None` means unmigrated: this
    /// machine is still the synthetic `local` barn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub this_barn: Option<String>,
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
            this_barn: None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl_identified!(Project);

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced: Option<bool>,
    /// Whether *this* machine wants the barn's sessions streamed into the
    /// session grid.
    ///
    /// Machine-local, and `canonical::SHAPES` classifies it so. Wanting to see
    /// a barn's sessions is a preference of the laptop in front of the user, not
    /// a property of the barn — the same shape as `synced`. Synced, the Ranch
    /// House's answer would arrive and switch a laptop's grid on or off behind
    /// the user's back.
    ///
    /// `None` and `Some(false)` mean the same thing to the grid and are kept
    /// apart for the same reason `synced` keeps them apart: never asked is not
    /// the same answer as declined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunneled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_ranch_house: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    /// DEVIATION from the Slice D plan's field list, which specifies
    /// `#[serde(default)]` alone. `skip_serializing_if` is needed here for the
    /// same reason the other five have it, and the plan's own stated reason: an
    /// empty `Vec` is not `null`, so `render_mapping`'s drop-the-nulls rule does
    /// not cover it, and a bare `#[serde(default)]` puts `addresses: []` into
    /// every one of the existing barn files on first write *and* gives every barn
    /// a different content hash from the one an older peer computes. Measured in
    /// `canonical::tests::an_empty_address_list_hashes_the_same_as_a_peer_that_has_no_such_field`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl_identified!(Barn);

/// An unconfigured barn: no name, no host, nothing enrolled.
///
/// Exists so that adding a field to `Barn` is not a twenty-two-site edit —
/// that is how many struct literals the six ranch fields broke. Listed
/// exhaustively rather than `#[derive(Default)]` on purpose: the explicit
/// literal is the tripwire, the same device `wire::WireTombstone`'s
/// `From` impls use. A new field fails to compile *here*, which is where the
/// question "what does an unconfigured barn say about this?" belongs.
///
/// ## `name` defaults to empty, and that is a hazard
///
/// `Barn { host: .., ..Default::default() }` compiles and has no name, and
/// `config::save_barn` would write it to `~/.yeehaw/barns/.yaml` — a dotfile
/// that then loads back as a barn called `""`. The defense is at the write path
/// (`config::validate_name` refuses a blank name) rather than here, for two
/// reasons: `Barn { name: String::new(), .. }` was already legal before this
/// impl existed, so a constructor could never have been the guard; and the write
/// path is where every other malformed name — `/`, `..`, `\0` — is already
/// caught, so there is one place to look.
///
/// Two callers must **not** reach for this: `views::barn_context::build_updated`
/// and `ranch::merge`'s `merge_content`. Both rebuild an *existing* barn, and
/// defaulting a field there is not a default, it is a deletion — editing a
/// host in the TUI would silently drop the barn's brand and its ranch-house
/// flag.
impl Default for Barn {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: None,
            user: None,
            // `None`, not `Some(22)`: the ssh paths already treat an absent port
            // as the default, and baking 22 in here would write the assumption
            // into every barn file where it could no longer be told from a
            // deliberate choice.
            port: None,
            identity_file: None,
            critters: Vec::new(),
            source: None,
            connection_type: None,
            connection_config: None,
            connectable: None,
            synced: None,
            tunneled: None,
            brand: None,
            is_ranch_house: None,
            tunnel_port: None,
            last_seen: None,
            addresses: Vec::new(),
            id: None,
            created_at: None,
            updated_at: None,
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl_identified!(RanchHand);

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl_identified!(Worm);

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
    SessionGrid,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_project() -> Project {
        Project {
            name: "api".into(),
            path: "/tmp".into(),
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
        }
    }

    /// Existing YAML on disk has none of the new fields. It must still load.
    #[test]
    fn project_loads_without_meta_fields() {
        let yaml = r#"
name: api
path: /Users/cam/Sites/api
summary: The API
"#;
        let p: Project = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.name, "api");
        assert!(p.id.is_none());
        assert!(p.created_at.is_none());
        assert!(p.updated_at.is_none());
    }

    /// The six ranch fields Slice D adds to `Barn`, spelled as they appear in a
    /// file. Listed once so the two tests below cannot drift apart.
    const RANCH_FIELDS: [&str; 6] =
        ["synced", "brand", "is_ranch_house", "tunnel_port", "last_seen", "addresses"];

    /// A barn file as the real ranch holds one today: a name, a host, and the
    /// three identity fields Phase 1 stamped on.
    const EXISTING_BARN_YAML: &str = "\
name: imac
host: 192.168.1.50
id: 3f0c1e8a-5f2b-4a55-9a3d-6d1c2b7e4f01
created_at: 2026-08-01T10:00:00+00:00
updated_at: 2026-09-01T10:00:00+00:00
";

    /// Every barn file on the ranch predates the ranch fields. Loading one must
    /// not require them, and must not invent values for them either: `synced`
    /// defaulting to `Some(false)` would mark the whole ranch as deliberately
    /// excluded from sync rather than as not-yet-enrolled.
    #[test]
    fn an_existing_barn_file_loads_with_no_ranch_fields_set() {
        let barn: Barn = serde_yaml::from_str(EXISTING_BARN_YAML).unwrap();

        assert_eq!(barn.name, "imac");
        assert_eq!(barn.host.as_deref(), Some("192.168.1.50"));
        assert_eq!(barn.synced, None, "absent is not `false`; nothing has enrolled this barn");
        assert_eq!(barn.brand, None);
        assert_eq!(barn.is_ranch_house, None, "absent is not `not the ranch house`");
        assert_eq!(barn.tunnel_port, None);
        assert_eq!(barn.last_seen, None);
        assert!(barn.addresses.is_empty(), "got {:?}", barn.addresses);
    }

    /// The one that protects the user's files. Every path that touches a barn
    /// re-serializes the whole struct — `save_barn` at `config.rs:555` does, and
    /// `stamp()` means even a no-op edit writes — so a field without
    /// `skip_serializing_if` is six new lines on all eight of the real barn
    /// files the first time anything is saved.
    ///
    /// It is not only noise. `canonical::render_mapping` drops null mapping
    /// entries precisely so that a peer on an older build, which renders no key
    /// at all, hashes the same as this build rendering `null` — so an emitted
    /// key here is the difference between "nothing changed" and "every barn
    /// changed" on a version-skewed sync.
    #[test]
    fn a_barn_with_no_ranch_fields_set_emits_none_of_them() {
        let barn: Barn = serde_yaml::from_str(EXISTING_BARN_YAML).unwrap();
        let written = serde_yaml::to_string(&barn).unwrap();

        for field in RANCH_FIELDS {
            assert!(
                !written.contains(&format!("{}:", field)),
                "re-serializing an existing barn file added a `{}` key it never had:\n{}",
                field,
                written
            );
        }
    }

    /// An entity with no id must serialize without emitting empty keys,
    /// so untouched files stay byte-identical in shape.
    #[test]
    fn project_without_meta_omits_meta_keys() {
        let p = bare_project();
        let out = serde_yaml::to_string(&p).unwrap();
        assert!(!out.contains("id:"), "unexpected id key:\n{}", out);
        assert!(!out.contains("created_at:"), "unexpected created_at key:\n{}", out);
        assert!(!out.contains("updated_at:"), "unexpected updated_at key:\n{}", out);
    }

    #[test]
    fn stamp_assigns_id_and_timestamps_once() {
        let mut p = bare_project();
        p.stamp();
        let id = p.id.clone().unwrap();
        let created = p.created_at.clone().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        p.stamp();

        assert_eq!(p.id.as_ref().unwrap(), &id, "id must be stable across stamps");
        assert_eq!(p.created_at.as_ref().unwrap(), &created, "created_at must not move");
        assert!(p.updated_at.as_ref().unwrap() > &created, "updated_at must advance");
    }

    /// A half-populated record — an id but no `created_at` — is not something
    /// any current code path produces, but a merge taking records from a peer
    /// will. Stamping `now` onto its `created_at` dates an entity of unknown
    /// age to this instant. `updated_at` is the only evidence of its real age
    /// the record carries, so that is what it is backfilled from.
    #[test]
    fn stamp_backfills_a_missing_created_at_from_updated_at_not_from_now() {
        let mut p = bare_project();
        p.id = Some("carried-over-id".into());
        let a_month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        p.updated_at = Some(a_month_ago.clone());

        p.stamp();

        assert_eq!(p.id.as_deref(), Some("carried-over-id"), "the id must be kept");
        assert_eq!(
            p.created_at.as_deref(),
            Some(a_month_ago.as_str()),
            "created_at must come from updated_at, not from now"
        );
    }

    /// The mirror shape: no id, but a `created_at` that is real. An id has to
    /// be minted — there is nothing else to do — but the timestamp is the
    /// entity's actual age and must survive untouched.
    #[test]
    fn stamp_mints_an_id_without_disturbing_an_existing_created_at() {
        let mut p = bare_project();
        let a_month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        p.created_at = Some(a_month_ago.clone());

        p.stamp();

        assert!(p.id.is_some(), "an entity with no id must get one");
        assert_eq!(
            p.created_at.as_deref(),
            Some(a_month_ago.as_str()),
            "created_at must not be rewritten"
        );
    }

    /// An NTP correction can step a clock backwards. If `stamp` writes `now`
    /// unconditionally, the write that happens *after* the step carries the
    /// *earlier* timestamp — and last-write-wins resolves to the older record.
    ///
    /// A stored timestamp that is ahead of `now` is exactly what a backwards
    /// step looks like from inside the process, so that is how it is staged.
    #[test]
    fn updated_at_does_not_move_backwards_when_the_clock_does() {
        let mut p = bare_project();
        p.stamp();

        let ahead = chrono::Utc::now() + chrono::Duration::hours(1);
        p.updated_at = Some(ahead.to_rfc3339());

        p.stamp();

        let after = chrono::DateTime::parse_from_rfc3339(p.updated_at.as_ref().unwrap())
            .expect("stamp must leave a parseable rfc3339 timestamp");
        assert!(
            after >= ahead,
            "updated_at went backwards: {} -> {}",
            ahead.to_rfc3339(),
            p.updated_at.as_ref().unwrap()
        );
    }

    /// The monotonicity guard has to parse, not compare strings. RFC 3339
    /// admits a `Z` suffix (which is what chrono's own serde impl emits) and
    /// any UTC offset, and neither sorts chronologically: the instant below is
    /// an hour in the *future* but its wall-clock hour is five below the
    /// current UTC hour, so a lexical `max` picks `now` and loses an hour.
    #[test]
    fn updated_at_monotonicity_compares_instants_not_strings() {
        let mut p = bare_project();

        let ahead = chrono::Utc::now() + chrono::Duration::hours(1);
        let written_in_a_western_offset = ahead
            .with_timezone(&chrono::FixedOffset::west_opt(5 * 3600).unwrap())
            .to_rfc3339();
        p.id = Some("id".into());
        p.created_at = Some(ahead.to_rfc3339());
        p.updated_at = Some(written_in_a_western_offset.clone());

        p.stamp();

        let after = chrono::DateTime::parse_from_rfc3339(p.updated_at.as_ref().unwrap())
            .expect("stamp must leave a parseable rfc3339 timestamp");
        assert!(
            after >= ahead,
            "a lexical comparison lost an hour: {} -> {}",
            written_in_a_western_offset,
            p.updated_at.as_ref().unwrap()
        );
    }

    /// The shape actually on this machine's disk today: `gradientSpread: null`,
    /// livestock with every optional key spelled out, a wiki section, and no
    /// meta fields anywhere. A synthetic all-`None` struct would not catch a
    /// `#[serde(default)]` that was forgotten on a field the real files carry.
    #[test]
    fn a_real_shaped_project_round_trips_without_gaining_meta_keys() {
        let yaml = r#"name: Agent Desk
path: /Users/cam/Sites/MPP/desk/
summary: Autonomous financial newsroom.
color: '#000000'
gradientSpread: null
gradientInverted: null
livestock:
- name: production
  path: /home/forge/desk.trellis.market/
  barn: guided
  repo: null
  branch: main
  log_path: null
  env_path: null
  source: null
  k8s_metadata: null
  trails:
  - agentdesk-deploy-production
herds: []
wiki:
- title: Architecture
  content: |
    ## What this is
issueProvider: null
wikiProvider: null
"#;
        let p: Project = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.name, "Agent Desk");
        assert_eq!(p.livestock.len(), 1);
        assert_eq!(p.wiki.len(), 1);
        assert!(p.id.is_none() && p.created_at.is_none() && p.updated_at.is_none());

        let out = serde_yaml::to_string(&p).unwrap();
        assert!(!out.contains("id:"), "an untouched project must not gain an id:\n{}", out);
        assert!(!out.contains("created_at:"), "unexpected created_at key:\n{}", out);
        assert!(!out.contains("updated_at:"), "unexpected updated_at key:\n{}", out);

        // And it must still load after the round trip.
        let again: Project = serde_yaml::from_str(&out).unwrap();
        assert_eq!(again.livestock[0].trails, vec!["agentdesk-deploy-production"]);
    }

    /// The same guarantee for the other three top-level types in this file.
    /// Each fixture is the shape the real ranch stores today.
    #[test]
    fn barn_worm_and_ranchhand_load_and_serialize_without_meta_fields() {
        let barn: Barn = serde_yaml::from_str(
            r#"name: ascend
host: 172.233.129.224
user: forge
port: 22
identity_file: ~/.ssh/id_big_ups
critters:
  - name: mysql
    service: mysql.service
    use_journald: true
"#,
        )
        .unwrap();
        assert_eq!(barn.critters.len(), 1);
        assert!(barn.id.is_none());
        let out = serde_yaml::to_string(&barn).unwrap();
        assert!(!out.contains("id:"), "barn gained meta keys:\n{}", out);
        assert!(!out.contains("created_at:"), "barn gained meta keys:\n{}", out);

        let worm: Worm = serde_yaml::from_str(
            r#"name: poll--web--deploy
command: yeehaw trail poll web deploy
schedule: '* * * * *'
type: shell
enabled: true
project: Yeehaw CLI
working_dir: null
"#,
        )
        .unwrap();
        assert!(worm.id.is_none());
        let out = serde_yaml::to_string(&worm).unwrap();
        assert!(!out.contains("id:"), "worm gained meta keys:\n{}", out);
        assert!(!out.contains("created_at:"), "worm gained meta keys:\n{}", out);

        let rh: RanchHand = serde_yaml::from_str(
            r#"name: cluster
project: Kill Switch
type: k8s
config:
  context: minikube
sync_settings:
  auto_sync: false
  interval_minutes: null
herd: infra
last_sync: null
"#,
        )
        .unwrap();
        assert!(rh.id.is_none());
        let out = serde_yaml::to_string(&rh).unwrap();
        assert!(!out.contains("id:"), "ranchhand gained meta keys:\n{}", out);
        assert!(!out.contains("created_at:"), "ranchhand gained meta keys:\n{}", out);
    }

    /// Every stamped type must behave the same way, not just `Project` —
    /// `impl_identified!` is the thing under test, and a hand-edited copy of it
    /// that read the wrong field would only show up here.
    #[test]
    fn stamp_is_idempotent_for_identity_on_every_stamped_type() {
        fn check<T: Identified>(mut e: T, what: &str) {
            e.stamp();
            let id = e.id().unwrap().to_string();
            let created = e.created_at().unwrap().to_string();

            std::thread::sleep(std::time::Duration::from_millis(5));
            e.stamp();

            assert_eq!(e.id().unwrap(), id, "{what}: id must be stable");
            assert_eq!(e.created_at().unwrap(), created, "{what}: created_at must not move");
        }

        check(bare_project(), "Project");
        check(
            Barn {
                name: "pi".into(),
                host: None,
                user: None,
                port: None,
                identity_file: None,
                critters: vec![],
                ..Default::default()
            },
            "Barn",
        );
        check(
            Worm {
                name: "nightly".into(),
                command: "echo".into(),
                schedule: "* * * * *".into(),
                worm_type: "shell".into(),
                enabled: true,
                project: None,
                working_dir: None,
                id: None,
                created_at: None,
                updated_at: None,
            },
            "Worm",
        );
        check(
            RanchHand {
                name: "cluster".into(),
                project: "p".into(),
                rh_type: "k8s".into(),
                config: serde_yaml::Value::Null,
                sync_settings: RanchHandSyncSettings { auto_sync: false, interval_minutes: None },
                herd: "h".into(),
                resource_mappings: vec![],
                last_sync: None,
                id: None,
                created_at: None,
                updated_at: None,
            },
            "RanchHand",
        );
        check(
            crate::trails::Trail {
                name: "deploy".into(),
                on: None,
                env: None,
                jobs: Default::default(),
                id: None,
                created_at: None,
                updated_at: None,
            },
            "Trail",
        );
    }
}
