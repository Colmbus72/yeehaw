// src/lib/issues/index.ts
import type { Project } from '../../types.js';
import type { IssueProvider } from './types.js';
import { GitHubProvider } from './github.js';
import { LinearProvider } from './linear.js';

export * from './types.js';
export { GitHubProvider } from './github.js';
export { LinearProvider } from './linear.js';

/**
 * Get the appropriate issue provider for a project.
 * Returns null if issue tracking is disabled.
 */
export function getProvider(project: Project): IssueProvider | null {
  const config = project.issueProvider ?? { type: 'github' };

  switch (config.type) {
    case 'github':
      return new GitHubProvider(project.livestock ?? []);
    case 'linear':
      return new LinearProvider(config.teamId, config.teamName);
    case 'none':
      return null;
  }
}

/**
 * Check if a project has issue tracking enabled.
 */
export function hasIssueTracking(project: Project): boolean {
  const config = project.issueProvider ?? { type: 'github' };
  return config.type !== 'none';
}
