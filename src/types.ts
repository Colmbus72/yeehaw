// Core domain types

// ============================================================================
// Ranch Hand Types (IaC Integration)
// ============================================================================

// Source tracking for all entities - "manual" or "ranchhand:<name>"
export type EntitySource = 'manual' | `ranchhand:${string}`;

// Connection types for barns
export type BarnConnectionType = 'ssh' | 'kubernetes' | 'terraform';

// Ranch Hand provider types
export type RanchHandType = 'kubernetes' | 'terraform';

// Kubernetes-specific configuration
export interface KubernetesConfig {
  kubeconfig_path?: string;  // default: ~/.kube/config
  context: string;           // e.g., "staging-context"
  private_registries: string[];  // pods from these = livestock
}

// Terraform-specific configuration
export interface TerraformConfig {
  backend: 's3' | 'local';
  bucket?: string;           // S3 bucket name
  key?: string;              // State file path in bucket
  region?: string;           // AWS region
  local_path?: string;       // For local backend
}

// Sync settings for Ranch Hand
export interface RanchHandSyncSettings {
  auto_sync: boolean;
  interval_minutes?: number;
}

// Resource to herd mapping (remembered assignments)
export interface ResourceMapping {
  resource_id: string;
  herd_name: string;
}

// Ranch Hand = IaC provider that syncs infrastructure into Yeehaw
export interface RanchHand {
  name: string;
  project: string;           // project name this belongs to
  type: RanchHandType;
  config: KubernetesConfig | TerraformConfig;
  sync_settings: RanchHandSyncSettings;
  herd: string;              // the ONE herd this ranch hand manages
  resource_mappings: ResourceMapping[];  // remembered assignments
  last_sync?: string;        // ISO timestamp of last sync
}

// K8s metadata for synced livestock
export interface K8sLivestockMetadata {
  namespace: string;
  pod_name: string;
  deployment?: string;
  image: string;
  image_tag?: string;
}

// K8s metadata for synced critters
export interface K8sCritterMetadata {
  namespace: string;
  pod_name: string;
  image: string;
}

// Terraform metadata for synced critters
export interface TerraformCritterMetadata {
  resource_type: string;     // e.g., "aws_db_instance"
  resource_name: string;     // e.g., "postgres"
}

// K8s connection config for barns
export interface K8sBarnConnectionConfig {
  context: string;
  node: string;
}

// ============================================================================
// Core Config Types
// ============================================================================

export interface Config {
  version: number;
  default_project: string | null;
  editor: string;
  theme: 'dark' | 'light';
  show_activity: boolean;
  claude: ClaudeConfig;
  tmux: TmuxConfig;
}

export interface ClaudeConfig {
  model: string;
  auto_attach: boolean;
}

export interface TmuxConfig {
  session_prefix: string;
  default_shell: string;
}

export interface Project {
  name: string;
  path: string;
  summary?: string;           // Short description shown in header
  color?: string;             // Hex color for branding (e.g., "#ff6b6b")
  gradientSpread?: number;    // 0-10, how wide the gradient range is (default 5)
  gradientInverted?: boolean; // Flip gradient direction
  livestock?: Livestock[];    // Deployed instances of this project
  herds?: Herd[];             // Groupings of livestock + critters
  wiki?: WikiSection[];       // Project knowledge base (used by local wiki provider)
  issueProvider?: IssueProviderConfig;  // Issue tracking provider
  wikiProvider?: WikiProviderConfig;    // Wiki provider (defaults to 'local')
}

export interface WikiSection {
  title: string;
  content: string;  // markdown
}

// Herd = a grouping of livestock + critters that work together (e.g., "production", "staging")
export interface Herd {
  name: string;                    // e.g., "production", "staging", "client-a"
  livestock: string[];             // livestock names from this project
  critters: HerdCritterRef[];      // references to critters on barns
  connections: HerdConnection[];   // which livestock talks to which critter
}

// Reference to a critter on a specific barn
export interface HerdCritterRef {
  barn: string;     // barn name
  critter: string;  // critter name on that barn
}

// Connection between livestock and critter (for future use)
export interface HerdConnection {
  livestock: string;  // livestock name
  critter: string;    // critter name (must match a HerdCritterRef.critter)
  barn: string;       // barn name (must match a HerdCritterRef.barn)
}

// Issue tracking provider configuration
export type IssueProviderConfig =
  | { type: 'github' }
  | { type: 'linear'; teamId?: string; teamName?: string }
  | { type: 'none' };

