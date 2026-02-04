/**
 * Terraform Ranch Hand Provider
 *
 * Reads Terraform state and syncs resources into Yeehaw:
 * - aws_db_instance → Critter
 * - aws_elasticache_cluster → Critter
 * - aws_instance → Barn (if used as servers)
 * - aws_mq_broker → Critter
 * - aws_opensearch_domain → Critter
 */

import { execSync } from 'child_process';
import { readFileSync, existsSync } from 'fs';
import { shellEscape } from './shell.js';
import type {
  RanchHand,
  TerraformConfig,
  Critter,
  Barn,
  TerraformCritterMetadata,
  EntitySource,
} from '../types.js';

// ============================================================================
// Types for Terraform State
// ============================================================================

interface TerraformState {
  version: number;
  terraform_version: string;
  resources: TerraformResource[];
}

interface TerraformResource {
  mode: 'managed' | 'data';
  type: string;
  name: string;
  provider: string;
  instances: TerraformInstance[];
}

interface TerraformInstance {
  attributes: Record<string, unknown>;
}

// ============================================================================
// Discovery Results
// ============================================================================

export interface TerraformDiscoveryResult {
  resources: TerraformResourceInfo[];
}

export interface TerraformResourceInfo {
  id: string;           // unique identifier (type.name)
  type: string;         // resource type (e.g., aws_db_instance)
  name: string;         // resource name
  displayName: string;  // human-readable name
  endpoint?: string;    // connection endpoint if available
  port?: number;        // port if available
  suggestedHerd?: string;  // suggested herd based on naming
  yeehawType: 'critter' | 'barn' | 'skip';  // what this maps to
}

export interface TerraformSyncResult {
  critters: Critter[];
  barns: Barn[];
}

// ============================================================================
// Resource Type Mappings
// ============================================================================

interface ResourceMapping {
  yeehawType: 'critter' | 'barn' | 'skip';
  service: string;
  getEndpoint: (attrs: Record<string, unknown>) => string | undefined;
  getPort: (attrs: Record<string, unknown>) => number | undefined;
}

const RESOURCE_MAPPINGS: Record<string, ResourceMapping> = {
  'aws_db_instance': {
    yeehawType: 'critter',
    service: 'postgresql', // Default, could be mysql based on engine
    getEndpoint: (attrs) => attrs.endpoint as string | undefined,
    getPort: (attrs) => attrs.port as number | undefined,
  },
  'aws_rds_cluster': {
    yeehawType: 'critter',
    service: 'aurora',
    getEndpoint: (attrs) => attrs.endpoint as string | undefined,
    getPort: (attrs) => attrs.port as number | undefined,
  },
  'aws_elasticache_cluster': {
    yeehawType: 'critter',
    service: 'redis',
    getEndpoint: (attrs) => {
      const nodes = attrs.cache_nodes as Array<{ address: string }> | undefined;
      return nodes?.[0]?.address;
    },
    getPort: (attrs) => attrs.port as number | undefined,
  },
  'aws_elasticache_replication_group': {
    yeehawType: 'critter',
    service: 'redis',
    getEndpoint: (attrs) => attrs.primary_endpoint_address as string | undefined,
    getPort: (attrs) => attrs.port as number | undefined,
  },
  'aws_mq_broker': {
    yeehawType: 'critter',
    service: 'rabbitmq',
    getEndpoint: (attrs) => {
      const instances = attrs.instances as Array<{ endpoints: string[] }> | undefined;
      return instances?.[0]?.endpoints?.[0];
    },
    getPort: () => 5672,
  },
  'aws_opensearch_domain': {
    yeehawType: 'critter',
    service: 'opensearch',
    getEndpoint: (attrs) => attrs.endpoint as string | undefined,
    getPort: () => 443,
  },
  'aws_elasticsearch_domain': {
    yeehawType: 'critter',
    service: 'elasticsearch',
    getEndpoint: (attrs) => attrs.endpoint as string | undefined,
    getPort: () => 443,
  },
  'aws_instance': {
    yeehawType: 'barn',
    service: 'ec2',
    getEndpoint: (attrs) => attrs.public_ip as string | undefined || attrs.private_ip as string | undefined,
    getPort: () => 22,
  },
};

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Fetch Terraform state from S3
 */
