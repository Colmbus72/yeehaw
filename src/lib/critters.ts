import { execa } from 'execa';
import type { Critter, Barn } from '../types.js';
import { buildSshCommand } from './livestock.js';
import { shellEscape } from './shell.js';
import { getErrorMessage } from './errors.js';

/**
 * Services that are interesting for developers (databases, web servers, etc.)
 */
const INTERESTING_SERVICE_PATTERNS = [
  // Databases
  'mysql', 'mariadb', 'postgres', 'postgresql', 'mongodb', 'mongo',
  // Caches
  'redis', 'memcached',
  // Web servers
  'nginx', 'apache', 'httpd', 'caddy',
  // PHP
  'php-fpm', 'php7', 'php8',
  // Python
  'gunicorn', 'uvicorn', 'celery',
  // Node
  'node', 'pm2',
  // Mail
  'postfix', 'dovecot',
  // Queue
  'rabbitmq', 'kafka',
  // Search
  'elasticsearch', 'opensearch', 'meilisearch',
  // Other
  'supervisor', 'docker',
];

/**
 * Common Supervisor config directories to check
 */
const SUPERVISOR_CONFIG_PATHS = [
  '/etc/supervisor/conf.d',
  '/etc/supervisord.d',
  '/etc/supervisor.d',
];

/**
 * Check if a service name matches any interesting pattern
 */
function isInterestingService(serviceName: string): boolean {
  const lower = serviceName.toLowerCase();
  return INTERESTING_SERVICE_PATTERNS.some(pattern => lower.includes(pattern));
}

/**
 * Extract a friendly name from a service name
 * e.g., "mysql.service" -> "mysql", "php8.1-fpm.service" -> "php8.1-fpm"
 */
function extractServiceName(service: string): string {
  return service.replace(/\.service$/, '');
}

/**
 * Discovered critter from scanning a barn
 */
export interface DiscoveredCritter {
  service: string;        // e.g., "mysql.service" or "supervisor:myapp"
  suggested_name: string; // e.g., "mysql" or "daemon-388186"
  command?: string;       // full command (for supervisor), useful for display/search
  binary?: string;        // extracted from ExecStart or command
  config_path?: string;   // if detectable
  log_path?: string;      // if detectable (especially for supervisor)
  status: 'running' | 'stopped' | 'unknown';
  manager: 'systemd' | 'supervisor';  // which process manager runs this
}

/**
 * Check if a service is managed by Supervisor (service name starts with "supervisor:")
 */
function isSupervisorService(service: string): boolean {
  return service.startsWith('supervisor:');
}

/**
 * Extract the program name from a Supervisor service identifier
 */
function getSupervisorProgramName(service: string): string {
  return service.replace(/^supervisor:/, '');
}

/**
 * Read logs from a critter (via journald, custom path, or supervisorctl)
 */
