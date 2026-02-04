// src/lib/issues/linear.ts
import type {
  Issue,
  IssueComment,
  FetchIssuesOptions,
  LinearProviderInterface,
  LinearTeam,
  LinearCycle,
  LinearAssignee,
  LinearStateType,
  LinearPriority,
} from './types.js';
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

const VIEWER_QUERY = `
  query {
    viewer {
      id
      name
      displayName
    }
  }
`;

const CYCLES_QUERY = `
  query($teamId: String!) {
    team(id: $teamId) {
      cycles(first: 50) {
        nodes {
          id
          name
          number
          startsAt
          endsAt
        }
      }
      activeCycle {
        id
        name
        number
      }
    }
  }
`;

const ASSIGNEES_QUERY = `
  query($teamId: String!) {
    team(id: $teamId) {
      members(first: 100) {
        nodes {
          id
          name
          displayName
        }
      }
    }
  }
`;

const ISSUES_QUERY = `
  query($teamId: String!, $first: Int, $filter: IssueFilter) {
    team(id: $teamId) {
      issues(first: $first, filter: $filter, orderBy: updatedAt) {
        nodes {
          id
          identifier
          title
          description
          url
          createdAt
          updatedAt
          priority
          estimate
          state {
            name
            type
          }
          creator {
            name
          }
          assignee {
            id
            name
            displayName
          }
          cycle {
            id
            name
            number
          }
          labels {
            nodes {
              name
            }
          }
          comments {
            nodes {
              id
              body
              createdAt
              user {
                name
              }
            }
          }
        }
      }
    }
  }
`;

const ISSUE_QUERY = `
  query($id: String!) {
    issue(id: $id) {
      id
      identifier
      title
      description
      url
      createdAt
      updatedAt
      priority
      estimate
      state {
        name
        type
      }
      creator {
        name
      }
      assignee {
        id
        name
        displayName
      }
      cycle {
        id
        name
        number
      }
      labels {
        nodes {
          name
        }
      }
      comments {
        nodes {
          id
          body
          createdAt
          user {
            name
          }
        }
      }
      team {
        name
      }
    }
  }
`;

interface LinearIssueNode {
  id: string;
  identifier: string;
  title: string;
  description: string | null;
  url: string;
  createdAt: string;
  updatedAt: string;
  priority: number;
  estimate: number | null;
  state: { name: string; type: string };
  creator: { name: string } | null;
  assignee: { id: string; name: string; displayName: string } | null;
  cycle: { id: string; name: string; number: number } | null;
  labels: { nodes: Array<{ name: string }> };
  comments: { nodes: Array<{ id: string; body: string; createdAt: string; user: { name: string } | null }> };
  team?: { name: string };
}

interface LinearCycleNode {
  id: string;
  name: string;
  number: number;
  startsAt?: string;
  endsAt?: string;
}

export class LinearProvider implements LinearProviderInterface {
  readonly type = 'linear' as const;
  private teamId?: string;
  private teamName?: string;
  private cachedUserId?: string | null;
  private cachedActiveCycleId?: string;

  constructor(teamId?: string, teamName?: string) {
    this.teamId = teamId;
    this.teamName = teamName;
  }

  async isAuthenticated(): Promise<boolean> {
    return isLinearAuthenticated();
  }

