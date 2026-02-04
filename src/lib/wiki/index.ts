// src/lib/wiki/index.ts
import type { Project } from '../../types.js';
import type { WikiProvider } from './types.js';
import { LocalWikiProvider } from './local.js';
import { LinearWikiProvider } from './linear.js';

export * from './types.js';
export { LocalWikiProvider } from './local.js';
export { LinearWikiProvider } from './linear.js';

/**
 * Get the appropriate wiki provider for a project.
 * Defaults to local provider if no wiki provider configured.
 *
 * @param project - The project to get the wiki provider for
 * @param onUpdate - Callback for when wiki content changes (required for local provider writes)
 */
export function getWikiProvider(
  project: Project,
  onUpdate?: (project: Project) => void
): WikiProvider {
  const config = project.wikiProvider ?? { type: 'local' };

  switch (config.type) {
    case 'local':
      return new LocalWikiProvider(project, onUpdate || (() => {}));
    case 'linear':
      return new LinearWikiProvider(config.teamId, config.teamName);
    default:
      // Default to local provider
      return new LocalWikiProvider(project, onUpdate || (() => {}));
  }
}

/**
 * Check if a project has wiki enabled.
 * Currently always returns true since we default to local.
 */
export function hasWikiEnabled(project: Project): boolean {
  // Wiki is always enabled (defaults to local)
  return true;
}

/**
 * Check if the wiki provider is read-only.
 */
export function isWikiReadOnly(project: Project): boolean {
  const config = project.wikiProvider ?? { type: 'local' };
  return config.type === 'linear';
}