export async function readCritterLogs(
  critter: Critter,
  barn: Barn,
  options: { lines?: number; pattern?: string } = {}
): Promise<{ content: string; error?: string }> {
  const { lines = 100, pattern } = options;
  const escapedLines = String(lines);

  let cmd: string;

  // Check if this is a Supervisor-managed service
  if (isSupervisorService(critter.service)) {
    const programName = getSupervisorProgramName(critter.service);

    if (critter.log_path) {
      // Use the configured log path
      const escapedLogPath = shellEscape(critter.log_path);
      cmd = `tail -n ${escapedLines} ${escapedLogPath}`;
      if (pattern) {
        const escapedPattern = shellEscape(pattern);
        cmd += ` | grep -i ${escapedPattern} || true`;
      }
    } else {
      // Fall back to supervisorctl tail
      const escapedProgram = shellEscape(programName);
      cmd = `supervisorctl tail -${escapedLines} ${escapedProgram}`;
      if (pattern) {
        const escapedPattern = shellEscape(pattern);
        cmd += ` | grep -i ${escapedPattern} || true`;
      }
    }
  } else if (critter.use_journald !== false) {
    // Use journalctl for systemd services
    const escapedService = shellEscape(critter.service);
    cmd = `journalctl -u ${escapedService} -n ${escapedLines} --no-pager`;
    if (pattern) {
      const escapedPattern = shellEscape(pattern);
      cmd += ` | grep -i ${escapedPattern} || true`;
    }
  } else if (critter.log_path) {
    // Use custom log path
    const escapedLogPath = shellEscape(critter.log_path);
    cmd = `tail -n ${escapedLines} ${escapedLogPath}`;
    if (pattern) {
      const escapedPattern = shellEscape(pattern);
      cmd += ` | grep -i ${escapedPattern} || true`;
    }
  } else {
    return { content: '', error: 'Critter has no log_path and use_journald is disabled' };
  }

  // Local barn
  if (barn.name === 'local') {
    try {
      const result = await execa('sh', ['-c', cmd]);
      if (!result.stdout.trim()) {
        return { content: '', error: `No logs found for ${critter.name}` };
      }
      return { content: result.stdout };
    } catch (err) {
      return { content: '', error: `Failed to read logs: ${getErrorMessage(err)}` };
    }
  }

  // Remote barn - SSH
  if (!barn.host || !barn.user) {
    return { content: '', error: `Barn '${barn.name}' is not configured for SSH` };
  }

  try {
    const sshArgs = buildSshCommand(barn);
    const result = await execa(sshArgs[0], [...sshArgs.slice(1), cmd]);
    if (!result.stdout.trim()) {
      return { content: '', error: `No logs found for ${critter.name}` };
    }
    return { content: result.stdout };
  } catch (err) {
    return { content: '', error: `SSH error: ${getErrorMessage(err)}` };
  }
}

/**
 * Parse systemctl show output into key-value pairs
 */
function parseSystemctlShow(output: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of output.split('\n')) {
    const eqIndex = line.indexOf('=');
    if (eqIndex !== -1) {
      const key = line.slice(0, eqIndex);
      const value = line.slice(eqIndex + 1);
      result[key] = value;
    }
  }
  return result;
}

/**
 * Parse INI-style Supervisor config file
 */
function parseSupervisorConfig(content: string): Record<string, Record<string, string>> {
  const sections: Record<string, Record<string, string>> = {};
  let currentSection = '';

  for (const line of content.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith(';') || trimmed.startsWith('#')) continue;

    // Section header [program:name] or [group:name]
    const sectionMatch = trimmed.match(/^\[([^\]]+)\]$/);
    if (sectionMatch) {
      currentSection = sectionMatch[1];
      sections[currentSection] = {};
      continue;
    }

    // Key=value pair
    if (currentSection) {
      const eqIndex = trimmed.indexOf('=');
      if (eqIndex !== -1) {
        const key = trimmed.slice(0, eqIndex).trim();
        const value = trimmed.slice(eqIndex + 1).trim();
        sections[currentSection][key] = value;
      }
    }
  }

  return sections;
}

/**
 * Discover Supervisor-managed programs on a barn
 * Scans config files directly (no sudo required)
 */
