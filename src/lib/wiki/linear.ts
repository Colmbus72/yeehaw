// src/lib/wiki/linear.ts
import type { WikiSection, LinearWikiProviderInterface, LinearTeam } from './types.js';
import { isLinearAuthenticated, linearGraphQL } from '../auth/index.js';

// GraphQL queries
const TEAMS_QUERY = `
  query {
    teams {
      nodes {
        id
        name
        key
      }
    }
  }
`;

const PROJECTS_QUERY = `
  query($teamId: String!) {
    team(id: $teamId) {
      projects(first: 100) {
        nodes {
          id
          name
          content
          updatedAt
        }
      }
    }
  }
`;

const PROJECT_QUERY = `
  query($id: String!) {
    project(id: $id) {
      id
      name
      content
      updatedAt
    }
  }
`;

interface LinearProjectNode {
  id: string;
  name: string;
  content: string | null;
  updatedAt: string;
}

export class LinearWikiProvider implements LinearWikiProviderInterface {
  readonly type = 'linear' as const;
  readonly isReadOnly = true as const;

  private teamId?: string;
  private teamName?: string;

  constructor(teamId?: string, teamName?: string) {
    this.teamId = teamId;
    this.teamName = teamName;
  }

  async isAuthenticated(): Promise<boolean> {
    return isLinearAuthenticated();
  }

  needsTeamSelection(): boolean {
    return !this.teamId;
  }

  async fetchTeams(): Promise<LinearTeam[]> {
    const data = await linearGraphQL<{ teams: { nodes: LinearTeam[] } }>(TEAMS_QUERY);
    return data.teams.nodes;
  }

  setTeamId(teamId: string): void {
    this.teamId = teamId;
  }

  setTeamName(teamName: string): void {
    this.teamName = teamName;
  }

  getTeamName(): string | undefined {
    return this.teamName;
  }

  async fetchSections(): Promise<WikiSection[]> {
    if (!this.teamId) {
      throw new Error('Team not selected');
    }

    const data = await linearGraphQL<{ team: { projects: { nodes: LinearProjectNode[] } } }>(
      PROJECTS_QUERY,
      { teamId: this.teamId }
    );

    return data.team.projects.nodes.map((project) => this.normalizeProject(project));
  }

  async getSection(title: string): Promise<WikiSection | null> {
    // For Linear, we fetch all sections and find by title
    // since Linear Projects are identified by ID, not title
    const sections = await this.fetchSections();
    return sections.find((s) => s.title === title) || null;
  }

  private normalizeProject(project: LinearProjectNode): WikiSection {
    return {
      title: project.name,
      content: project.content || '',
      id: project.id,
      updatedAt: project.updatedAt,
    };
  }
}
