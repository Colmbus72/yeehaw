// src/lib/issues/types.ts

export interface IssueComment {
  id: string;
  author: string;
  body: string;
  createdAt: string;
}

// Linear priority levels (0 = no priority, 1 = urgent, 2 = high, 3 = medium, 4 = low)
export type LinearPriority = 0 | 1 | 2 | 3 | 4;

// Linear state types for visual mapping
export type LinearStateType = 'backlog' | 'unstarted' | 'started' | 'completed' | 'canceled' | 'triage';

export interface LinearAssignee {
  id: string;
  name: string;
  displayName: string;
}

export interface LinearCycle {
  id: string;
  name: string;
  number: number;
}

export interface Issue {
  id: string;
  identifier: string; // "#123" for GitHub, "ENG-123" for Linear
  title: string;
  state: string; // Original state ("open", "In Progress", "Done", etc.)
  stateType?: LinearStateType; // For visual mapping
  isOpen: boolean; // For filtering logic
  author: string;
  body: string;
  labels: string[];
  url: string;
  createdAt: string;
  updatedAt: string;
  comments: IssueComment[];
  source: IssueSource;
  // Linear-specific fields
  priority?: LinearPriority;
  estimate?: number; // Points
  assignee?: LinearAssignee;
  cycle?: LinearCycle;
}

export type IssueSource =
  | { type: 'github'; repo: string; livestockName: string; livestockPath: string }
  | { type: 'linear'; team: string };

export interface LinearIssueFilter {
  assigneeId?: string | null; // null = unassigned, undefined = any, string = specific user
  assigneeIsMe?: boolean; // true = filter by current user using Linear's isMe filter
  cycleId?: string;
  stateType?: LinearStateType | LinearStateType[];
  sortBy?: 'priority' | 'updatedAt' | 'createdAt';
  sortDirection?: 'asc' | 'desc';
}

export interface FetchIssuesOptions {
  state?: 'open' | 'closed' | 'all';
  limit?: number;
  // Linear-specific filtering
  linearFilter?: LinearIssueFilter;
}

export interface IssueProvider {
  readonly type: 'github' | 'linear';

  isAuthenticated(): Promise<boolean>;
  authenticate(): Promise<void>;
  fetchIssues(options?: FetchIssuesOptions): Promise<Issue[]>;
  getIssue(id: string): Promise<Issue>;
}

// Linear-specific extension for team selection
export interface LinearProviderInterface extends IssueProvider {
  readonly type: 'linear';
  needsTeamSelection(): boolean;
  fetchTeams(): Promise<LinearTeam[]>;
  setTeamId(teamId: string): void;
  setTeamName(teamName: string): void;
  getTeamName(): string | undefined;
  fetchCycles(): Promise<LinearCycle[]>;
  fetchAssignees(): Promise<LinearAssignee[]>;
  getCurrentUserId(): Promise<string | null>;
}

export interface LinearTeam {
  id: string;
  name: string;
  key: string; // e.g., "ENG", "PROD"
}
