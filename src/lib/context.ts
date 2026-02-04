import { loadProject } from './config.js';
import type { Project } from '../types.js';

/**
 * Build a context string to inject into Claude sessions spawned from Yeehaw.
 * Includes project name and wiki section titles (not content) to hint at available context.
 */
export function buildProjectContext(projectName: string): string | null {
  const project = loadProject(projectName);
  if (!project) {
    return null;
  }

  const lines: string[] = [];

  lines.push(`You are working on the "${project.name}" project.`);

  if (project.summary) {
    lines.push(`Project: ${project.summary}`);
  }

  // Add wiki section titles as hints
  const wikiSections = project.wiki || [];
  if (wikiSections.length > 0) {
    lines.push('');
    lines.push('Yeehaw wiki sections available:');
    for (const section of wikiSections) {
      lines.push(`- ${section.title}`);
    }
    lines.push('');
    lines.push('Use mcp__yeehaw__get_wiki_section to fetch relevant context before making architectural decisions.');
  }

  return lines.join('\n');
}

/**
 * Build context for a livestock-specific session.
 * Includes project context plus livestock details.
 */
export function buildLivestockContext(projectName: string, livestockName: string): string | null {
  const project = loadProject(projectName);
  if (!project) {
    return null;
  }

  const livestock = project.livestock?.find(l => l.name === livestockName);
  if (!livestock) {
    return buildProjectContext(projectName);
  }

  const lines: string[] = [];

  lines.push(`You are working on the "${project.name}" project, in the "${livestock.name}" environment.`);

  if (project.summary) {
    lines.push(`Project: ${project.summary}`);
  }

  // Add livestock details
  if (livestock.branch) {
    lines.push(`Branch: ${livestock.branch}`);
  }

  // Add wiki section titles as hints
  const wikiSections = project.wiki || [];
  if (wikiSections.length > 0) {
    lines.push('');
    lines.push('Yeehaw wiki sections available:');
    for (const section of wikiSections) {
      lines.push(`- ${section.title}`);
    }
    lines.push('');
    lines.push('Use mcp__yeehaw__get_wiki_section to fetch relevant context before making architectural decisions.');
  }

  return lines.join('\n');
}