// Wiki provider configuration
export type WikiProviderConfig =
  | { type: 'local' }
  | { type: 'linear'; teamId?: string; teamName?: string };

// Livestock = deployed instance of a project (your apps - Django, Laravel, Node, etc.)
// From project view: "where does my app run?"
// From barn view: "what apps are on this server?"
export interface Livestock {
  name: string;           // display name (e.g., "local", "dev", "production")
  path: string;           // path (local or remote)
  barn?: string;          // if set, this is remote (SSH via barn)
  repo?: string;          // git clone URL
  branch?: string;        // git branch
  // Operational config (paths relative to livestock path)
  log_path?: string;      // e.g., "storage/logs/" or "logs/"
  env_path?: string;      // e.g., ".env" or "config/.env"
  // Ranch Hand sync fields
  source?: EntitySource;  // "manual" or "ranchhand:<name>" - defaults to "manual"
  k8s_metadata?: K8sLivestockMetadata;  // populated for K8s-synced livestock
}

// Critter = system service that supports livestock (nginx, mysql, redis, php-fpm, etc.)
export interface Critter {
  name: string;           // User-friendly name (e.g., "mysql", "redis-cache")
  service: string;        // systemd service name (e.g., "mysql.service")
  service_path?: string;  // systemd unit file path (e.g., "/lib/systemd/system/mysql.service")
  config_path?: string;   // e.g., "/etc/mysql/mysql.conf.d/mysqld.cnf"
  log_path?: string;      // Custom log path if not using journald
  use_journald?: boolean; // Default true - use journalctl for logs
  // Ranch Hand sync fields
  source?: EntitySource;  // "manual" or "ranchhand:<name>" - defaults to "manual"
  endpoint?: string;      // for Terraform resources (RDS endpoint, etc.)
  port?: number;          // service port (for Terraform resources)
  k8s_metadata?: K8sCritterMetadata;      // populated for K8s-synced critters
  tf_metadata?: TerraformCritterMetadata; // populated for Terraform-synced critters
}

// Barn = a server you manage
export interface Barn {
  name: string;
  host?: string;           // Optional for local barn
  user?: string;           // Optional for local barn
  port?: number;           // Optional for local barn
  identity_file?: string;  // Optional for local barn
  critters?: Critter[];    // System services on this barn
  // Note: livestock is derived from projects that reference this barn
  // Ranch Hand sync fields
  source?: EntitySource;   // "manual" or "ranchhand:<name>" - defaults to "manual"
  connection_type?: BarnConnectionType;  // "ssh" (default), "kubernetes", or "terraform"
  connection_config?: K8sBarnConnectionConfig;  // type-specific connection details
  connectable?: boolean;   // false for K8s barns (for now)
}

export interface Session {
  id: string;
  type: 'claude' | 'shell';
  project: string | null;
  livestock: string | null;   // which livestock this session is for
  barn: string | null;
  tmux_session: string;
  tmux_window: number | null;
  started_at: string;
  working_directory: string;
  notes: string;
  status: 'active' | 'detached' | 'ended';
}

// Session info for visualizer (simplified from TmuxWindow)
export interface VisualizerSession {
  index: number;
  name: string;
  type: 'claude' | 'shell';
  statusText: string;  // e.g., "Working...", "idle 5m", "Waiting for input"
  statusIcon: string;  // e.g., "◐", "○", "◉"
}

// Night Sky Visualizer Context
export interface NightSkyContext {
  type: 'global' | 'project' | 'livestock' | 'barn' | 'critter';
  project?: Project;
  livestock?: Livestock;
  barn?: Barn;
  critter?: Critter;
  sessions?: VisualizerSession[];  // Active sessions for this context
}

export type AppView =
  | { type: 'global' }
  | { type: 'project'; project: Project }
  | { type: 'barn'; barn: Barn }
  | { type: 'wiki'; project: Project }
  | { type: 'issues'; project: Project }
  | { type: 'livestock'; project: Project; livestock: Livestock; source: 'project' | 'barn'; sourceBarn?: Barn }
  | { type: 'logs'; project: Project; livestock: Livestock; source: 'project' | 'barn'; sourceBarn?: Barn }
  | { type: 'critter'; barn: Barn; critter: Critter }
  | { type: 'critter-logs'; barn: Barn; critter: Critter }
  | { type: 'herd'; project: Project; herd: Herd }
  | { type: 'ranchhand'; project: Project; ranchhand: RanchHand }
  | { type: 'night-sky' };