function fetchStateFromS3(
  bucket: string,
  key: string,
  region: string
): TerraformState {
  const s3Path = shellEscape(`s3://${bucket}/${key}`);
  const cmd = `aws s3 cp ${s3Path} - --region ${shellEscape(region)}`;

  try {
    const result = execSync(cmd, {
      encoding: 'utf-8',
      maxBuffer: 50 * 1024 * 1024, // 50MB buffer
    });
    return JSON.parse(result) as TerraformState;
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    throw new Error(`Failed to fetch Terraform state from S3: ${msg}`);
  }
}

/**
 * Read Terraform state from local file
 */
function readStateFromFile(path: string): TerraformState {
  if (!existsSync(path)) {
    throw new Error(`Terraform state file not found: ${path}`);
  }

  const content = readFileSync(path, 'utf-8');
  return JSON.parse(content) as TerraformState;
}

/**
 * Suggest a herd based on resource naming patterns
 */
function suggestHerd(
  resourceName: string,
  resourceType: string,
  attrs: Record<string, unknown>,
  existingHerds: string[]
): string | undefined {
  // Check common patterns in resource name
  const nameLower = resourceName.toLowerCase();
  const identifier = (attrs.identifier as string || '').toLowerCase();
  const tags = attrs.tags as Record<string, string> | undefined;

  // Priority 1: Check tags for environment
  if (tags?.environment) {
    const envTag = tags.environment.toLowerCase();
    const match = existingHerds.find(h => h.toLowerCase() === envTag);
    if (match) return match;
  }

  // Priority 2: Check for common environment keywords in name/identifier
  const searchText = `${nameLower} ${identifier}`;
  const patterns = [
    { keywords: ['prod', 'production'], herd: 'production' },
    { keywords: ['staging', 'stage'], herd: 'staging' },
    { keywords: ['dev', 'develop', 'development'], herd: 'development' },
    { keywords: ['test', 'testing'], herd: 'testing' },
  ];

  for (const pattern of patterns) {
    for (const keyword of pattern.keywords) {
      if (searchText.includes(keyword)) {
        // Try to match to existing herd
        const match = existingHerds.find(
          h => h.toLowerCase().includes(keyword) || h.toLowerCase() === pattern.herd
        );
        if (match) return match;
        // Fall back to suggested name
        return pattern.herd;
      }
    }
  }

  return undefined;
}

/**
 * Derive service type from resource attributes
 */
function deriveServiceType(
  resourceType: string,
  attrs: Record<string, unknown>
): string {
  const mapping = RESOURCE_MAPPINGS[resourceType];
  if (!mapping) return 'unknown';

  // Special case: RDS can be PostgreSQL or MySQL
  if (resourceType === 'aws_db_instance') {
    const engine = (attrs.engine as string || '').toLowerCase();
    if (engine.includes('mysql') || engine.includes('mariadb')) {
      return 'mysql';
    }
    if (engine.includes('postgres')) {
      return 'postgresql';
    }
    return engine || 'database';
  }

  return mapping.service;
}

// ============================================================================
// Discovery Functions
// ============================================================================

/**
 * Discover resources from Terraform state
 */
export function discoverTerraformResources(
  config: TerraformConfig,
  existingHerds: string[] = []
): TerraformDiscoveryResult {
  // Load state based on backend type
  let state: TerraformState;
  if (config.backend === 's3') {
    if (!config.bucket || !config.key || !config.region) {
      throw new Error('S3 backend requires bucket, key, and region');
    }
    state = fetchStateFromS3(config.bucket, config.key, config.region);
  } else {
    if (!config.local_path) {
      throw new Error('Local backend requires local_path');
    }
    state = readStateFromFile(config.local_path);
  }

  const resources: TerraformResourceInfo[] = [];

  for (const resource of state.resources) {
    // Skip data sources
    if (resource.mode !== 'managed') continue;

    const mapping = RESOURCE_MAPPINGS[resource.type];
    if (!mapping) continue;

    for (const instance of resource.instances) {
      const attrs = instance.attributes;
      const id = `${resource.type}.${resource.name}`;

      // Get display name from identifier or tags
      const identifier = attrs.identifier as string | undefined;
      const tags = attrs.tags as Record<string, string> | undefined;
      const displayName = identifier || tags?.Name || resource.name;

      resources.push({
        id,
        type: resource.type,
        name: resource.name,
        displayName,
        endpoint: mapping.getEndpoint(attrs),
        port: mapping.getPort(attrs),
        suggestedHerd: suggestHerd(resource.name, resource.type, attrs, existingHerds),
        yeehawType: mapping.yeehawType,
      });
    }
  }

  return { resources };
}