async function discoverSupervisorPrograms(
  barn: Barn
): Promise<DiscoveredCritter[]> {
  if (barn.name !== 'local' && (!barn.host || !barn.user)) return [];

  const programs: DiscoveredCritter[] = [];

  for (const configDir of SUPERVISOR_CONFIG_PATHS) {
    // List all .conf and .ini files in the directory
    const listCmd = `ls -1 ${configDir}/*.conf ${configDir}/*.ini 2>/dev/null || true`;

    let configFiles: string[] = [];

    try {
      let listOutput: string;

      if (barn.name === 'local') {
        const result = await execa('sh', ['-c', listCmd]);
        listOutput = result.stdout;
      } else {
        const sshArgs = buildSshCommand(barn);
        const result = await execa(sshArgs[0], [...sshArgs.slice(1), listCmd]);
        listOutput = result.stdout;
      }

      configFiles = listOutput.split('\n').filter(f => f.trim());
    } catch {
      continue; // Directory doesn't exist or no configs
    }

    // Parse each config file
    for (const confPath of configFiles) {
      const catCmd = `cat ${shellEscape(confPath)} 2>/dev/null || true`;

      try {
        let configContent: string;

        if (barn.name === 'local') {
          const result = await execa('sh', ['-c', catCmd]);
          configContent = result.stdout;
        } else {
          const sshArgs = buildSshCommand(barn);
          const result = await execa(sshArgs[0], [...sshArgs.slice(1), catCmd]);
          configContent = result.stdout;
        }

        if (!configContent.trim()) continue;

        const sections = parseSupervisorConfig(configContent);

        // Find all [program:name] sections in this file
        for (const [sectionName, sectionData] of Object.entries(sections)) {
          if (!sectionName.startsWith('program:')) continue;

          const programName = sectionName.replace('program:', '');

          // Skip if we've already processed this program (from another config dir)
          if (programs.some(p => p.service === `supervisor:${programName}`)) continue;

          const command = sectionData.command || '';
          const binary = command.split(/\s+/)[0];

          // Get log path
          let log_path: string | undefined = sectionData.stdout_logfile ||
                        sectionData.stderr_logfile ||
                        sectionData.logfile;

          // Skip template paths
          if (log_path && (log_path.includes('%(') || log_path === 'AUTO' || log_path === 'NONE')) {
            log_path = undefined;
          }

          programs.push({
            service: `supervisor:${programName}`,
            suggested_name: programName,
            command,  // include full command for display/search
            binary,
            config_path: confPath,
            log_path,
            status: 'unknown',  // can't determine without sudo
            manager: 'supervisor',
          });
        }
      } catch {
        // Config file read error, continue
      }
    }
  }

  return programs;
}

/**
 * Discover systemd services on a barn
 */
async function discoverSystemdServices(
  barn: Barn
): Promise<DiscoveredCritter[]> {
  // Command to list running services
  const listCmd = 'systemctl list-units --type=service --state=running --no-pager --plain 2>/dev/null || true';

  let output: string;

  // Local barn
  if (barn.name === 'local') {
    try {
      const result = await execa('sh', ['-c', listCmd]);
      output = result.stdout;
    } catch {
      return [];
    }
  } else {
    // Remote barn - SSH
    if (!barn.host || !barn.user) {
      return [];
    }

    try {
      const sshArgs = buildSshCommand(barn);
      const result = await execa(sshArgs[0], [...sshArgs.slice(1), listCmd]);
      output = result.stdout;
    } catch {
      return [];
    }
  }

  // Parse the output - each line: "unit.service loaded active running description"
  const lines = output.split('\n').filter(line => line.trim());
  const discovered: DiscoveredCritter[] = [];

  for (const line of lines) {
    const parts = line.trim().split(/\s+/);
    if (parts.length < 1) continue;

    const service = parts[0];
    if (!service.endsWith('.service')) continue;
    if (!isInterestingService(service)) continue;

    const suggested_name = extractServiceName(service);

    // Try to get more details about this service
    let binary: string | undefined;
    let config_path: string | undefined;

    const showCmd = `systemctl show ${shellEscape(service)} --property=ExecStart`;

    try {
      let showOutput: string;

      if (barn.name === 'local') {
        const showResult = await execa('sh', ['-c', showCmd]);
        showOutput = showResult.stdout;
      } else {
        const sshArgs = buildSshCommand(barn);
        const showResult = await execa(sshArgs[0], [...sshArgs.slice(1), showCmd]);
        showOutput = showResult.stdout;
      }

      const props = parseSystemctlShow(showOutput);

      // Extract binary from ExecStart (format: { path=/usr/bin/mysqld ; argv[]=... })
      if (props.ExecStart) {
        const pathMatch = props.ExecStart.match(/path=([^\s;]+)/);
        if (pathMatch) {
          binary = pathMatch[1];
        }

        // Try to find config flags like --config= or -c
        const configMatch = props.ExecStart.match(/--config[=\s]([^\s;]+)/);
        if (configMatch) {
          config_path = configMatch[1];
        }
      }
    } catch {
      // Ignore errors getting details - just use basic info
    }

    discovered.push({
      service,
      suggested_name,
      binary,
      config_path,
      status: 'running',
      manager: 'systemd',
    });
  }

  return discovered;
}