  async authenticate(): Promise<void> {
    throw new Error('Use saveLinearApiKey() to authenticate with a Personal API Key');
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

  async fetchTeamName(): Promise<string | undefined> {
    if (this.teamName) return this.teamName;
    if (!this.teamId) return undefined;

    try {
      const data = await linearGraphQL<{ team: { name: string } }>(
        `query($teamId: String!) { team(id: $teamId) { name } }`,
        { teamId: this.teamId }
      );
      this.teamName = data.team.name;
      return this.teamName;
    } catch {
      return undefined;
    }
  }

  async getCurrentUserId(): Promise<string | null> {
    if (this.cachedUserId !== undefined) {
      return this.cachedUserId;
    }

    try {
      const data = await linearGraphQL<{ viewer: { id: string } }>(VIEWER_QUERY);
      this.cachedUserId = data.viewer.id;
      return this.cachedUserId;
    } catch {
      this.cachedUserId = null;
      return null;
    }
  }

  async fetchCycles(): Promise<LinearCycle[]> {
    if (!this.teamId) {
      throw new Error('Team not selected');
    }

    const data = await linearGraphQL<{
      team: {
        cycles: { nodes: LinearCycleNode[] };
        activeCycle: LinearCycleNode | null;
      };
    }>(CYCLES_QUERY, { teamId: this.teamId });

    // Cache the active cycle ID
    if (data.team.activeCycle) {
      this.cachedActiveCycleId = data.team.activeCycle.id;
    }

    return data.team.cycles.nodes.map((c) => ({
      id: c.id,
      name: c.name || `Cycle ${c.number}`,
      number: c.number,
    }));
  }

  getActiveCycleId(): string | undefined {
    return this.cachedActiveCycleId;
  }

  async fetchAssignees(): Promise<LinearAssignee[]> {
    if (!this.teamId) {
      throw new Error('Team not selected');
    }

    const data = await linearGraphQL<{
      team: {
        members: { nodes: Array<{ id: string; name: string; displayName: string }> };
      };
    }>(ASSIGNEES_QUERY, { teamId: this.teamId });

    return data.team.members.nodes;
  }

  async fetchIssues(options: FetchIssuesOptions = {}): Promise<Issue[]> {
    if (!this.teamId) {
      throw new Error('Team not selected');
    }

    const { state = 'open', limit = 50, linearFilter } = options;

    // Build filter object
    const filter: Record<string, unknown> = {};

    // State filter
    if (state === 'open') {
      filter.state = { type: { in: ['backlog', 'unstarted', 'started'] } };
    } else if (state === 'closed') {
      filter.state = { type: { in: ['completed', 'canceled'] } };
    }

    // Linear-specific filters
    if (linearFilter) {
      // Assignee filter - use isMe for current user (more reliable than comparing IDs)
      if (linearFilter.assigneeIsMe) {
        filter.assignee = { isMe: { eq: true } };
      } else if (linearFilter.assigneeId !== undefined) {
        if (linearFilter.assigneeId === null) {
          filter.assignee = { null: true };
        } else {
          filter.assignee = { id: { eq: linearFilter.assigneeId } };
        }
      }

      // Cycle filter
      if (linearFilter.cycleId) {
        filter.cycle = { id: { eq: linearFilter.cycleId } };
      }

      // State type filter (overrides basic state filter)
      if (linearFilter.stateType) {
        const types = Array.isArray(linearFilter.stateType)
          ? linearFilter.stateType
          : [linearFilter.stateType];
        filter.state = { type: { in: types } };
      }
    }

    const data = await linearGraphQL<{ team: { issues: { nodes: LinearIssueNode[] } } }>(
      ISSUES_QUERY,
      { teamId: this.teamId, first: limit, filter: Object.keys(filter).length > 0 ? filter : undefined }
    );

    let issues = data.team.issues.nodes.map((issue) => this.normalizeIssue(issue));

    // Client-side sorting
    if (linearFilter?.sortBy === 'priority') {
      // Priority: 1 = urgent (highest), 4 = low, 0 = no priority (lowest)
      issues = issues.sort((a, b) => {
        const aPri = a.priority === 0 ? 5 : (a.priority ?? 5);
        const bPri = b.priority === 0 ? 5 : (b.priority ?? 5);
        return linearFilter.sortDirection === 'desc' ? aPri - bPri : bPri - aPri;
      });
    } else if (linearFilter?.sortBy === 'createdAt') {
      issues = issues.sort((a, b) =>
        new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );
    }
    // Default: already sorted by updatedAt from API

    return issues;
  }

  async getIssue(id: string): Promise<Issue> {
    const data = await linearGraphQL<{ issue: LinearIssueNode }>(ISSUE_QUERY, { id });
    return this.normalizeIssue(data.issue);
  }

  private normalizeIssue(issue: LinearIssueNode): Issue {
    const openStateTypes = ['backlog', 'unstarted', 'started'];
    const isOpen = openStateTypes.includes(issue.state.type);

    const comments: IssueComment[] = issue.comments.nodes.map((c) => ({
      id: c.id,
      author: c.user?.name || 'Unknown',
      body: c.body,
      createdAt: c.createdAt,
    }));

    return {
      id: issue.id,
      identifier: issue.identifier,
      title: issue.title,
      state: issue.state.name,
      stateType: issue.state.type as LinearStateType,
      isOpen,
      author: issue.creator?.name || 'Unknown',
      body: issue.description || '',
      labels: issue.labels.nodes.map((l) => l.name),
      url: issue.url,
      createdAt: issue.createdAt,
      updatedAt: issue.updatedAt,
      comments,
      source: {
        type: 'linear',
        team: issue.team?.name || this.teamName || 'Unknown',
      },
      priority: issue.priority as LinearPriority,
      estimate: issue.estimate ?? undefined,
      assignee: issue.assignee
        ? {
            id: issue.assignee.id,
            name: issue.assignee.name,
            displayName: issue.assignee.displayName,
          }
        : undefined,
      cycle: issue.cycle
        ? {
            id: issue.cycle.id,
            name: issue.cycle.name || `Cycle ${issue.cycle.number}`,
            number: issue.cycle.number,
          }
        : undefined,
    };
  }
}
