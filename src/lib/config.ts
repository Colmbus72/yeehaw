import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync, unlinkSync } from 'fs';
import YAML from 'js-yaml';
import type { Config, Project, Barn, Livestock, Critter, Session, RanchHand } from '../types.js';
import {
  YEEHAW_DIR,
  CONFIG_FILE,
  PROJECTS_DIR,
  BARNS_DIR,
  RANCHHANDS_DIR,
  SESSIONS_DIR,
  getProjectPath,
  getBarnPath,
  getRanchHandPath,
} from './paths.js';

// The local barn is always available - represents the local machine
export const LOCAL_BARN: Barn = {
  name: 'local',
  critters: [],
};

// Check if a barn is the local machine
export function isLocalBarn(barn: Barn): boolean {
  return barn.name === 'local';
}

/**
 * Type guard to check if a barn has valid SSH configuration.
 * Returns true only if all required SSH fields are present.
 */
export function hasValidSshConfig(barn: Barn): barn is Barn & {
  host: string;
  user: string;
  port: number;
  identity_file: string;
} {
  return (
    !isLocalBarn(barn) &&
    typeof barn.host === 'string' && barn.host.length > 0 &&
    typeof barn.user === 'string' && barn.user.length > 0 &&
    typeof barn.port === 'number' &&
    typeof barn.identity_file === 'string' && barn.identity_file.length > 0
  );
}

const DEFAULT_CONFIG: Config = {
  version: 1,
  default_project: null,
  editor: 'vim',
  theme: 'dark',
  show_activity: true,
  claude: {
    model: 'claude-sonnet-4-20250514',
    auto_attach: true,
  },
  tmux: {
    session_prefix: 'yh-',
    default_shell: '/bin/zsh',
  },
};

export function ensureConfigDirs(): void {
  const dirs = [YEEHAW_DIR, PROJECTS_DIR, BARNS_DIR, RANCHHANDS_DIR, SESSIONS_DIR];
  for (const dir of dirs) {
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
  }
}

export function loadConfig(): Config {
  ensureConfigDirs();

  if (!existsSync(CONFIG_FILE)) {
    writeFileSync(CONFIG_FILE, YAML.dump(DEFAULT_CONFIG), 'utf-8');
    return DEFAULT_CONFIG;
  }

  const content = readFileSync(CONFIG_FILE, 'utf-8');
  const parsed = YAML.load(content) as Partial<Config>;
  return { ...DEFAULT_CONFIG, ...parsed };
}

function normalizeProject(project: Project): Project {
  project.livestock = project.livestock || [];
  return project;
}

export function loadProjects(): Project[] {
  ensureConfigDirs();

  if (!existsSync(PROJECTS_DIR)) return [];

  const files = readdirSync(PROJECTS_DIR).filter((f) => f.endsWith('.yaml'));
  return files.map((file) => {
    const content = readFileSync(getProjectPath(file.replace('.yaml', '')), 'utf-8');
    const project = YAML.load(content) as Project;
    return normalizeProject(project);
  });
}

export function loadProject(name: string): Project | null {
  const path = getProjectPath(name);
  if (!existsSync(path)) return null;

  const content = readFileSync(path, 'utf-8');
  const project = YAML.load(content) as Project;
  return normalizeProject(project);
}

export function loadBarns(): Barn[] {
  ensureConfigDirs();

  // Always include the local barn first
  const barns: Barn[] = [LOCAL_BARN];

  if (!existsSync(BARNS_DIR)) return barns;

  const files = readdirSync(BARNS_DIR).filter((f) => f.endsWith('.yaml'));
  for (const file of files) {
    const content = readFileSync(getBarnPath(file.replace('.yaml', '')), 'utf-8');
    const barn = YAML.load(content) as Barn;
    // Skip if someone manually created a 'local' barn file
    if (barn.name !== 'local') {
      barns.push(barn);
    }
  }
  return barns;
}

export function loadBarn(name: string): Barn | null {
  // Local barn is always available
  if (name === 'local') {
    return LOCAL_BARN;
  }

  const path = getBarnPath(name);
  if (!existsSync(path)) return null;

  const content = readFileSync(path, 'utf-8');
  return YAML.load(content) as Barn;
}

export function saveProject(project: Project): void {
  ensureConfigDirs();
  const path = getProjectPath(project.name);
  const content = YAML.dump(project);
  writeFileSync(path, content, 'utf-8');
}

export function deleteProject(name: string): boolean {
  const path = getProjectPath(name);
  if (!existsSync(path)) return false;
  unlinkSync(path);
  return true;
}

export function saveBarn(barn: Barn): void {
  ensureConfigDirs();
  const path = getBarnPath(barn.name);
  const content = YAML.dump(barn);
  writeFileSync(path, content, 'utf-8');
}