/**
 * Discover critters (running services) on a barn
 * Checks both systemd and Supervisor
 */
export async function discoverCritters(
  barn: Barn
): Promise<{ critters: DiscoveredCritter[]; error?: string }> {
  const discovered: DiscoveredCritter[] = [];
  const errors: string[] = [];

  // Discover systemd services
  try {
    const systemdCritters = await discoverSystemdServices(barn);
    discovered.push(...systemdCritters);
  } catch (err) {
    errors.push(`systemd: ${getErrorMessage(err)}`);
  }

  // Discover Supervisor programs
  try {
    const supervisorCritters = await discoverSupervisorPrograms(barn);
    discovered.push(...supervisorCritters);
  } catch (err) {
    errors.push(`supervisor: ${getErrorMessage(err)}`);
  }

  return {
    critters: discovered,
    error: errors.length > 0 ? errors.join('; ') : undefined,
  };
}

/**
 * Service info from systemctl
 */
export interface SystemService {
  name: string;      // e.g., "mysql.service"
  state: 'running' | 'stopped' | 'unknown';
  description?: string;
}

/**
 * List systemd services on a barn
 * @param barn - The barn to query
 * @param activeOnly - If true, only return running services (default: true)
 */
export async function listSystemServices(
  barn: Barn,
  activeOnly: boolean = true
): Promise<{ services: SystemService[]; error?: string }> {
  // For active only: list-units shows running services
  // For all: list-unit-files shows all installed services
  const cmd = activeOnly
    ? 'systemctl list-units --type=service --state=running --no-pager --no-legend'
    : 'systemctl list-unit-files --type=service --no-pager --no-legend';

  let output: string;

  // Local barn
  if (barn.name === 'local') {
    try {
      const result = await execa('sh', ['-c', cmd]);
      output = result.stdout;
    } catch (err) {
      return { services: [], error: `Failed to list services: ${getErrorMessage(err)}` };
    }
  } else {
    // Remote barn - SSH
    if (!barn.host || !barn.user) {
      return { services: [], error: `Barn '${barn.name}' is not configured for SSH` };
    }

    try {
      const sshArgs = buildSshCommand(barn);
      const result = await execa(sshArgs[0], [...sshArgs.slice(1), cmd]);
      output = result.stdout;
    } catch (err) {
      return { services: [], error: `SSH error: ${getErrorMessage(err)}` };
    }
  }

  const services: SystemService[] = [];
  const lines = output.split('\n').filter(line => line.trim());

  for (const line of lines) {
    const parts = line.trim().split(/\s+/);
    if (parts.length < 1) continue;

    const serviceName = parts[0];
    if (!serviceName.endsWith('.service')) continue;

    if (activeOnly) {
      // list-units format: UNIT LOAD ACTIVE SUB DESCRIPTION...
      services.push({
        name: serviceName,
        state: 'running',
        description: parts.slice(4).join(' ') || undefined,
      });
    } else {
      // list-unit-files format: UNIT STATE PRESET
      const stateStr = parts[1]?.toLowerCase() || '';
      services.push({
        name: serviceName,
        state: stateStr === 'enabled' ? 'unknown' : 'stopped',
      });
    }
  }

  return { services };
}

/**
 * Details extracted from a systemd service file
 */
export interface ServiceDetails {
  service_path: string;    // Path to the unit file
  config_path?: string;    // Detected config file path
  log_path?: string;       // Detected log path (if not using journald)
  use_journald: boolean;   // Whether service logs to journal
}