// ============================================================================
// Sync Functions
// ============================================================================

/**
 * Sync resources from Terraform state based on ranch hand configuration
 */
export function syncTerraformResources(
  ranchhand: RanchHand
): TerraformSyncResult {
  const config = ranchhand.config as TerraformConfig;
  const sourceTag: EntitySource = `ranchhand:${ranchhand.name}`;

  // Load state
  let state: TerraformState;
  if (config.backend === 's3') {
    if (!config.bucket || !config.key || !config.region) {
      throw new Error('S3 backend requires bucket, key, and region');
    }
    state = fetchStateFromS3(config.bucket, config.key, config.region);
  } else {
    if (!config.local_path) {
      throw new Error('Local backend requires local_path');
    }
    state = readStateFromFile(config.local_path);
  }

  const critters: Critter[] = [];
  const barns: Barn[] = [];

  for (const resource of state.resources) {
    if (resource.mode !== 'managed') continue;

    const mapping = RESOURCE_MAPPINGS[resource.type];
    if (!mapping) continue;

    for (const instance of resource.instances) {
      const attrs = instance.attributes;
      const id = `${resource.type}.${resource.name}`;

      // Check if this resource has a herd mapping (user assigned or auto-matched)
      const resourceMapping = ranchhand.resource_mappings.find(m => m.resource_id === id);

      // Only sync resources that have been assigned to the ranch hand's herd
      if (!resourceMapping || resourceMapping.herd_name !== ranchhand.herd) {
        continue;
      }

      // Get display name
      const identifier = attrs.identifier as string | undefined;
      const tags = attrs.tags as Record<string, string> | undefined;
      const displayName = identifier || tags?.Name || resource.name;

      if (mapping.yeehawType === 'critter') {
        const tfMetadata: TerraformCritterMetadata = {
          resource_type: resource.type,
          resource_name: resource.name,
        };

        critters.push({
          name: displayName,
          service: deriveServiceType(resource.type, attrs),
          source: sourceTag,
          endpoint: mapping.getEndpoint(attrs),
          port: mapping.getPort(attrs),
          tf_metadata: tfMetadata,
        });
      } else if (mapping.yeehawType === 'barn') {
        const endpoint = mapping.getEndpoint(attrs);
        barns.push({
          name: displayName,
          host: endpoint,
          source: sourceTag,
          connection_type: 'terraform',
          connectable: !!endpoint, // Connectable if we have an IP
          critters: [],
        });
      }
    }
  }

  return { critters, barns };
}

/**
 * Test S3 access for Terraform state
 */
export function testS3Access(bucket: string, key: string, region: string): boolean {
  try {
    const s3Path = shellEscape(`s3://${bucket}/${key}`);
    const cmd = `aws s3 ls ${s3Path} --region ${shellEscape(region)}`;
    execSync(cmd, { encoding: 'utf-8' });
    return true;
  } catch {
    return false;
  }
}

/**
 * List available Terraform state files in an S3 bucket prefix
 */
export function listS3StateFiles(
  bucket: string,
  prefix: string,
  region: string
): string[] {
  try {
    const s3Path = shellEscape(`s3://${bucket}/${prefix}`);
    const cmd = `aws s3 ls ${s3Path} --recursive --region ${shellEscape(region)}`;
    const result = execSync(cmd, { encoding: 'utf-8' });

    return result
      .split('\n')
      .filter(line => line.includes('.tfstate') && !line.includes('.tfstate.backup'))
      .map(line => {
        const parts = line.trim().split(/\s+/);
        return parts[parts.length - 1]; // The key is the last part
      })
      .filter(Boolean);
  } catch {
    return [];
  }
}