export function deleteBarn(name: string): boolean {
  // Cannot delete the local barn
  if (name === 'local') return false;

  const path = getBarnPath(name);
  if (!existsSync(path)) return false;
  unlinkSync(path);
  return true;
}

// Get all livestock deployed to a specific barn (derived from projects)
export function getLivestockForBarn(barnName: string): Array<{ project: Project; livestock: Livestock }> {
  const projects = loadProjects();
  const result: Array<{ project: Project; livestock: Livestock }> = [];

  for (const project of projects) {
    for (const livestock of project.livestock || []) {
      // Match by barn name, or match local barn with undefined/missing barn field
      if (livestock.barn === barnName || (barnName === 'local' && !livestock.barn)) {
        result.push({ project, livestock });
      }
    }
  }

  return result;
}

// ============================================================================
// Critter operations
// ============================================================================

/**
 * Add a critter to a barn
 */
export function addCritterToBarn(barnName: string, critter: Critter): void {
  const barn = loadBarn(barnName);
  if (!barn) {
    throw new Error(`Barn not found: ${barnName}`);
  }

  barn.critters = barn.critters || [];

  // Check for duplicate
  if (barn.critters.some(c => c.name === critter.name)) {
    throw new Error(`Critter "${critter.name}" already exists on barn "${barnName}"`);
  }

  barn.critters.push(critter);

  // Only save to file if it's not the local barn
  if (barnName !== 'local') {
    saveBarn(barn);
  }
}

/**
 * Remove a critter from a barn
 */
export function removeCritterFromBarn(barnName: string, critterName: string): boolean {
  const barn = loadBarn(barnName);
  if (!barn) {
    throw new Error(`Barn not found: ${barnName}`);
  }

  const originalLength = (barn.critters || []).length;
  barn.critters = (barn.critters || []).filter(c => c.name !== critterName);

  if (barn.critters.length === originalLength) {
    return false; // Critter wasn't found
  }

  // Only save to file if it's not the local barn
  if (barnName !== 'local') {
    saveBarn(barn);
  }
  return true;
}

/**
 * Get a specific critter from a barn
 */
export function getCritter(barnName: string, critterName: string): Critter | undefined {
  const barn = loadBarn(barnName);
  if (!barn) {
    return undefined;
  }
  return barn.critters?.find(c => c.name === critterName);
}

// ============================================================================
// Ranch Hand operations
// ============================================================================

/**
 * Load all ranch hands
 */
export function loadRanchHands(): RanchHand[] {
  ensureConfigDirs();

  if (!existsSync(RANCHHANDS_DIR)) return [];

  const files = readdirSync(RANCHHANDS_DIR).filter((f) => f.endsWith('.yaml'));
  return files.map((file) => {
    const content = readFileSync(getRanchHandPath(file.replace('.yaml', '')), 'utf-8');
    return YAML.load(content) as RanchHand;
  });
}

/**
 * Load a specific ranch hand by name
 */
export function loadRanchHand(name: string): RanchHand | null {
  const path = getRanchHandPath(name);
  if (!existsSync(path)) return null;

  const content = readFileSync(path, 'utf-8');
  return YAML.load(content) as RanchHand;
}

/**
 * Load all ranch hands for a specific project
 */
export function loadRanchHandsForProject(projectName: string): RanchHand[] {
  return loadRanchHands().filter(rh => rh.project === projectName);
}

/**
 * Save a ranch hand
 */
export function saveRanchHand(ranchhand: RanchHand): void {
  ensureConfigDirs();
  const path = getRanchHandPath(ranchhand.name);
  const content = YAML.dump(ranchhand);
  writeFileSync(path, content, 'utf-8');
}

/**
 * Delete a ranch hand
 */
export function deleteRanchHand(name: string): boolean {
  const path = getRanchHandPath(name);
  if (!existsSync(path)) return false;
  unlinkSync(path);
  return true;
}

/**
 * Update ranch hand's last sync timestamp
 */
export function updateRanchHandLastSync(name: string): void {
  const ranchhand = loadRanchHand(name);
  if (!ranchhand) {
    throw new Error(`Ranch hand not found: ${name}`);
  }
  ranchhand.last_sync = new Date().toISOString();
  saveRanchHand(ranchhand);
}

/**
 * Add a resource mapping to a ranch hand (remembers herd assignments)
 */
export function addRanchHandResourceMapping(
  ranchhandName: string,
  resourceId: string,
  herdName: string
): void {
  const ranchhand = loadRanchHand(ranchhandName);
  if (!ranchhand) {
    throw new Error(`Ranch hand not found: ${ranchhandName}`);
  }

  // Remove existing mapping for this resource if any
  ranchhand.resource_mappings = ranchhand.resource_mappings.filter(
    m => m.resource_id !== resourceId
  );

  // Add new mapping
  ranchhand.resource_mappings.push({ resource_id: resourceId, herd_name: herdName });
  saveRanchHand(ranchhand);
}