/**
 * Get details about a Supervisor program by finding and parsing its config file
 * Searches all config files since program name may not match filename (e.g., Forge daemons)
 */
async function getSupervisorProgramDetails(
  barn: Barn,
  programName: string
): Promise<{ details?: ServiceDetails; error?: string }> {
  if (!barn.host && barn.name !== 'local') {
    return { error: `Barn '${barn.name}' is not configured for SSH` };
  }

  // First try direct file name match (fast path)
  for (const configDir of SUPERVISOR_CONFIG_PATHS) {
    const possiblePaths = [
      `${configDir}/${programName}.conf`,
      `${configDir}/${programName}.ini`,
    ];

    for (const confPath of possiblePaths) {
      const catCmd = `cat ${shellEscape(confPath)} 2>/dev/null`;

      try {
        let configContent: string;

        if (barn.name === 'local') {
          const result = await execa('sh', ['-c', catCmd]);
          configContent = result.stdout;
        } else {
          const sshArgs = buildSshCommand(barn);
          const result = await execa(sshArgs[0], [...sshArgs.slice(1), catCmd]);
          configContent = result.stdout;
        }

        if (configContent.trim()) {
          const sections = parseSupervisorConfig(configContent);
          const programSection = sections[`program:${programName}`];

          if (programSection) {
            return extractDetailsFromProgramSection(confPath, programSection);
          }
        }
      } catch {
        // Config file not found, continue
      }
    }
  }

  // Fallback: scan all config files (handles Forge-style numeric daemon names)
  for (const configDir of SUPERVISOR_CONFIG_PATHS) {
    const listCmd = `ls -1 ${shellEscape(configDir)}/*.conf ${shellEscape(configDir)}/*.ini 2>/dev/null || true`;

    let configFiles: string[] = [];

    try {
      let listOutput: string;

      if (barn.name === 'local') {
        const result = await execa('sh', ['-c', listCmd]);
        listOutput = result.stdout;
      } else {
        const sshArgs = buildSshCommand(barn);
        const result = await execa(sshArgs[0], [...sshArgs.slice(1), listCmd]);
        listOutput = result.stdout;
      }

      configFiles = listOutput.split('\n').filter(f => f.trim());
    } catch {
      continue;
    }

    for (const confPath of configFiles) {
      const catCmd = `cat ${shellEscape(confPath)} 2>/dev/null || true`;

      try {
        let configContent: string;

        if (barn.name === 'local') {
          const result = await execa('sh', ['-c', catCmd]);
          configContent = result.stdout;
        } else {
          const sshArgs = buildSshCommand(barn);
          const result = await execa(sshArgs[0], [...sshArgs.slice(1), catCmd]);
          configContent = result.stdout;
        }

        if (!configContent.trim()) continue;

        const sections = parseSupervisorConfig(configContent);
        const programSection = sections[`program:${programName}`];

        if (programSection) {
          return extractDetailsFromProgramSection(confPath, programSection);
        }
      } catch {
        continue;
      }
    }
  }

  return { error: `Could not find config file for Supervisor program '${programName}'` };
}

/**
 * Extract ServiceDetails from a parsed Supervisor program section
 */
function extractDetailsFromProgramSection(
  confPath: string,
  programSection: Record<string, string>
): { details: ServiceDetails } {
  let config_path: string | undefined;
  let log_path: string | undefined;

  // Check for any config file references in the command
  const command = programSection.command || '';
  const configPatterns = [
    /--config[=\s]([^\s]+)/,
    /-c\s+([^\s]+)/,
    /--conf[=\s]([^\s]+)/,
    /--settings[=\s]([^\s]+)/,
  ];
  for (const pattern of configPatterns) {
    const match = command.match(pattern);
    if (match) {
      config_path = match[1];
      break;
    }
  }

  // Get log path
  log_path = programSection.stdout_logfile ||
            programSection.stderr_logfile ||
            programSection.logfile;

  // Skip template paths
  if (log_path && (log_path.includes('%(') || log_path === 'AUTO' || log_path === 'NONE')) {
    log_path = undefined;
  }

  return {
    details: {
      service_path: confPath,
      config_path,
      log_path,
      use_journald: false, // Supervisor doesn't use journald
    },
  };
}

