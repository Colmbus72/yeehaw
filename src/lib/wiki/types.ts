// src/lib/wiki/types.ts

export interface WikiSection {
  title: string;
  content: string;
  // Linear-specific metadata (optional)
  id?: string;
  updatedAt?: string;
}

export interface WikiProvider {
  readonly type: 'local' | 'linear';
  readonly isReadOnly: boolean;

  fetchSections(): Promise<WikiSection[]>;
  getSection(title: string): Promise<WikiSection | null>;
}

// Local provider extension with write operations
export interface LocalWikiProviderInterface extends WikiProvider {
  readonly type: 'local';
  readonly isReadOnly: false;

  addSection(section: WikiSection): Promise<void>;
  updateSection(oldTitle: string, section: WikiSection): Promise<void>;
  deleteSection(title: string): Promise<void>;
}

// Linear-specific extension for team selection
export interface LinearWikiProviderInterface extends WikiProvider {
  readonly type: 'linear';
  readonly isReadOnly: true;

  isAuthenticated(): Promise<boolean>;
  needsTeamSelection(): boolean;
  fetchTeams(): Promise<LinearTeam[]>;
  setTeamId(teamId: string): void;
  setTeamName(teamName: string): void;
  getTeamName(): string | undefined;
}

export interface LinearTeam {
  id: string;
  name: string;
  key: string;
}
