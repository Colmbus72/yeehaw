/**
 * Ranch Hand Detection Library
 *
 * Scans directories to auto-detect Terraform and Kubernetes configurations.
 * Leans on existing CLI tooling and minimal parsing for simplicity.
 */

import { readdirSync, readFileSync, existsSync, statSync } from 'fs';
import { join, relative } from 'path';

export interface DetectedTerraformEnv {
  id: string;           // relative path like "platform4/base"
  absolutePath: string;
  backendType: 's3' | 'local';
  bucket?: string;
  key?: string;
  region?: string;
  localPath?: string;
}

/**
 * Recursively find all directories containing .tf files
 */
function findTerraformDirs(baseDir: string, maxDepth = 5): string[] {
  const results: string[] = [];

  function scan(dir: string, depth: number) {
    if (depth > maxDepth) return;

    try {
      const entries = readdirSync(dir, { withFileTypes: true });
      const hasTfFiles = entries.some(e => e.isFile() && e.name.endsWith('.tf'));

      if (hasTfFiles) {
        results.push(dir);
      }

      for (const entry of entries) {
        if (entry.isDirectory() && !entry.name.startsWith('.') && entry.name !== 'node_modules') {
          scan(join(dir, entry.name), depth + 1);
        }
      }
    } catch {
      // Permission denied or other error, skip
    }
  }

  scan(baseDir, 0);
  return results;
}

/**
 * Parse backend configuration from .tf files in a directory
 */
function parseBackendFromTfFiles(dir: string): Partial<DetectedTerraformEnv> | null {
  try {
    const files = readdirSync(dir).filter(f => f.endsWith('.tf'));

    for (const file of files) {
      const content = readFileSync(join(dir, file), 'utf-8');

      // Look for backend "s3" block - handle both direct and nested in terraform {} block
      // Use a more flexible regex that captures until we see a closing brace followed by newline or end
      const s3Match = content.match(/backend\s+"s3"\s*\{([\s\S]*?)\n\s*\}/);
      if (s3Match) {
        const block = s3Match[1];
        // Handle both quoted and unquoted values (some use variables)
        const bucket = block.match(/bucket\s*=\s*"([^"]+)"/)?.[1];
        const key = block.match(/key\s*=\s*"([^"]+)"/)?.[1];
        const region = block.match(/region\s*=\s*"([^"]+)"/)?.[1];

        // Return even if we can't parse all values (they might use variables)
        return { backendType: 's3', bucket, key, region };
      }

      // Look for backend "local" block
      const localMatch = content.match(/backend\s+"local"\s*\{([\s\S]*?)\n\s*\}/);
      if (localMatch) {
        const block = localMatch[1];
        const path = block.match(/path\s*=\s*"([^"]+)"/)?.[1];
        return { backendType: 'local', localPath: path };
      }

      // Also check for simple backend "s3" {} or backend "local" {} (empty config, uses env vars)
      if (content.match(/backend\s+"s3"\s*\{/)) {
        return { backendType: 's3' };
      }
      if (content.match(/backend\s+"local"\s*\{/)) {
        return { backendType: 'local' };
      }
    }
  } catch {
    // Error reading files
  }

  return null;
}

/**
 * Check for resolved backend in .terraform directory
 * This is the preferred source as it contains the actual resolved config
 */
function parseResolvedBackend(dir: string): Partial<DetectedTerraformEnv> | null {
  const statePath = join(dir, '.terraform', 'terraform.tfstate');

  if (!existsSync(statePath)) return null;

  try {
    const content = readFileSync(statePath, 'utf-8');
    const state = JSON.parse(content);

    if (state.backend?.type === 's3') {
      const config = state.backend.config || {};
      return {
        backendType: 's3',
        bucket: config.bucket,
        key: config.key,
        region: config.region,
      };
    }

    if (state.backend?.type === 'local') {
      return {
        backendType: 'local',
        localPath: state.backend.config?.path,
      };
    }
  } catch {
    // Invalid JSON or other error
  }

  return null;
}

/**
 * Detect all Terraform environments in a directory
 *
 * Scans recursively for Terraform configurations and extracts backend info
 * from either resolved .terraform state or parsing .tf files.
 */
export function detectTerraformEnvironments(directory: string): DetectedTerraformEnv[] {
  if (!existsSync(directory) || !statSync(directory).isDirectory()) {
    return [];
  }

  const tfDirs = findTerraformDirs(directory);
  const results: DetectedTerraformEnv[] = [];

  for (const dir of tfDirs) {
    // Prefer resolved backend, fall back to parsing .tf files
    const backend = parseResolvedBackend(dir) || parseBackendFromTfFiles(dir);

    if (backend && backend.backendType) {
      results.push({
        id: relative(directory, dir) || '.',
        absolutePath: dir,
        backendType: backend.backendType,
        bucket: backend.bucket,
        key: backend.key,
        region: backend.region,
        localPath: backend.localPath,
      });
    }
  }

  return results;
}