/**
 * Get details about a systemd service by parsing its unit file
 */
async function getSystemdServiceDetails(
  barn: Barn,
  serviceName: string
): Promise<{ details?: ServiceDetails; error?: string }> {
  // Get the unit file path
  const pathCmd = `systemctl show -p FragmentPath ${shellEscape(serviceName)} --value`;

  let servicePath: string;

  if (barn.name === 'local') {
    try {
      const result = await execa('sh', ['-c', pathCmd]);
      servicePath = result.stdout.trim();
    } catch (err) {
      return { error: `Failed to get service path: ${getErrorMessage(err)}` };
    }
  } else {
    if (!barn.host || !barn.user) {
      return { error: `Barn '${barn.name}' is not configured for SSH` };
    }
    try {
      const sshArgs = buildSshCommand(barn);
      const result = await execa(sshArgs[0], [...sshArgs.slice(1), pathCmd]);
      servicePath = result.stdout.trim();
    } catch (err) {
      return { error: `SSH error: ${getErrorMessage(err)}` };
    }
  }

  if (!servicePath) {
    return { error: 'Could not find service unit file' };
  }

  // Read the service file to extract details
  const catCmd = `cat ${shellEscape(servicePath)}`;
  let serviceContent: string;

  if (barn.name === 'local') {
    try {
      const result = await execa('sh', ['-c', catCmd]);
      serviceContent = result.stdout;
    } catch (err) {
      return { error: `Failed to read service file: ${getErrorMessage(err)}` };
    }
  } else {
    try {
      const sshArgs = buildSshCommand(barn);
      const result = await execa(sshArgs[0], [...sshArgs.slice(1), catCmd]);
      serviceContent = result.stdout;
    } catch (err) {
      return { error: `SSH error: ${getErrorMessage(err)}` };
    }
  }

  // Parse the service file
  let config_path: string | undefined;
  let log_path: string | undefined;
  let use_journald = true;

  for (const line of serviceContent.split('\n')) {
    const trimmed = line.trim();

    // Look for ExecStart to find config flags
    if (trimmed.startsWith('ExecStart=')) {
      const execLine = trimmed.slice('ExecStart='.length);
      // Common config flag patterns
      const configPatterns = [
        /--config[=\s]([^\s]+)/,
        /--defaults-file[=\s]([^\s]+)/,
        /-c\s+([^\s]+)/,
        /--conf[=\s]([^\s]+)/,
      ];
      for (const pattern of configPatterns) {
        const match = execLine.match(pattern);
        if (match) {
          config_path = match[1];
          break;
        }
      }
    }

    // Check StandardOutput/StandardError for log paths
    if (trimmed.startsWith('StandardOutput=') || trimmed.startsWith('StandardError=')) {
      const value = trimmed.split('=')[1];
      if (value && !value.startsWith('journal') && !value.startsWith('inherit')) {
        // Could be file:path or append:path
        const fileMatch = value.match(/(?:file|append):(.+)/);
        if (fileMatch) {
          log_path = fileMatch[1];
          use_journald = false;
        }
      }
    }
  }

  return {
    details: {
      service_path: servicePath,
      config_path,
      log_path,
      use_journald,
    },
  };
}

/**
 * Get details about a service (systemd or Supervisor)
 */
export async function getServiceDetails(
  barn: Barn,
  serviceName: string
): Promise<{ details?: ServiceDetails; error?: string }> {
  // Check if this is a Supervisor service
  if (isSupervisorService(serviceName)) {
    const programName = getSupervisorProgramName(serviceName);
    return getSupervisorProgramDetails(barn, programName);
  }

  // Otherwise treat as systemd service
  return getSystemdServiceDetails(barn, serviceName);
}
