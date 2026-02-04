#!/usr/bin/env node
/**
 * Yeehaw MCP Server
 *
 * Exposes Yeehaw project/barn/livestock management to Claude Code sessions.
 *
 * Terminology:
 * - Project: A codebase you're working on
 * - Barn: A server you manage
 * - Livestock: Deployed instances of your apps (local or on barns)
 * - Critter: System services that support livestock (nginx, mysql, etc.)
 *
 * Usage: Add to Claude Code's MCP config:
 * {
 *   "mcpServers": {
 *     "yeehaw": {
 *       "command": "node",
 *       "args": ["/path/to/yeehaw/dist/mcp-server.js"]
 *     }
 *   }
 * }
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ListResourcesRequestSchema,
  ReadResourceRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import {
  loadProjects,
  loadProject,
  loadBarns,
  loadBarn,
  saveProject,
  saveBarn,
  deleteProject,
  deleteBarn,
  getLivestockForBarn,
  ensureConfigDirs,
  addCritterToBarn,
  removeCritterFromBarn,
  getCritter,
  loadRanchHands,
  loadRanchHand,
  loadRanchHandsForProject,
  saveRanchHand,
  deleteRanchHand,
  updateRanchHandLastSync,
  addRanchHandResourceMapping,
} from './lib/config.js';
import type { Project, Barn, Livestock, Critter, WikiSection, Herd, RanchHand, KubernetesConfig, TerraformConfig } from './types.js';
import {
  discoverK8sResources,
  syncK8sResources,
  getKubectlContexts,
  getCurrentKubectlContext,
} from './lib/ranchhand-k8s.js';
import {
  discoverTerraformResources,
  syncTerraformResources,
  testS3Access,
  listS3StateFiles,
} from './lib/ranchhand-terraform.js';
import { readLivestockLogs, readLivestockEnv } from './lib/livestock.js';
import { readCritterLogs, discoverCritters } from './lib/critters.js';
import { getWikiProvider, isWikiReadOnly, type WikiSection as WikiSectionType } from './lib/wiki/index.js';
import {
  requireString,
  optionalString,
  optionalNumber,
  optionalBoolean,
  type McpArgs,
} from './lib/mcp-validation.js';

const server = new Server(
  {
    name: 'yeehaw',
    version: '0.2.0',
  },
  {
    capabilities: {
      tools: {},
      resources: {},
    },
  }
);

// ============================================================================
// Resources - Data Claude can read
// ============================================================================

server.setRequestHandler(ListResourcesRequestSchema, async () => {
  const projects = loadProjects();
  const barns = loadBarns();

  return {
    resources: [
      ...projects.map((p) => ({
        uri: `yeehaw://project/${p.name}`,
        name: `Project: ${p.name}`,
        description: p.summary || `Project at ${p.path}`,
        mimeType: 'application/json',
      })),
      ...barns.map((b) => ({
        uri: `yeehaw://barn/${b.name}`,
        name: `Barn: ${b.name}`,
        description: b.name === 'local' ? 'Local machine' : `Server at ${b.host}`,
        mimeType: 'application/json',
      })),
      {
        uri: 'yeehaw://projects',
        name: 'All Projects',
        description: 'List of all Yeehaw projects',
        mimeType: 'application/json',
      },
      {
        uri: 'yeehaw://barns',
        name: 'All Barns',
        description: 'List of all Yeehaw barns (servers)',
        mimeType: 'application/json',
      },
    ],
  };
});

server.setRequestHandler(ReadResourceRequestSchema, async (request) => {
  const { uri } = request.params;

  if (uri === 'yeehaw://projects') {
    const projects = loadProjects();
    // Return simplified list - use yeehaw://project/{name} for full details
    const simplified = projects.map((p) => ({
      name: p.name,
      path: p.path,
      summary: p.summary,
      livestock: (p.livestock || []).map((l) => l.name),
    }));
    return {
      contents: [
        {
          uri,
          mimeType: 'application/json',
          text: JSON.stringify(simplified, null, 2),
        },
      ],
    };
  }

  if (uri === 'yeehaw://barns') {
    const barns = loadBarns();
    // Return simplified list - use yeehaw://barn/{name} for full details
    const simplified = barns.map((b) => ({
      name: b.name,
      host: b.host,
      critters: (b.critters || []).map((c) => c.name),
    }));
    return {
      contents: [
        {
          uri,
          mimeType: 'application/json',
          text: JSON.stringify(simplified, null, 2),
        },
      ],
    };
  }

  const projectMatch = uri.match(/^yeehaw:\/\/project\/(.+)$/);
  if (projectMatch) {
    const project = loadProject(projectMatch[1]);
    if (!project) {
      throw new Error(`Project not found: ${projectMatch[1]}`);
    }
    // Return simplified project data
    // Use get_wiki_section, get_herd tools for full details
    const simplified = {
      name: project.name,
      path: project.path,
      summary: project.summary,
      color: project.color,
      issueProvider: project.issueProvider,
      livestock: (project.livestock || []).map((l) => ({
        name: l.name,
        barn: l.barn || 'local',
      })),
      herds: (project.herds || []).map((h) => h.name),
      wiki: (project.wiki || []).map((s) => s.title),
    };
    return {
      contents: [
        {
          uri,
          mimeType: 'application/json',
          text: JSON.stringify(simplified, null, 2),
        },
      ],
    };
  }

  const barnMatch = uri.match(/^yeehaw:\/\/barn\/(.+)$/);
  if (barnMatch) {
    const barn = loadBarn(barnMatch[1]);
    if (!barn) {
      throw new Error(`Barn not found: ${barnMatch[1]}`);
    }
    // Return barn details with simplified references to deployed resources
    // Use yeehaw://project/{name} to get full project details
    const livestock = getLivestockForBarn(barn.name);
    const simplifiedLivestock = livestock.map((l) => ({
      project: l.project.name,
      livestock: l.livestock.name,
    }));
    const result = {
      name: barn.name,
      host: barn.host,
      user: barn.user,
      port: barn.port,
      identity_file: barn.identity_file,
      critters: (barn.critters || []).map((c) => ({
        name: c.name,
        service: c.service,
      })),
      deployedLivestock: simplifiedLivestock,
    };
    return {
      contents: [
        {
          uri,
          mimeType: 'application/json',
          text: JSON.stringify(result, null, 2),
        },
      ],
    };
  }

  throw new Error(`Unknown resource: ${uri}`);
});

// ============================================================================
// Tools - Actions Claude can take
// ============================================================================

server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      // Project tools
      {
        name: 'list_projects',
        description: 'List all Yeehaw projects',
        inputSchema: {
          type: 'object',
          properties: {},
        },
      },
      {
        name: 'get_project',
        description: 'Get details of a specific project including its livestock',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Project name' },
          },
          required: ['name'],
        },
      },
      {
        name: 'create_project',
        description: 'Create a new project',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Project name' },
            path: { type: 'string', description: 'Local path to project' },
            summary: { type: 'string', description: 'Short description' },
            color: { type: 'string', description: 'Hex color (e.g., #ff6b6b)' },
          },
          required: ['name', 'path'],
        },
      },
      {
        name: 'update_project',
        description: 'Update an existing project',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Project name to update' },
            summary: { type: 'string', description: 'New summary' },
            color: { type: 'string', description: 'New hex color' },
            path: { type: 'string', description: 'New path' },
          },
          required: ['name'],
        },
      },
      {
        name: 'delete_project',
        description: 'Delete a project (requires confirmation)',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Project name to delete' },
            confirm: { type: 'string', description: 'Must match project name to confirm deletion' },
          },
          required: ['name', 'confirm'],
        },
      },
      // Livestock tools
      {
        name: 'add_livestock',
        description: 'Add livestock (deployed app instance) to a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            name: { type: 'string', description: 'Livestock name (e.g., local, dev, production)' },
            path: { type: 'string', description: 'Path (local or remote)' },
            barn: { type: 'string', description: 'Barn name for remote livestock' },
            repo: { type: 'string', description: 'Git repository URL' },
            branch: { type: 'string', description: 'Git branch' },
            log_path: { type: 'string', description: 'Path to logs relative to livestock path (e.g., storage/logs/)' },
            env_path: { type: 'string', description: 'Path to env file relative to livestock path (e.g., .env)' },
          },
          required: ['project', 'name', 'path'],
        },
      },
      {
        name: 'remove_livestock',
        description: 'Remove livestock from a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            name: { type: 'string', description: 'Livestock name to remove' },
          },
          required: ['project', 'name'],
        },
      },
      {
        name: 'read_livestock_logs',
        description: 'Read log files from a livestock deployment',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            livestock: { type: 'string', description: 'Livestock name (e.g., production)' },
            lines: { type: 'number', description: 'Last N lines (default: 100)' },
            pattern: { type: 'string', description: 'Grep pattern to filter logs (case-insensitive)' },
          },
          required: ['project', 'livestock'],
        },
      },
      {
        name: 'read_livestock_env',
        description: 'Read environment config from a livestock deployment',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            livestock: { type: 'string', description: 'Livestock name' },
            show_values: { type: 'boolean', description: 'Show values (default: false, keys only for security)' },
          },
          required: ['project', 'livestock'],
        },
      },
      // Barn tools
      {
        name: 'list_barns',
        description: 'List all Yeehaw barns (servers)',
        inputSchema: {
          type: 'object',
          properties: {},
        },
      },
      {
        name: 'get_barn',
        description: 'Get details of a specific barn including deployed livestock',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Barn name' },
          },
          required: ['name'],
        },
      },
      {
        name: 'create_barn',
        description: 'Create a new barn (server)',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Barn name (identifier)' },
            host: { type: 'string', description: 'Hostname or IP address' },
            user: { type: 'string', description: 'SSH username' },
            port: { type: 'number', description: 'SSH port (default: 22)' },
            identity_file: { type: 'string', description: 'Path to SSH private key' },
          },
          required: ['name', 'host', 'user', 'identity_file'],
        },
      },
      {
        name: 'update_barn',
        description: 'Update an existing barn',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Barn name to update' },
            host: { type: 'string', description: 'New hostname or IP' },
            user: { type: 'string', description: 'New SSH username' },
            port: { type: 'number', description: 'New SSH port' },
            identity_file: { type: 'string', description: 'New path to SSH key' },
          },
          required: ['name'],
        },
      },
      {
        name: 'delete_barn',
        description: 'Delete a barn (requires confirmation)',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Barn name to delete' },
            confirm: { type: 'string', description: 'Must match barn name to confirm deletion' },
          },
          required: ['name', 'confirm'],
        },
      },
      // Wiki tools
      {
        name: 'get_wiki',
        description: 'Get all wiki section titles for a project (use get_wiki_section to fetch content)',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
          },
          required: ['project'],
        },
      },
      {
        name: 'get_wiki_section',
        description: 'Get the content of a specific wiki section',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            title: { type: 'string', description: 'Section title' },
          },
          required: ['project', 'title'],
        },
      },
      {
        name: 'add_wiki_section',
        description: 'Add a new wiki section to a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            title: { type: 'string', description: 'Section title' },
            content: { type: 'string', description: 'Section content (markdown)' },
          },
          required: ['project', 'title', 'content'],
        },
      },
      {
        name: 'update_wiki_section',
        description: 'Update an existing wiki section',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            title: { type: 'string', description: 'Section title to update' },
            new_title: { type: 'string', description: 'New title (optional)' },
            content: { type: 'string', description: 'New content (optional)' },
          },
          required: ['project', 'title'],
        },
      },
      {
        name: 'delete_wiki_section',
        description: 'Delete a wiki section from a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            title: { type: 'string', description: 'Section title to delete' },
          },
          required: ['project', 'title'],
        },
      },
      // Critter tools
      {
        name: 'add_critter',
        description: 'Add a critter (system service like mysql, redis, nginx) to a barn',
        inputSchema: {
          type: 'object',
          properties: {
            barn: { type: 'string', description: 'Barn name' },
            name: { type: 'string', description: 'Critter name (user-friendly, e.g., "mysql", "redis-cache")' },
            service: { type: 'string', description: 'systemd service name (e.g., "mysql.service")' },
            config_path: { type: 'string', description: 'Path to config file (optional)' },
            log_path: { type: 'string', description: 'Custom log path if not using journald (optional)' },
            use_journald: { type: 'boolean', description: 'Use journalctl for logs (default: true)' },
          },
          required: ['barn', 'name', 'service'],
        },
      },
      {
        name: 'remove_critter',
        description: 'Remove a critter from a barn',
        inputSchema: {
          type: 'object',
          properties: {
            barn: { type: 'string', description: 'Barn name' },
            name: { type: 'string', description: 'Critter name to remove' },
          },
          required: ['barn', 'name'],
        },
      },
      {
        name: 'read_critter_logs',
        description: 'Read logs from a critter (via journald or custom path)',
        inputSchema: {
          type: 'object',
          properties: {
            barn: { type: 'string', description: 'Barn name' },
            critter: { type: 'string', description: 'Critter name' },
            lines: { type: 'number', description: 'Last N lines (default: 100)' },
            pattern: { type: 'string', description: 'Grep pattern to filter logs (case-insensitive)' },
          },
          required: ['barn', 'critter'],
        },
      },
      {
        name: 'discover_critters',
        description: 'Scan a barn for running services (systemd and Supervisor) and return suggestions for critters to add',
        inputSchema: {
          type: 'object',
          properties: {
            barn: { type: 'string', description: 'Barn name to scan' },
          },
          required: ['barn'],
        },
      },
      // Herd tools
      {
        name: 'list_herds',
        description: 'List all herds in a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
          },
          required: ['project'],
        },
      },
      {
        name: 'get_herd',
        description: 'Get details of a specific herd including its livestock and critters',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            name: { type: 'string', description: 'Herd name' },
          },
          required: ['project', 'name'],
        },
      },
      {
        name: 'create_herd',
        description: 'Create a new herd in a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            name: { type: 'string', description: 'Herd name (e.g., production, staging)' },
          },
          required: ['project', 'name'],
        },
      },
      {
        name: 'delete_herd',
        description: 'Delete a herd from a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            name: { type: 'string', description: 'Herd name to delete' },
          },
          required: ['project', 'name'],
        },
      },
      {
        name: 'add_livestock_to_herd',
        description: 'Add a livestock to a herd. Livestock can only be in one herd.',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            herd: { type: 'string', description: 'Herd name' },
            livestock: { type: 'string', description: 'Livestock name to add' },
          },
          required: ['project', 'herd', 'livestock'],
        },
      },
      {
        name: 'remove_livestock_from_herd',
        description: 'Remove a livestock from a herd',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            herd: { type: 'string', description: 'Herd name' },
            livestock: { type: 'string', description: 'Livestock name to remove' },
          },
          required: ['project', 'herd', 'livestock'],
        },
      },
      {
        name: 'add_critter_to_herd',
        description: 'Add a critter reference to a herd. Critters can be in multiple herds.',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            herd: { type: 'string', description: 'Herd name' },
            barn: { type: 'string', description: 'Barn name where critter lives' },
            critter: { type: 'string', description: 'Critter name to add' },
          },
          required: ['project', 'herd', 'barn', 'critter'],
        },
      },
      {
        name: 'remove_critter_from_herd',
        description: 'Remove a critter reference from a herd',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
            herd: { type: 'string', description: 'Herd name' },
            barn: { type: 'string', description: 'Barn name' },
            critter: { type: 'string', description: 'Critter name to remove' },
          },
          required: ['project', 'herd', 'barn', 'critter'],
        },
      },
      // Ranch Hand tools
      {
        name: 'list_ranchhands',
        description: 'List all ranch hands (IaC providers) for a project',
        inputSchema: {
          type: 'object',
          properties: {
            project: { type: 'string', description: 'Project name' },
          },
          required: ['project'],
        },
      },
      {
        name: 'get_ranchhand',
        description: 'Get details of a specific ranch hand',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Ranch hand name' },
          },
          required: ['name'],
        },
      },
      {
        name: 'create_ranchhand',
        description: 'Create a new ranch hand (IaC provider) for a project',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Ranch hand name' },
            project: { type: 'string', description: 'Project name' },
            type: { type: 'string', description: 'Provider type: kubernetes or terraform' },
            // Kubernetes config
            k8s_context: { type: 'string', description: 'Kubernetes context name (for kubernetes type)' },
            k8s_kubeconfig_path: { type: 'string', description: 'Path to kubeconfig file (optional, defaults to ~/.kube/config)' },
            k8s_private_registries: { type: 'string', description: 'Comma-separated list of private registries (pods from these = livestock)' },
            // Terraform config
            tf_backend: { type: 'string', description: 'Terraform backend type: s3 or local' },
            tf_bucket: { type: 'string', description: 'S3 bucket name (for s3 backend)' },
            tf_key: { type: 'string', description: 'State file key/path in bucket (for s3 backend)' },
            tf_region: { type: 'string', description: 'AWS region (for s3 backend)' },
            tf_local_path: { type: 'string', description: 'Path to local state file (for local backend)' },
          },
          required: ['name', 'project', 'type'],
        },
      },
      {
        name: 'delete_ranchhand',
        description: 'Delete a ranch hand',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Ranch hand name' },
          },
          required: ['name'],
        },
      },
      {
        name: 'discover_ranchhand_resources',
        description: 'Discover available resources from a ranch hand (namespaces for K8s, resources for Terraform)',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Ranch hand name' },
          },
          required: ['name'],
        },
      },
      {
        name: 'select_ranchhand_herds',
        description: 'Select which herds/namespaces to sync from a ranch hand',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Ranch hand name' },
            herds: { type: 'string', description: 'Comma-separated list of herd/namespace names to sync' },
          },
          required: ['name', 'herds'],
        },
      },
      {
        name: 'sync_ranchhand',
        description: 'Sync resources from a ranch hand into the project',
        inputSchema: {
          type: 'object',
          properties: {
            name: { type: 'string', description: 'Ranch hand name' },
          },
          required: ['name'],
        },
      },
      {
        name: 'assign_ranchhand_resource_to_herd',
        description: 'Assign a Terraform resource to a specific herd (for resources that could not be auto-matched)',
        inputSchema: {
          type: 'object',
          properties: {
            ranchhand: { type: 'string', description: 'Ranch hand name' },
            resource_id: { type: 'string', description: 'Resource ID (e.g., aws_db_instance.postgres)' },
            herd: { type: 'string', description: 'Herd name to assign to' },
          },
          required: ['ranchhand', 'resource_id', 'herd'],
        },
      },
      {
        name: 'get_kubectl_contexts',
        description: 'List available kubectl contexts from kubeconfig',
        inputSchema: {
          type: 'object',
          properties: {
            kubeconfig_path: { type: 'string', description: 'Path to kubeconfig file (optional)' },
          },
        },
      },
      {
        name: 'list_terraform_state_files',
        description: 'List Terraform state files in an S3 bucket',
        inputSchema: {
          type: 'object',
          properties: {
            bucket: { type: 'string', description: 'S3 bucket name' },
            prefix: { type: 'string', description: 'Path prefix to search' },
            region: { type: 'string', description: 'AWS region' },
          },
          required: ['bucket', 'prefix', 'region'],
        },
      },
    ],
  };
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  try {
    switch (name) {
    // Project operations
    case 'list_projects': {
      const projects = loadProjects();
      // Return simplified project data to reduce context usage
      const simplified = projects.map((p) => ({
        name: p.name,
        path: p.path,
        summary: p.summary,
        color: p.color,
        livestock: (p.livestock || []).map((l) => ({
          name: l.name,
          path: l.path,
          barn: l.barn,
        })),
      }));
      return {
        content: [
          {
            type: 'text',
            text: JSON.stringify(simplified, null, 2),
          },
        ],
      };
    }

    case 'get_project': {
      const name = requireString(args as McpArgs, 'name');
      const project = loadProject(name);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${name}` }],
          isError: true,
        };
      }
      // Return project with simplified nested data
      // Use get_wiki_section, get_herd, etc. to fetch full details
      const simplified = {
        name: project.name,
        path: project.path,
        summary: project.summary,
        color: project.color,
        issueProvider: project.issueProvider,
        livestock: (project.livestock || []).map((l) => ({
          name: l.name,
          barn: l.barn || 'local',
        })),
        herds: (project.herds || []).map((h) => h.name),
        wiki: (project.wiki || []).map((s) => s.title),
      };
      return {
        content: [{ type: 'text', text: JSON.stringify(simplified, null, 2) }],
      };
    }

    case 'create_project': {
      const name = requireString(args as McpArgs, 'name');
      const path = requireString(args as McpArgs, 'path');
      const summary = optionalString(args as McpArgs, 'summary');
      const color = optionalString(args as McpArgs, 'color');

      const existing = loadProject(name);
      if (existing) {
        return {
          content: [{ type: 'text', text: `Project already exists: ${name}` }],
          isError: true,
        };
      }
      const project: Project = {
        name,
        path,
        summary,
        color,
        livestock: [],
      };
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Created project: ${project.name}` }],
      };
    }

    case 'update_project': {
      const name = requireString(args as McpArgs, 'name');
      const summary = optionalString(args as McpArgs, 'summary');
      const color = optionalString(args as McpArgs, 'color');
      const path = optionalString(args as McpArgs, 'path');

      const project = loadProject(name);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${name}` }],
          isError: true,
        };
      }
      if (summary !== undefined) project.summary = summary;
      if (color !== undefined) project.color = color;
      if (path !== undefined) project.path = path;
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Updated project: ${project.name}` }],
      };
    }

    case 'delete_project': {
      const projectName = requireString(args as McpArgs, 'name');
      const confirm = requireString(args as McpArgs, 'confirm');
      if (confirm !== projectName) {
        return {
          content: [{ type: 'text', text: `Confirmation does not match. To delete, confirm must equal "${projectName}"` }],
          isError: true,
        };
      }
      const deleted = deleteProject(projectName);
      if (!deleted) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: `Deleted project: ${projectName}` }],
      };
    }

    // Livestock operations
    case 'add_livestock': {
      const projectName = requireString(args as McpArgs, 'project');
      const livestockName = requireString(args as McpArgs, 'name');
      const livestockPath = requireString(args as McpArgs, 'path');
      const barn = optionalString(args as McpArgs, 'barn');
      const repo = optionalString(args as McpArgs, 'repo');
      const branch = optionalString(args as McpArgs, 'branch');
      const log_path = optionalString(args as McpArgs, 'log_path');
      const env_path = optionalString(args as McpArgs, 'env_path');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const livestock: Livestock = {
        name: livestockName,
        path: livestockPath,
        barn,
        repo,
        branch,
        log_path,
        env_path,
      };
      project.livestock = project.livestock || [];
      // Remove existing livestock with same name
      project.livestock = project.livestock.filter((l) => l.name !== livestock.name);
      project.livestock.push(livestock);
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Added livestock '${livestock.name}' to project '${project.name}'` }],
      };
    }

    case 'remove_livestock': {
      const projectName = requireString(args as McpArgs, 'project');
      const livestockName = requireString(args as McpArgs, 'name');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      project.livestock = (project.livestock || []).filter(
        (l) => l.name !== livestockName
      );
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Removed livestock '${livestockName}' from project '${project.name}'` }],
      };
    }

    case 'read_livestock_logs': {
      const projectName = requireString(args as McpArgs, 'project');
      const livestockName = requireString(args as McpArgs, 'livestock');
      const lines = optionalNumber(args as McpArgs, 'lines');
      const pattern = optionalString(args as McpArgs, 'pattern');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const livestock = (project.livestock || []).find(l => l.name === livestockName);
      if (!livestock) {
        return {
          content: [{ type: 'text', text: `Livestock not found: ${livestockName}` }],
          isError: true,
        };
      }
      const result = await readLivestockLogs(livestock, { lines, pattern });
      if (result.error) {
        return {
          content: [{ type: 'text', text: result.error }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: result.content }],
      };
    }

    case 'read_livestock_env': {
      const projectName = requireString(args as McpArgs, 'project');
      const livestockName = requireString(args as McpArgs, 'livestock');
      const showValues = optionalBoolean(args as McpArgs, 'show_values', false);

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const livestock = (project.livestock || []).find(l => l.name === livestockName);
      if (!livestock) {
        return {
          content: [{ type: 'text', text: `Livestock not found: ${livestockName}` }],
          isError: true,
        };
      }
      const result = await readLivestockEnv(livestock, showValues);
      if (result.error) {
        return {
          content: [{ type: 'text', text: result.error }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: result.content }],
      };
    }

    // Barn operations
    case 'list_barns': {
      const barns = loadBarns();
      // Return simplified barn data - just identifiers and connection info
      // Use get_barn to see critters deployed to a specific barn
      const simplified = barns.map((b) => ({
        name: b.name,
        host: b.host,
        user: b.user,
        port: b.port,
        critters: (b.critters || []).map((c) => c.name),
      }));
      return {
        content: [{ type: 'text', text: JSON.stringify(simplified, null, 2) }],
      };
    }

    case 'get_barn': {
      const barnName = requireString(args as McpArgs, 'name');
      const barn = loadBarn(barnName);
      if (!barn) {
        return {
          content: [{ type: 'text', text: `Barn not found: ${barnName}` }],
          isError: true,
        };
      }
      // Return barn details with simplified references to deployed resources
      // Use get_project to get full details about a specific project/livestock
      const livestock = getLivestockForBarn(barn.name);
      const simplifiedLivestock = livestock.map((l) => ({
        project: l.project.name,
        livestock: l.livestock.name,
      }));
      const result = {
        name: barn.name,
        host: barn.host,
        user: barn.user,
        port: barn.port,
        identity_file: barn.identity_file,
        critters: (barn.critters || []).map((c) => ({
          name: c.name,
          service: c.service,
        })),
        deployedLivestock: simplifiedLivestock,
      };
      return {
        content: [{ type: 'text', text: JSON.stringify(result, null, 2) }],
      };
    }

    case 'create_barn': {
      const barnName = requireString(args as McpArgs, 'name');
      const host = requireString(args as McpArgs, 'host');
      const user = requireString(args as McpArgs, 'user');
      const port = optionalNumber(args as McpArgs, 'port') ?? 22;
      const identity_file = requireString(args as McpArgs, 'identity_file');

      // Cannot create a barn named 'local' - it's reserved
      if (barnName === 'local') {
        return {
          content: [{ type: 'text', text: `Cannot create barn named 'local': this name is reserved for the local machine` }],
          isError: true,
        };
      }
      const existing = loadBarn(barnName);
      if (existing) {
        return {
          content: [{ type: 'text', text: `Barn already exists: ${barnName}` }],
          isError: true,
        };
      }
      const barn: Barn = {
        name: barnName,
        host,
        user,
        port,
        identity_file,
        critters: [],
      };
      saveBarn(barn);
      return {
        content: [{ type: 'text', text: `Created barn: ${barn.name} (${barn.user}@${barn.host})` }],
      };
    }

    case 'update_barn': {
      const barnName = requireString(args as McpArgs, 'name');
      const host = optionalString(args as McpArgs, 'host');
      const user = optionalString(args as McpArgs, 'user');
      const port = optionalNumber(args as McpArgs, 'port');
      const identity_file = optionalString(args as McpArgs, 'identity_file');

      // Cannot update the local barn
      if (barnName === 'local') {
        return {
          content: [{ type: 'text', text: `Cannot update 'local' barn: it represents the local machine` }],
          isError: true,
        };
      }
      const barn = loadBarn(barnName);
      if (!barn) {
        return {
          content: [{ type: 'text', text: `Barn not found: ${barnName}` }],
          isError: true,
        };
      }
      if (host !== undefined) barn.host = host;
      if (user !== undefined) barn.user = user;
      if (port !== undefined) barn.port = port;
      if (identity_file !== undefined) barn.identity_file = identity_file;
      saveBarn(barn);
      return {
        content: [{ type: 'text', text: `Updated barn: ${barn.name}` }],
      };
    }

    case 'delete_barn': {
      const barnName = requireString(args as McpArgs, 'name');
      const confirm = requireString(args as McpArgs, 'confirm');

      // Cannot delete the local barn
      if (barnName === 'local') {
        return {
          content: [{ type: 'text', text: `Cannot delete 'local' barn: it represents the local machine and is always available` }],
          isError: true,
        };
      }
      if (confirm !== barnName) {
        return {
          content: [{ type: 'text', text: `Confirmation does not match. To delete, confirm must equal "${barnName}"` }],
          isError: true,
        };
      }
      // Check if any livestock references this barn
      const livestock = getLivestockForBarn(barnName);
      if (livestock.length > 0) {
        const refs = livestock.map((l) => `${l.project.name}/${l.livestock.name}`).join(', ');
        return {
          content: [{ type: 'text', text: `Cannot delete barn: still referenced by livestock: ${refs}` }],
          isError: true,
        };
      }
      const deleted = deleteBarn(barnName);
      if (!deleted) {
        return {
          content: [{ type: 'text', text: `Barn not found: ${barnName}` }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: `Deleted barn: ${barnName}` }],
      };
    }

    // Wiki operations
    case 'get_wiki': {
      const projectName = requireString(args as McpArgs, 'project');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      // Return only section titles to reduce context usage
      const titles = (project.wiki || []).map((s) => ({ title: s.title }));
      return {
        content: [{ type: 'text', text: JSON.stringify(titles, null, 2) }],
      };
    }

    case 'get_wiki_section': {
      const projectName = requireString(args as McpArgs, 'project');
      const title = requireString(args as McpArgs, 'title');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const section = (project.wiki || []).find((s) => s.title === title);
      if (!section) {
        return {
          content: [{ type: 'text', text: `Wiki section not found: ${title}` }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: JSON.stringify(section, null, 2) }],
      };
    }

    case 'add_wiki_section': {
      const projectName = requireString(args as McpArgs, 'project');
      const title = requireString(args as McpArgs, 'title');
      const content = requireString(args as McpArgs, 'content');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }

      // Check if wiki provider is read-only (Linear)
      if (isWikiReadOnly(project)) {
        return {
          content: [{ type: 'text', text: `Cannot add wiki section: Wiki is configured to use Linear Projects (read-only). Edit wiki sections directly in Linear.` }],
          isError: true,
        };
      }

      const section: WikiSection = { title, content };
      project.wiki = project.wiki || [];
      // Check if section with this title already exists
      const existingIdx = project.wiki.findIndex((s) => s.title === section.title);
      if (existingIdx >= 0) {
        return {
          content: [{ type: 'text', text: `Wiki section already exists: ${section.title}. Use update_wiki_section to modify.` }],
          isError: true,
        };
      }
      project.wiki.push(section);
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Added wiki section '${section.title}' to project '${project.name}'` }],
      };
    }

    case 'update_wiki_section': {
      const projectName = requireString(args as McpArgs, 'project');
      const title = requireString(args as McpArgs, 'title');
      const newTitle = optionalString(args as McpArgs, 'new_title');
      const content = optionalString(args as McpArgs, 'content');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }

      // Check if wiki provider is read-only (Linear)
      if (isWikiReadOnly(project)) {
        return {
          content: [{ type: 'text', text: `Cannot update wiki section: Wiki is configured to use Linear Projects (read-only). Edit wiki sections directly in Linear.` }],
          isError: true,
        };
      }

      project.wiki = project.wiki || [];
      const sectionIdx = project.wiki.findIndex((s) => s.title === title);
      if (sectionIdx < 0) {
        return {
          content: [{ type: 'text', text: `Wiki section not found: ${title}` }],
          isError: true,
        };
      }
      if (newTitle !== undefined) {
        project.wiki[sectionIdx].title = newTitle;
      }
      if (content !== undefined) {
        project.wiki[sectionIdx].content = content;
      }
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Updated wiki section '${title}' in project '${project.name}'` }],
      };
    }

    case 'delete_wiki_section': {
      const projectName = requireString(args as McpArgs, 'project');
      const title = requireString(args as McpArgs, 'title');

      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }

      // Check if wiki provider is read-only (Linear)
      if (isWikiReadOnly(project)) {
        return {
          content: [{ type: 'text', text: `Cannot delete wiki section: Wiki is configured to use Linear Projects (read-only). Edit wiki sections directly in Linear.` }],
          isError: true,
        };
      }

      project.wiki = project.wiki || [];
      const originalLength = project.wiki.length;
      project.wiki = project.wiki.filter((s) => s.title !== title);
      if (project.wiki.length === originalLength) {
        return {
          content: [{ type: 'text', text: `Wiki section not found: ${title}` }],
          isError: true,
        };
      }
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Deleted wiki section '${title}' from project '${project.name}'` }],
      };
    }

    // Critter operations
    case 'add_critter': {
      const barnName = requireString(args as McpArgs, 'barn');
      const critterName = requireString(args as McpArgs, 'name');
      const service = requireString(args as McpArgs, 'service');
      const config_path = optionalString(args as McpArgs, 'config_path');
      const log_path = optionalString(args as McpArgs, 'log_path');
      const use_journald = optionalBoolean(args as McpArgs, 'use_journald', true);

      const critter: Critter = {
        name: critterName,
        service,
        config_path,
        log_path,
        use_journald,
      };

      try {
        addCritterToBarn(barnName, critter);
        return {
          content: [{ type: 'text', text: `Added critter '${critterName}' (${service}) to barn '${barnName}'` }],
        };
      } catch (err) {
        return {
          content: [{ type: 'text', text: err instanceof Error ? err.message : String(err) }],
          isError: true,
        };
      }
    }

    case 'remove_critter': {
      const barnName = requireString(args as McpArgs, 'barn');
      const critterName = requireString(args as McpArgs, 'name');

      try {
        const removed = removeCritterFromBarn(barnName, critterName);
        if (!removed) {
          return {
            content: [{ type: 'text', text: `Critter '${critterName}' not found on barn '${barnName}'` }],
            isError: true,
          };
        }
        return {
          content: [{ type: 'text', text: `Removed critter '${critterName}' from barn '${barnName}'` }],
        };
      } catch (err) {
        return {
          content: [{ type: 'text', text: err instanceof Error ? err.message : String(err) }],
          isError: true,
        };
      }
    }

    case 'read_critter_logs': {
      const barnName = requireString(args as McpArgs, 'barn');
      const critterName = requireString(args as McpArgs, 'critter');
      const lines = optionalNumber(args as McpArgs, 'lines');
      const pattern = optionalString(args as McpArgs, 'pattern');

      const barn = loadBarn(barnName);
      if (!barn) {
        return {
          content: [{ type: 'text', text: `Barn not found: ${barnName}` }],
          isError: true,
        };
      }

      const critter = getCritter(barnName, critterName);
      if (!critter) {
        return {
          content: [{ type: 'text', text: `Critter '${critterName}' not found on barn '${barnName}'` }],
          isError: true,
        };
      }

      const result = await readCritterLogs(critter, barn, { lines, pattern });
      if (result.error) {
        return {
          content: [{ type: 'text', text: result.error }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: result.content }],
      };
    }

    case 'discover_critters': {
      const barnName = requireString(args as McpArgs, 'barn');

      const barn = loadBarn(barnName);
      if (!barn) {
        return {
          content: [{ type: 'text', text: `Barn not found: ${barnName}` }],
          isError: true,
        };
      }

      const result = await discoverCritters(barn);
      if (result.error) {
        return {
          content: [{ type: 'text', text: result.error }],
          isError: true,
        };
      }

      if (result.critters.length === 0) {
        return {
          content: [{ type: 'text', text: 'No interesting services discovered on this barn' }],
        };
      }

      return {
        content: [{ type: 'text', text: JSON.stringify(result.critters, null, 2) }],
      };
    }

    // Herd operations
    case 'list_herds': {
      const projectName = requireString(args as McpArgs, 'project');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const herds = project.herds || [];
      if (herds.length === 0) {
        return {
          content: [{ type: 'text', text: `No herds in project '${projectName}'` }],
        };
      }
      const herdList = herds.map((h) =>
        `- ${h.name}: ${h.livestock.length} livestock, ${h.critters.length} critters`
      ).join('\n');
      return {
        content: [{ type: 'text', text: `Herds in '${projectName}':\n${herdList}` }],
      };
    }

    case 'get_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'name');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const herd = (project.herds || []).find((h) => h.name === herdName);
      if (!herd) {
        return {
          content: [{ type: 'text', text: `Herd not found: ${herdName}` }],
          isError: true,
        };
      }
      const livestockList = herd.livestock.length > 0
        ? herd.livestock.map((l) => `  - ${l}`).join('\n')
        : '  (none)';
      const critterList = herd.critters.length > 0
        ? herd.critters.map((c) => `  - ${c.critter} (${c.barn})`).join('\n')
        : '  (none)';
      return {
        content: [{ type: 'text', text: `Herd: ${herd.name}\n\nLivestock:\n${livestockList}\n\nCritters:\n${critterList}` }],
      };
    }

    case 'create_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'name');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const herds = project.herds || [];
      if (herds.some((h) => h.name === herdName)) {
        return {
          content: [{ type: 'text', text: `Herd already exists: ${herdName}` }],
          isError: true,
        };
      }
      const newHerd: Herd = {
        name: herdName,
        livestock: [],
        critters: [],
        connections: [],
      };
      project.herds = [...herds, newHerd];
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Created herd '${herdName}' in project '${projectName}'` }],
      };
    }

    case 'delete_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'name');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const herds = project.herds || [];
      if (!herds.some((h) => h.name === herdName)) {
        return {
          content: [{ type: 'text', text: `Herd not found: ${herdName}` }],
          isError: true,
        };
      }
      project.herds = herds.filter((h) => h.name !== herdName);
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Deleted herd '${herdName}' from project '${projectName}'` }],
      };
    }

    case 'add_livestock_to_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'herd');
      const livestockName = requireString(args as McpArgs, 'livestock');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      // Check livestock exists
      const livestock = (project.livestock || []).find((l) => l.name === livestockName);
      if (!livestock) {
        return {
          content: [{ type: 'text', text: `Livestock not found: ${livestockName}` }],
          isError: true,
        };
      }
      // Check herd exists
      const herds = project.herds || [];
      const herdIndex = herds.findIndex((h) => h.name === herdName);
      if (herdIndex === -1) {
        return {
          content: [{ type: 'text', text: `Herd not found: ${herdName}` }],
          isError: true,
        };
      }
      // Check livestock not already in any herd
      for (const h of herds) {
        if (h.livestock.includes(livestockName)) {
          return {
            content: [{ type: 'text', text: `Livestock '${livestockName}' is already in herd '${h.name}'` }],
            isError: true,
          };
        }
      }
      // Add livestock to herd
      herds[herdIndex].livestock.push(livestockName);
      project.herds = herds;
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Added livestock '${livestockName}' to herd '${herdName}'` }],
      };
    }

    case 'remove_livestock_from_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'herd');
      const livestockName = requireString(args as McpArgs, 'livestock');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const herds = project.herds || [];
      const herdIndex = herds.findIndex((h) => h.name === herdName);
      if (herdIndex === -1) {
        return {
          content: [{ type: 'text', text: `Herd not found: ${herdName}` }],
          isError: true,
        };
      }
      if (!herds[herdIndex].livestock.includes(livestockName)) {
        return {
          content: [{ type: 'text', text: `Livestock '${livestockName}' is not in herd '${herdName}'` }],
          isError: true,
        };
      }
      herds[herdIndex].livestock = herds[herdIndex].livestock.filter((l) => l !== livestockName);
      project.herds = herds;
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Removed livestock '${livestockName}' from herd '${herdName}'` }],
      };
    }

    case 'add_critter_to_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'herd');
      const barnName = requireString(args as McpArgs, 'barn');
      const critterName = requireString(args as McpArgs, 'critter');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      // Check barn and critter exist
      const barn = loadBarn(barnName);
      if (!barn) {
        return {
          content: [{ type: 'text', text: `Barn not found: ${barnName}` }],
          isError: true,
        };
      }
      const critter = (barn.critters || []).find((c) => c.name === critterName);
      if (!critter) {
        return {
          content: [{ type: 'text', text: `Critter '${critterName}' not found on barn '${barnName}'` }],
          isError: true,
        };
      }
      // Check herd exists
      const herds = project.herds || [];
      const herdIndex = herds.findIndex((h) => h.name === herdName);
      if (herdIndex === -1) {
        return {
          content: [{ type: 'text', text: `Herd not found: ${herdName}` }],
          isError: true,
        };
      }
      // Check critter not already in this herd
      const alreadyInHerd = herds[herdIndex].critters.some(
        (c) => c.barn === barnName && c.critter === critterName
      );
      if (alreadyInHerd) {
        return {
          content: [{ type: 'text', text: `Critter '${critterName}' (${barnName}) is already in herd '${herdName}'` }],
          isError: true,
        };
      }
      // Add critter reference
      herds[herdIndex].critters.push({ barn: barnName, critter: critterName });
      project.herds = herds;
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Added critter '${critterName}' (${barnName}) to herd '${herdName}'` }],
      };
    }

    case 'remove_critter_from_herd': {
      const projectName = requireString(args as McpArgs, 'project');
      const herdName = requireString(args as McpArgs, 'herd');
      const barnName = requireString(args as McpArgs, 'barn');
      const critterName = requireString(args as McpArgs, 'critter');
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }
      const herds = project.herds || [];
      const herdIndex = herds.findIndex((h) => h.name === herdName);
      if (herdIndex === -1) {
        return {
          content: [{ type: 'text', text: `Herd not found: ${herdName}` }],
          isError: true,
        };
      }
      const critterIndex = herds[herdIndex].critters.findIndex(
        (c) => c.barn === barnName && c.critter === critterName
      );
      if (critterIndex === -1) {
        return {
          content: [{ type: 'text', text: `Critter '${critterName}' (${barnName}) is not in herd '${herdName}'` }],
          isError: true,
        };
      }
      herds[herdIndex].critters.splice(critterIndex, 1);
      project.herds = herds;
      saveProject(project);
      return {
        content: [{ type: 'text', text: `Removed critter '${critterName}' (${barnName}) from herd '${herdName}'` }],
      };
    }

    // Ranch Hand operations
    case 'list_ranchhands': {
      const projectName = requireString(args as McpArgs, 'project');
      const ranchhands = loadRanchHandsForProject(projectName);
      return {
        content: [{
          type: 'text',
          text: JSON.stringify(ranchhands.map(rh => ({
            name: rh.name,
            type: rh.type,
            herd: rh.herd,
            last_sync: rh.last_sync,
          })), null, 2),
        }],
      };
    }

    case 'get_ranchhand': {
      const rhName = requireString(args as McpArgs, 'name');
      const ranchhand = loadRanchHand(rhName);
      if (!ranchhand) {
        return {
          content: [{ type: 'text', text: `Ranch hand not found: ${rhName}` }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: JSON.stringify(ranchhand, null, 2) }],
      };
    }

    case 'create_ranchhand': {
      const rhName = requireString(args as McpArgs, 'name');
      const projectName = requireString(args as McpArgs, 'project');
      const rhType = requireString(args as McpArgs, 'type');

      // Validate project exists
      const project = loadProject(projectName);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${projectName}` }],
          isError: true,
        };
      }

      // Check for duplicate
      const existing = loadRanchHand(rhName);
      if (existing) {
        return {
          content: [{ type: 'text', text: `Ranch hand already exists: ${rhName}` }],
          isError: true,
        };
      }

      let config: KubernetesConfig | TerraformConfig;

      if (rhType === 'kubernetes') {
        const context = requireString(args as McpArgs, 'k8s_context');
        const kubeconfigPath = optionalString(args as McpArgs, 'k8s_kubeconfig_path');
        const registriesStr = optionalString(args as McpArgs, 'k8s_private_registries') || '';
        const privateRegistries = registriesStr.split(',').map(r => r.trim()).filter(Boolean);

        config = {
          context,
          kubeconfig_path: kubeconfigPath,
          private_registries: privateRegistries,
        } as KubernetesConfig;
      } else if (rhType === 'terraform') {
        const backend = requireString(args as McpArgs, 'tf_backend') as 's3' | 'local';

        if (backend === 's3') {
          config = {
            backend: 's3',
            bucket: requireString(args as McpArgs, 'tf_bucket'),
            key: requireString(args as McpArgs, 'tf_key'),
            region: requireString(args as McpArgs, 'tf_region'),
          } as TerraformConfig;
        } else {
          config = {
            backend: 'local',
            local_path: requireString(args as McpArgs, 'tf_local_path'),
          } as TerraformConfig;
        }
      } else {
        return {
          content: [{ type: 'text', text: `Invalid ranch hand type: ${rhType}. Must be 'kubernetes' or 'terraform'` }],
          isError: true,
        };
      }

      const ranchhand: RanchHand = {
        name: rhName,
        project: projectName,
        type: rhType as 'kubernetes' | 'terraform',
        config,
        sync_settings: {
          auto_sync: false,
        },
        herd: '',  // Will be set when user assigns to herd
        resource_mappings: [],
      };

      saveRanchHand(ranchhand);
      return {
        content: [{ type: 'text', text: `Created ranch hand '${rhName}' (${rhType}) for project '${projectName}'` }],
      };
    }

    case 'delete_ranchhand': {
      const rhName = requireString(args as McpArgs, 'name');
      const deleted = deleteRanchHand(rhName);
      if (!deleted) {
        return {
          content: [{ type: 'text', text: `Ranch hand not found: ${rhName}` }],
          isError: true,
        };
      }
      return {
        content: [{ type: 'text', text: `Deleted ranch hand '${rhName}'` }],
      };
    }

    case 'discover_ranchhand_resources': {
      const rhName = requireString(args as McpArgs, 'name');
      const ranchhand = loadRanchHand(rhName);
      if (!ranchhand) {
        return {
          content: [{ type: 'text', text: `Ranch hand not found: ${rhName}` }],
          isError: true,
        };
      }

      if (ranchhand.type === 'kubernetes') {
        const config = ranchhand.config as KubernetesConfig;
        const discovery = discoverK8sResources(config);
        return {
          content: [{
            type: 'text',
            text: JSON.stringify({
              type: 'kubernetes',
              namespaces: discovery.namespaces,
              nodes: discovery.nodes,
            }, null, 2),
          }],
        };
      } else {
        const config = ranchhand.config as TerraformConfig;
        // Get existing herds for matching suggestions
        const project = loadProject(ranchhand.project);
        const existingHerds = (project?.herds || []).map(h => h.name);
        const discovery = discoverTerraformResources(config, existingHerds);
        return {
          content: [{
            type: 'text',
            text: JSON.stringify({
              type: 'terraform',
              resources: discovery.resources,
            }, null, 2),
          }],
        };
      }
    }

    case 'select_ranchhand_herds': {
      const rhName = requireString(args as McpArgs, 'name');
      const herdsStr = requireString(args as McpArgs, 'herds');
      const selectedHerds = herdsStr.split(',').map(h => h.trim()).filter(Boolean);

      const ranchhand = loadRanchHand(rhName);
      if (!ranchhand) {
        return {
          content: [{ type: 'text', text: `Ranch hand not found: ${rhName}` }],
          isError: true,
        };
      }

      ranchhand.herd = selectedHerds[0] || '';  // Take first one (single herd per ranch hand)
      saveRanchHand(ranchhand);
      return {
        content: [{ type: 'text', text: `Updated ranch hand '${rhName}' to sync herd: ${ranchhand.herd || '(none)'}` }],
      };
    }

    case 'sync_ranchhand': {
      const rhName = requireString(args as McpArgs, 'name');
      const ranchhand = loadRanchHand(rhName);
      if (!ranchhand) {
        return {
          content: [{ type: 'text', text: `Ranch hand not found: ${rhName}` }],
          isError: true,
        };
      }

      if (!ranchhand.herd) {
        return {
          content: [{ type: 'text', text: `Ranch hand '${rhName}' has no herd assigned. Use select_ranchhand_herds first.` }],
          isError: true,
        };
      }

      const project = loadProject(ranchhand.project);
      if (!project) {
        return {
          content: [{ type: 'text', text: `Project not found: ${ranchhand.project}` }],
          isError: true,
        };
      }

      let syncSummary: string;

      if (ranchhand.type === 'kubernetes') {
        const result = syncK8sResources(ranchhand);

        // Save barns
        for (const barn of result.barns) {
          // Check if barn already exists
          const existingBarn = loadBarn(barn.name);
          if (!existingBarn) {
            saveBarn(barn);
          }
        }

        // Add livestock to project
        const existingLivestockNames = (project.livestock || []).map(l => l.name);
        for (const ls of result.livestock) {
          if (!existingLivestockNames.includes(ls.name)) {
            project.livestock = project.livestock || [];
            project.livestock.push(ls);
          }
        }

        // Add/update herds
        project.herds = project.herds || [];
        for (const herd of result.herds) {
          const existingHerdIndex = project.herds.findIndex(h => h.name === herd.name);
          if (existingHerdIndex === -1) {
            project.herds.push(herd);
          } else {
            // Merge livestock and critters
            const existingHerd = project.herds[existingHerdIndex];
            for (const lsName of herd.livestock) {
              if (!existingHerd.livestock.includes(lsName)) {
                existingHerd.livestock.push(lsName);
              }
            }
            for (const crRef of herd.critters) {
              if (!existingHerd.critters.some(c => c.critter === crRef.critter && c.barn === crRef.barn)) {
                existingHerd.critters.push(crRef);
              }
            }
          }
        }

        saveProject(project);
        updateRanchHandLastSync(rhName);

        syncSummary = `Synced from K8s: ${result.barns.length} barns, ${result.livestock.length} livestock, ${result.critters.length} critters, ${result.herds.length} herds`;
      } else {
        const result = syncTerraformResources(ranchhand);

        // Save barns
        for (const barn of result.barns) {
          const existingBarn = loadBarn(barn.name);
          if (!existingBarn) {
            saveBarn(barn);
          }
        }

        // For Terraform critters, we need to add them to barns or handle differently
        // Since TF critters don't live on traditional barns, we'll track them separately
        // For now, add them to a synthetic "terraform" barn
        if (result.critters.length > 0) {
          let tfBarn = loadBarn('terraform-managed');
          if (!tfBarn) {
            tfBarn = {
              name: 'terraform-managed',
              source: `ranchhand:${rhName}`,
              connection_type: 'terraform',
              connectable: false,
              critters: [],
            };
          }
          for (const critter of result.critters) {
            if (!tfBarn.critters?.some(c => c.name === critter.name)) {
              tfBarn.critters = tfBarn.critters || [];
              tfBarn.critters.push(critter);
            }
          }
          saveBarn(tfBarn);
        }

        saveProject(project);
        updateRanchHandLastSync(rhName);

        syncSummary = `Synced from Terraform: ${result.barns.length} barns, ${result.critters.length} critters`;
      }

      return {
        content: [{ type: 'text', text: syncSummary }],
      };
    }

    case 'assign_ranchhand_resource_to_herd': {
      const rhName = requireString(args as McpArgs, 'ranchhand');
      const resourceId = requireString(args as McpArgs, 'resource_id');
      const herdName = requireString(args as McpArgs, 'herd');

      addRanchHandResourceMapping(rhName, resourceId, herdName);
      return {
        content: [{ type: 'text', text: `Assigned resource '${resourceId}' to herd '${herdName}'` }],
      };
    }

    case 'get_kubectl_contexts': {
      const kubeconfigPath = optionalString(args as McpArgs, 'kubeconfig_path');
      const contexts = getKubectlContexts(kubeconfigPath);
      const current = getCurrentKubectlContext(kubeconfigPath);
      return {
        content: [{
          type: 'text',
          text: JSON.stringify({ contexts, current }, null, 2),
        }],
      };
    }

    case 'list_terraform_state_files': {
      const bucket = requireString(args as McpArgs, 'bucket');
      const prefix = requireString(args as McpArgs, 'prefix');
      const region = requireString(args as McpArgs, 'region');

      const files = listS3StateFiles(bucket, prefix, region);
      return {
        content: [{
          type: 'text',
          text: JSON.stringify({ bucket, prefix, files }, null, 2),
        }],
      };
    }

    default:
      return {
        content: [{ type: 'text', text: `Unknown tool: ${name}` }],
        isError: true,
      };
    }
  } catch (err) {
    // Validation errors from requireString etc.
    return {
      content: [{ type: 'text', text: err instanceof Error ? err.message : String(err) }],
      isError: true,
    };
  }
});

// ============================================================================
// Start server
// ============================================================================

async function main() {
  ensureConfigDirs();
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error('Yeehaw MCP server running');
}

main().catch(console.error);
