// src/lib/wiki/local.ts
import type { Project } from '../../types.js';
import type { WikiSection, LocalWikiProviderInterface } from './types.js';

export class LocalWikiProvider implements LocalWikiProviderInterface {
  readonly type = 'local' as const;
  readonly isReadOnly = false as const;

  private project: Project;
  private onUpdate: (project: Project) => void;

  constructor(project: Project, onUpdate: (project: Project) => void) {
    this.project = project;
    this.onUpdate = onUpdate;
  }

  async fetchSections(): Promise<WikiSection[]> {
    return this.project.wiki || [];
  }

  async getSection(title: string): Promise<WikiSection | null> {
    const sections = this.project.wiki || [];
    return sections.find((s) => s.title === title) || null;
  }

  async addSection(section: WikiSection): Promise<void> {
    const wiki = [...(this.project.wiki || [])];

    // Check for duplicate title
    if (wiki.some((s) => s.title === section.title)) {
      throw new Error(`Wiki section already exists: ${section.title}`);
    }

    wiki.push(section);
    this.project = { ...this.project, wiki };
    this.onUpdate(this.project);
  }

  async updateSection(oldTitle: string, section: WikiSection): Promise<void> {
    const wiki = [...(this.project.wiki || [])];
    const index = wiki.findIndex((s) => s.title === oldTitle);

    if (index === -1) {
      throw new Error(`Wiki section not found: ${oldTitle}`);
    }

    // Check for duplicate title if title changed
    if (oldTitle !== section.title && wiki.some((s) => s.title === section.title)) {
      throw new Error(`Wiki section already exists: ${section.title}`);
    }

    wiki[index] = section;
    this.project = { ...this.project, wiki };
    this.onUpdate(this.project);
  }

  async deleteSection(title: string): Promise<void> {
    const wiki = (this.project.wiki || []).filter((s) => s.title !== title);
    this.project = { ...this.project, wiki };
    this.onUpdate(this.project);
  }

  /**
   * Update the internal project reference (called when project changes externally)
   */
  updateProject(project: Project): void {
    this.project = project;
  }
}
