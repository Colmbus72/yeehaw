// src/views/IssuesView.tsx
import React, { useState, useEffect, useCallback } from 'react';
import { Box, Text, useInput, useStdout } from 'ink';
import TextInput from 'ink-text-input';
import { join } from 'path';
import { homedir } from 'os';
import { Header } from '../components/Header.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem, type RowAction } from '../components/List.js';
import { ScrollableMarkdown } from '../components/ScrollableMarkdown.js';
import type { Project } from '../types.js';
import {
  getProvider,
  LinearProvider,
  type Issue,
  type LinearTeam,
  type LinearCycle,
  type LinearAssignee,
  type LinearIssueFilter,
  type LinearStateType,
  type LinearPriority,
} from '../lib/issues/index.js';
import { saveProject } from '../lib/config.js';
import { buildProjectContext } from '../lib/context.js';
import { openIssueInBrowser } from '../lib/github.js';
import { saveLinearApiKey, validateLinearApiKey, LINEAR_API_KEY_URL, clearLinearToken } from '../lib/auth/linear.js';

type FocusedPanel = 'list' | 'details';
type ViewState =
  | { type: 'loading' }
  | { type: 'error'; message: string }
  | { type: 'not-authenticated'; providerType: 'github' | 'linear' }
  | { type: 'linear-auth-input' }
  | { type: 'linear-auth-validating' }
  | { type: 'select-team'; teams: LinearTeam[] }
  | { type: 'ready'; issues: Issue[] }
  | { type: 'filter' }
  | { type: 'disabled' };

interface IssuesViewProps {
  project: Project;
  onBack: () => void;
  onOpenClaude?: (workingDir: string, issueContext: string) => void;
}

// Expand ~ in paths to home directory
function expandPath(path: string): string {
  if (path.startsWith('~/')) {
    return join(homedir(), path.slice(2));
  }
  return path;
}

// Status indicator based on Linear state type
function getStatusIndicator(stateType?: LinearStateType): { char: string; color: string } {
  switch (stateType) {
    case 'backlog':
    case 'triage':
      return { char: '░', color: 'gray' };
    case 'unstarted':
      return { char: '░', color: 'gray' };
    case 'started':
      return { char: '▒', color: 'yellow' };
    case 'completed':
      return { char: '█', color: 'blue' };
    case 'canceled':
      return { char: '░', color: 'red' };
    default:
      return { char: ' ', color: 'gray' };
  }
}

// Priority indicator: _ . : ! for low/med/high/urgent
function getPriorityIndicator(priority?: LinearPriority): string {
  switch (priority) {
    case 1: return '!'; // urgent
    case 2: return ':'; // high
    case 3: return '.'; // medium
    case 4: return '_'; // low
    default: return ' '; // no priority
  }
}

// Get initials from name (first letter of first and last name)
function getInitials(name?: string): string {
  if (!name) return '  ';
  const parts = name.trim().split(/\s+/);
  if (parts.length === 1) {
    return parts[0].substring(0, 2).toUpperCase();
  }
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

export function IssuesView({ project, onBack, onOpenClaude }: IssuesViewProps) {
  const { stdout } = useStdout();
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('list');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [viewState, setViewState] = useState<ViewState>({ type: 'loading' });
  const [linearProvider, setLinearProvider] = useState<LinearProvider | null>(null);
  const [apiKeyInput, setApiKeyInput] = useState('');

  // Calculate dynamic height for ScrollableMarkdown based on terminal size
  // Layout: Header(3) + FilterInfo(1) + PanelBorders(2) + PanelTitle(1) + PanelHints(1) + Padding(2) = ~10 lines overhead
  const terminalHeight = stdout?.rows ?? 24;
  const panelContentHeight = Math.max(8, terminalHeight - 10);

  // Filter state for Linear
  const [cycles, setCycles] = useState<LinearCycle[]>([]);
  const [assignees, setAssignees] = useState<LinearAssignee[]>([]);
  const [currentFilter, setCurrentFilter] = useState<LinearIssueFilter>({
    stateType: ['backlog', 'unstarted', 'started'],  // Default to open issues
    sortBy: 'priority',
    sortDirection: 'desc',
  });
  const [filterInitialized, setFilterInitialized] = useState(false);

  // Filter UI state
  const [filterField, setFilterField] = useState<'assignee' | 'cycle' | 'status' | 'sort'>('assignee');
  const [filterSelectedIndex, setFilterSelectedIndex] = useState(0);

  // Team name for display
  const [teamName, setTeamName] = useState<string | undefined>();

  // Cached current user ID for filter
  const [currentUserId, setCurrentUserId] = useState<string | null>(null);

  // Track if user explicitly selected "Me" filter (separate from actual ID)
  // Default to true since we want "assigned to me" as the default filter
  const [filterByMe, setFilterByMe] = useState(true);

  const loadIssues = useCallback(async (filter?: LinearIssueFilter) => {
    setViewState({ type: 'loading' });

    const provider = getProvider(project);

    if (!provider) {
      setViewState({ type: 'disabled' });
      return;
    }

    // Check authentication
    const isAuthed = await provider.isAuthenticated();
    if (!isAuthed) {
      setViewState({ type: 'not-authenticated', providerType: provider.type });
      if (provider.type === 'linear') {
        setLinearProvider(provider as LinearProvider);
      }
      return;
    }

    // For Linear, check if team selection is needed
    if (provider.type === 'linear') {
      const linearProv = provider as LinearProvider;
      setLinearProvider(linearProv);

      if (linearProv.needsTeamSelection()) {
        try {
          const teams = await linearProv.fetchTeams();
          setViewState({ type: 'select-team', teams });
          return;
        } catch (err) {
          setViewState({ type: 'error', message: `Failed to fetch teams: ${err}` });
          return;
        }
      }

      // Set team name from provider (loaded from config)
      setTeamName(linearProv.getTeamName());

      // Initialize default filter (current cycle, assigned to me)
      if (!filterInitialized) {
        try {
          // Fetch cycles, assignees, and user ID in parallel
          const [fetchedCycles, fetchedAssignees, userId] = await Promise.all([
            linearProv.fetchCycles(),
            linearProv.fetchAssignees(),
            linearProv.getCurrentUserId(),
          ]);
          setCycles(fetchedCycles);
          setAssignees(fetchedAssignees);
          setCurrentUserId(userId);

          // Set default filter: current cycle, assigned to me, open issues
          const activeCycleId = linearProv.getActiveCycleId();
          const defaultFilter: LinearIssueFilter = {
            cycleId: activeCycleId,
            assigneeIsMe: true,  // Use Linear's isMe filter for reliability
            stateType: ['backlog', 'unstarted', 'started'],
            sortBy: 'priority',
            sortDirection: 'desc',
          };
          setFilterByMe(true);
          setCurrentFilter(defaultFilter);
          setFilterInitialized(true);
          filter = defaultFilter;
        } catch (err) {
          // Continue without filter if fetch fails
          setFilterInitialized(true);
        }
      }
    }

    // Fetch issues
    try {
      const issues = await provider.fetchIssues({
        linearFilter: filter ?? currentFilter,
      });
      setViewState({ type: 'ready', issues });
    } catch (err) {
      setViewState({ type: 'error', message: `Failed to fetch issues: ${err}` });
    }
  }, [project, currentFilter, filterInitialized]);

  useEffect(() => {
    loadIssues();
  }, []);  // Only run once on mount

  const selectedIssue = viewState.type === 'ready' ? viewState.issues[selectedIndex] : null;

  // Handle API key submission
  const handleApiKeySubmit = async () => {
    if (!apiKeyInput.trim()) return;

    setViewState({ type: 'linear-auth-validating' });

    const isValid = await validateLinearApiKey(apiKeyInput.trim());
    if (isValid) {
      saveLinearApiKey(apiKeyInput.trim());
      setApiKeyInput('');
      loadIssues();
    } else {
      setViewState({ type: 'error', message: 'Invalid API key. Please check and try again.' });
    }
  };

  // Handle team selection
  const selectTeam = async (team: LinearTeam) => {
    if (!linearProvider) return;

    linearProvider.setTeamId(team.id);
    linearProvider.setTeamName(team.name);
    setTeamName(team.name);

    // Save team ID and name to project config
    const updatedProject: Project = {
      ...project,
      issueProvider: { type: 'linear', teamId: team.id, teamName: team.name },
    };
    saveProject(updatedProject);

    // Continue with existing provider (don't call loadIssues which would create new provider from stale prop)
    setViewState({ type: 'loading' });

    try {
      // Fetch cycles, assignees, and user ID
      const [fetchedCycles, fetchedAssignees, userId] = await Promise.all([
        linearProvider.fetchCycles(),
        linearProvider.fetchAssignees(),
        linearProvider.getCurrentUserId(),
      ]);
      setCycles(fetchedCycles);
      setAssignees(fetchedAssignees);
      setCurrentUserId(userId);

      // Set default filter: current cycle, assigned to me, open issues
      const activeCycleId = linearProvider.getActiveCycleId();
      const defaultFilter: LinearIssueFilter = {
        cycleId: activeCycleId,
        assigneeIsMe: true,  // Use Linear's isMe filter for reliability
        stateType: ['backlog', 'unstarted', 'started'],
        sortBy: 'priority',
        sortDirection: 'desc',
      };
      setFilterByMe(true);
      setCurrentFilter(defaultFilter);
      setFilterInitialized(true);

      // Fetch issues
      const issues = await linearProvider.fetchIssues({ linearFilter: defaultFilter });
      setViewState({ type: 'ready', issues });
    } catch (err) {
      setViewState({ type: 'error', message: `Failed to load issues: ${err}` });
    }
  };

  // Build Claude context for an issue (includes project context)
  const buildClaudeContext = (issue: Issue): string => {
    const lines: string[] = [];

    // Include project context first (wiki sections, summary, etc.)
    const projectContext = buildProjectContext(project.name);
    if (projectContext) {
      lines.push(projectContext);
      lines.push('');
      lines.push('---');
      lines.push('');
    }

    lines.push('You are working on the following issue:');
    lines.push('');
    lines.push(`Title: ${issue.title}`);
    lines.push(`Identifier: ${issue.identifier}`);
    lines.push(`State: ${issue.state}`);
    lines.push(`Author: ${issue.author}`);
    lines.push(`URL: ${issue.url}`);

    if (issue.priority !== undefined && issue.priority > 0) {
      const priorityNames = ['', 'Urgent', 'High', 'Medium', 'Low'];
      lines.push(`Priority: ${priorityNames[issue.priority]}`);
    }

    if (issue.assignee) {
      lines.push(`Assignee: ${issue.assignee.name}`);
    }

    if (issue.estimate !== undefined) {
      lines.push(`Points: ${issue.estimate}`);
    }

    lines.push('');
    lines.push('Description:');
    lines.push(issue.body || '(No description provided)');
    lines.push('');

    if (issue.labels.length > 0) {
      lines.push(`Labels: ${issue.labels.join(', ')}`);
      lines.push('');
    }

    if (issue.comments.length > 0) {
      lines.push('---');
      lines.push('Comments:');
      lines.push('');

      for (const comment of issue.comments) {
        const date = new Date(comment.createdAt).toLocaleDateString();
        lines.push(`${comment.author} (${date}):`);
        lines.push(comment.body);
        lines.push('');
        lines.push('---');
      }
    }

    return lines.join('\n');
  };

  // Apply filter and reload
  const applyFilter = (newFilter: LinearIssueFilter) => {
    setCurrentFilter(newFilter);
    setViewState({ type: 'loading' });
    loadIssues(newFilter);
  };

  // Handle input
  useInput((input, key) => {
    // Don't intercept input during text input mode
    if (viewState.type === 'linear-auth-input') {
      if (key.escape) {
        setApiKeyInput('');
        setViewState({ type: 'not-authenticated', providerType: 'linear' });
      }
      return;
    }

    // Filter view navigation
    if (viewState.type === 'filter') {
      if (key.escape) {
        loadIssues(currentFilter);
        return;
      }

      // Tab/Shift+Tab: select current option AND move to next/previous field
      if (key.tab) {
        // First, select the current option (without applying yet)
        selectFilterOptionLocal(filterField, filterSelectedIndex);
        // Then move to next or previous field
        const fields: typeof filterField[] = ['assignee', 'cycle', 'status', 'sort'];
        const currentIndex = fields.indexOf(filterField);
        const nextField = key.shift
          ? fields[(currentIndex - 1 + fields.length) % fields.length]
          : fields[(currentIndex + 1) % fields.length];
        setFilterField(nextField);
        // Set cursor to the currently selected option in the new field
        setFilterSelectedIndex(getCurrentFilterIndex(nextField));
        return;
      }

      // Navigate options with j/k
      if (input === 'j' || key.downArrow) {
        const options = getFilterOptions(filterField);
        setFilterSelectedIndex((prev) => Math.min(prev + 1, options.length - 1));
        return;
      }
      if (input === 'k' || key.upArrow) {
        setFilterSelectedIndex((prev) => Math.max(prev - 1, 0));
        return;
      }

      // Enter: select current option, apply filter, and exit
      if (key.return) {
        const newFilter = selectFilterOptionLocal(filterField, filterSelectedIndex);
        applyFilter(newFilter);
        return;
      }

      return;
    }

    if (key.escape) {
      onBack();
      return;
    }

    // Handle auth prompt - press Enter to start API key input
    if (viewState.type === 'not-authenticated' && viewState.providerType === 'linear') {
      if (key.return) {
        setViewState({ type: 'linear-auth-input' });
      }
      return;
    }

    // Handle error state - 'r' to retry, 'a' to re-authenticate
    if (viewState.type === 'error') {
      if (input === 'r') {
        loadIssues();
        return;
      }
      if (input === 'a') {
        clearLinearToken();
        setViewState({ type: 'linear-auth-input' });
        return;
      }
      return;
    }

    // In ready state
    if (viewState.type !== 'ready') return;

    // Tab to switch focus
    if (key.tab) {
      setFocusedPanel((prev) => (prev === 'list' ? 'details' : 'list'));
      return;
    }

    // Refresh issues
    if (input === 'r') {
      loadIssues(currentFilter);
      return;
    }

    // Open filter (Linear only)
    if (input === 'f' && project.issueProvider?.type === 'linear') {
      setViewState({ type: 'filter' });
      setFilterField('assignee');
      // Start on the currently selected assignee option
      setFilterSelectedIndex(getCurrentFilterIndex('assignee'));
      return;
    }
  });

  // Get filter options for current field
  const getFilterOptions = (field: typeof filterField): Array<{ id: string; label: string }> => {
    switch (field) {
      case 'assignee':
        // Filter out current user from teammates list since "Assigned to me" covers that
        const otherAssignees = currentUserId
          ? assignees.filter((a) => a.id !== currentUserId)
          : assignees;
        return [
          { id: '__me__', label: 'Assigned to me' },
          { id: '__any__', label: 'Anyone' },
          { id: '__none__', label: 'Unassigned' },
          ...otherAssignees.map((a) => ({ id: a.id, label: a.name })),
        ];
      case 'cycle':
        return [
          { id: '__any__', label: 'Any cycle' },
          ...cycles.map((c) => ({ id: c.id, label: c.name })),
        ];
      case 'status':
        return [
          { id: '__open__', label: 'Open (backlog, unstarted, started)' },
          { id: '__all__', label: 'All statuses' },
          { id: 'backlog', label: 'Backlog' },
          { id: 'unstarted', label: 'Todo' },
          { id: 'started', label: 'In Progress' },
          { id: 'completed', label: 'Done' },
          { id: 'canceled', label: 'Canceled' },
        ];
      case 'sort':
        return [
          { id: 'priority_desc', label: 'Priority (high first)' },
          { id: 'priority_asc', label: 'Priority (low first)' },
          { id: 'updatedAt', label: 'Recently updated' },
          { id: 'createdAt', label: 'Recently created' },
        ];
      default:
        return [];
    }
  };

  // Get the index of the currently selected option for a filter field
  const getCurrentFilterIndex = (field: typeof filterField): number => {
    const options = getFilterOptions(field);
    switch (field) {
      case 'assignee':
        if (filterByMe || currentFilter.assigneeIsMe) {
          return options.findIndex((o) => o.id === '__me__');
        } else if (currentFilter.assigneeId === null) {
          return options.findIndex((o) => o.id === '__none__');
        } else if (currentFilter.assigneeId) {
          const idx = options.findIndex((o) => o.id === currentFilter.assigneeId);
          return idx >= 0 ? idx : options.findIndex((o) => o.id === '__any__');
        }
        return options.findIndex((o) => o.id === '__any__');
      case 'cycle':
        if (currentFilter.cycleId) {
          const idx = options.findIndex((o) => o.id === currentFilter.cycleId);
          return idx >= 0 ? idx : 0;
        }
        return 0; // "Any cycle"
      case 'status':
        if (!currentFilter.stateType) {
          return options.findIndex((o) => o.id === '__all__');
        } else if (Array.isArray(currentFilter.stateType) && currentFilter.stateType.length === 3) {
          return options.findIndex((o) => o.id === '__open__');
        } else if (!Array.isArray(currentFilter.stateType)) {
          const idx = options.findIndex((o) => o.id === currentFilter.stateType);
          return idx >= 0 ? idx : 0;
        }
        return 0;
      case 'sort':
        if (currentFilter.sortBy === 'priority') {
          return currentFilter.sortDirection === 'asc'
            ? options.findIndex((o) => o.id === 'priority_asc')
            : options.findIndex((o) => o.id === 'priority_desc');
        } else if (currentFilter.sortBy === 'updatedAt') {
          return options.findIndex((o) => o.id === 'updatedAt');
        } else if (currentFilter.sortBy === 'createdAt') {
          return options.findIndex((o) => o.id === 'createdAt');
        }
        return 0;
      default:
        return 0;
    }
  };

  // Select a filter option locally (without fetching) - used by Tab
  // Returns the new filter for immediate use (since setState is async)
  const selectFilterOptionLocal = (field: typeof filterField, index: number): LinearIssueFilter => {
    const options = getFilterOptions(field);
    const option = options[index];
    if (!option) return currentFilter;

    const newFilter = { ...currentFilter };

    switch (field) {
      case 'assignee':
        if (option.id === '__me__') {
          // Use Linear's isMe filter - more reliable than comparing user IDs
          newFilter.assigneeIsMe = true;
          delete newFilter.assigneeId;
          setFilterByMe(true);
        } else if (option.id === '__any__') {
          delete newFilter.assigneeId;
          delete newFilter.assigneeIsMe;
          setFilterByMe(false);
        } else if (option.id === '__none__') {
          newFilter.assigneeId = null;
          delete newFilter.assigneeIsMe;
          setFilterByMe(false);
        } else {
          newFilter.assigneeId = option.id;
          delete newFilter.assigneeIsMe;
          setFilterByMe(false);
        }
        break;
      case 'cycle':
        if (option.id === '__any__') {
          delete newFilter.cycleId;
        } else {
          newFilter.cycleId = option.id;
        }
        break;
      case 'status':
        if (option.id === '__open__') {
          newFilter.stateType = ['backlog', 'unstarted', 'started'];
        } else if (option.id === '__all__') {
          delete newFilter.stateType;
        } else {
          newFilter.stateType = option.id as LinearStateType;
        }
        break;
      case 'sort':
        if (option.id === 'priority_desc') {
          newFilter.sortBy = 'priority';
          newFilter.sortDirection = 'desc';
        } else if (option.id === 'priority_asc') {
          newFilter.sortBy = 'priority';
          newFilter.sortDirection = 'asc';
        } else {
          newFilter.sortBy = option.id as 'updatedAt' | 'createdAt';
          delete newFilter.sortDirection;
        }
        break;
    }

    setCurrentFilter(newFilter);
    return newFilter;
  };

  // Select a filter option and apply (fetch) - used by Enter
  const selectFilterOption = async (field: typeof filterField, index: number) => {
    const options = getFilterOptions(field);
    const option = options[index];
    if (!option) return;

    const newFilter = { ...currentFilter };

    switch (field) {
      case 'assignee':
        if (option.id === '__me__') {
          // Use Linear's isMe filter - more reliable than comparing user IDs
          newFilter.assigneeIsMe = true;
          delete newFilter.assigneeId;
          setFilterByMe(true);
        } else if (option.id === '__any__') {
          delete newFilter.assigneeId;
          delete newFilter.assigneeIsMe;
          setFilterByMe(false);
        } else if (option.id === '__none__') {
          newFilter.assigneeId = null;
          delete newFilter.assigneeIsMe;
          setFilterByMe(false);
        } else {
          newFilter.assigneeId = option.id;
          delete newFilter.assigneeIsMe;
          setFilterByMe(false);
        }
        break;
      case 'cycle':
        if (option.id === '__any__') {
          delete newFilter.cycleId;
        } else {
          newFilter.cycleId = option.id;
        }
        break;
      case 'status':
        if (option.id === '__open__') {
          newFilter.stateType = ['backlog', 'unstarted', 'started'];
        } else if (option.id === '__all__') {
          delete newFilter.stateType;
        } else {
          newFilter.stateType = option.id as LinearStateType;
        }
        break;
      case 'sort':
        if (option.id === 'priority_desc') {
          newFilter.sortBy = 'priority';
          newFilter.sortDirection = 'desc';
        } else if (option.id === 'priority_asc') {
          newFilter.sortBy = 'priority';
          newFilter.sortDirection = 'asc';
        } else {
          newFilter.sortBy = option.id as 'updatedAt' | 'createdAt';
          delete newFilter.sortDirection;
        }
        break;
    }

    applyFilter(newFilter);
  };

  // Handle opening Claude for an issue (called from row action)
  const handleOpenClaude = (issue: Issue) => {
    if (!onOpenClaude) return;

    let workingDir: string;

    if (issue.source.type === 'github') {
      workingDir = issue.source.livestockPath || project.path;
    } else {
      workingDir = project.path;
    }

    // Expand ~ to home directory and fallback to cwd if empty
    if (workingDir) {
      workingDir = expandPath(workingDir);
    } else {
      workingDir = process.cwd();
    }

    const context = buildClaudeContext(issue);
    onOpenClaude(workingDir, context);
  };

  const truncate = (str: string, maxLen: number) => {
    if (str.length <= maxLen) return str;
    return str.slice(0, maxLen - 1) + '…';
  };

  // Build issue details with better layout
  const buildIssueDetails = (issue: Issue): string => {
    const lines: string[] = [];

    // Metadata grid
    if (issue.source.type === 'github') {
      lines.push(`**Repo:** ${issue.source.repo}  •  **Livestock:** ${issue.source.livestockName}`);
    }

    lines.push(`**State:** ${issue.state}  •  **Author:** ${issue.author}`);

    if (issue.priority !== undefined && issue.priority > 0) {
      const priorityNames = ['', 'Urgent', 'High', 'Medium', 'Low'];
      const priorityLine = `**Priority:** ${priorityNames[issue.priority]}`;
      const pointsLine = issue.estimate !== undefined ? `  •  **Points:** ${issue.estimate}` : '';
      lines.push(priorityLine + pointsLine);
    } else if (issue.estimate !== undefined) {
      lines.push(`**Points:** ${issue.estimate}`);
    }

    if (issue.assignee) {
      lines.push(`**Assignee:** ${issue.assignee.name}`);
    }

    if (issue.cycle) {
      lines.push(`**Cycle:** ${issue.cycle.name}`);
    }

    if (issue.labels.length > 0) {
      lines.push(`**Labels:** ${issue.labels.join(', ')}`);
    }

    lines.push(`**Updated:** ${new Date(issue.updatedAt).toLocaleDateString()}`);
    lines.push('');
    lines.push('---');
    lines.push('');

    if (issue.body) {
      lines.push(issue.body);
    } else {
      lines.push('*No description provided.*');
    }

    // Add comments section
    if (issue.comments.length > 0) {
      lines.push('');
      lines.push('---');
      lines.push('');
      lines.push(`**Comments (${issue.comments.length}):**`);
      lines.push('');

      for (const comment of issue.comments) {
        const date = new Date(comment.createdAt).toLocaleDateString();
        lines.push(`**${comment.author}** - ${date}`);
        lines.push('');
        lines.push(comment.body);
        lines.push('');
        lines.push('---');
        lines.push('');
      }
    }

    return lines.join('\n');
  };

  // Get subtitle for header
  const getSubtitle = (): string => {
    if (project.issueProvider?.type === 'linear' && teamName) {
      return `Issues • ${teamName}`;
    }
    return 'Issues';
  };

  // Loading state
  if (viewState.type === 'loading') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={getSubtitle()} color={project.color} />
        <Box padding={2}>
          <Text>Loading issues...</Text>
        </Box>
      </Box>
    );
  }

  // Disabled state
  if (viewState.type === 'disabled') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Issues" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text dimColor>Issue tracking is disabled for this project.</Text>
          <Box marginTop={1}>
            <Text dimColor>Press 'e' in the project view to enable it.</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Not authenticated - GitHub
  if (viewState.type === 'not-authenticated' && viewState.providerType === 'github') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Issues" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="yellow">GitHub CLI not authenticated</Text>
          <Box marginTop={1}>
            <Text>Run this command in your terminal:</Text>
          </Box>
          <Box marginTop={1}>
            <Text color="cyan">  gh auth login</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Then return here and the issues will load.</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Not authenticated - Linear (prompt to enter key)
  if (viewState.type === 'not-authenticated' && viewState.providerType === 'linear') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Issues" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="yellow">Linear authentication required</Text>
          <Box marginTop={1}>
            <Text>You need a Personal API Key from Linear.</Text>
          </Box>
          <Box marginTop={1}>
            <Text>Create one at: <Text color="cyan">{LINEAR_API_KEY_URL}</Text></Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Press Enter to paste your API key</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Linear API key input
  if (viewState.type === 'linear-auth-input') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Issues" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="yellow">Enter Linear API Key</Text>
          <Box marginTop={1}>
            <Text dimColor>Paste your Personal API Key from Linear:</Text>
          </Box>
          <Box marginTop={1}>
            <Text>API Key: </Text>
            <TextInput
              value={apiKeyInput}
              onChange={setApiKeyInput}
              onSubmit={handleApiKeySubmit}
              mask="*"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: submit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Linear validating
  if (viewState.type === 'linear-auth-validating') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Issues" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text>Validating API key...</Text>
        </Box>
      </Box>
    );
  }

  // Team selection
  if (viewState.type === 'select-team') {
    const teamItems: ListItem[] = viewState.teams.map((team) => ({
      id: team.id,
      label: `${team.name} (${team.key})`,
      status: 'active',
    }));

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Issues" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text bold color="yellow">Select a Linear team</Text>
          <Box marginBottom={1}>
            <Text dimColor>Which team's issues should be shown?</Text>
          </Box>
          <List
            items={teamItems}
            focused={true}
            onSelect={(item) => {
              const team = viewState.teams.find((t) => t.id === item.id);
              if (team) selectTeam(team);
            }}
          />
        </Box>
      </Box>
    );
  }

  // Filter view
  if (viewState.type === 'filter') {
    const fields: Array<{ key: typeof filterField; label: string }> = [
      { key: 'assignee', label: 'Assignee' },
      { key: 'cycle', label: 'Cycle' },
      { key: 'status', label: 'Status' },
      { key: 'sort', label: 'Sort by' },
    ];

    const currentOptions = getFilterOptions(filterField);

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={getSubtitle()} color={project.color} />
        <Box paddingX={2} paddingY={1} flexDirection="column">
          <Text bold>Filter Issues</Text>
          <Box marginTop={1} gap={2}>
            {fields.map((f) => (
              <Text key={f.key} color={filterField === f.key ? 'cyan' : 'gray'}>
                {filterField === f.key ? `[${f.label}]` : f.label}
              </Text>
            ))}
          </Box>
          <Box marginTop={1} flexDirection="column">
            {currentOptions.map((opt, i) => (
              <Text key={opt.id} color={i === filterSelectedIndex ? 'green' : undefined}>
                {i === filterSelectedIndex ? '▸ ' : '  '}{opt.label}
              </Text>
            ))}
          </Box>
          <Box marginTop={2}>
            <Text dimColor>Tab: switch field • j/k: navigate • Enter: select • Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Error state
  if (viewState.type === 'error') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={getSubtitle()} color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="red">Error: {viewState.message}</Text>
          <Box marginTop={1}>
            <Text dimColor>Press 'r' to retry, 'a' to re-authenticate</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ready state - show issues
  if (viewState.type !== 'ready') {
    return null;
  }

  // Row-level actions for issues
  const issueActions: RowAction[] = onOpenClaude
    ? [{ key: 'c', label: 'claude' }, { key: 'o', label: 'open' }]
    : [{ key: 'o', label: 'open' }];

  const isLinear = project.issueProvider?.type === 'linear';

  const issueItems: ListItem[] = viewState.issues.map((issue: Issue, i: number) => {
    let label: string;

    if (isLinear) {
      // Linear: status indicator + priority + identifier + title + assignee + points
      const status = getStatusIndicator(issue.stateType);
      const priority = getPriorityIndicator(issue.priority);
      const initials = getInitials(issue.assignee?.name);
      const points = issue.estimate !== undefined ? `${issue.estimate}p` : '';

      // Format: ▒! ENG-123 Issue title here                CD 2p
      const titleMaxLen = 32;
      const truncatedTitle = truncate(issue.title, titleMaxLen);
      label = `${priority} ${issue.identifier} ${truncatedTitle}`;

      // Add assignee and points at the end
      const suffix = [initials, points].filter(Boolean).join(' ');
      if (suffix) {
        // Pad to align
        const padding = Math.max(0, 50 - label.length - suffix.length);
        label = label + ' '.repeat(padding) + suffix;
      }

      return {
        id: String(i),
        label,
        actions: issueActions,
        prefix: <Text color={status.color}>{status.char}</Text>,
      };
    } else {
      // GitHub: [livestock] identifier title
      const sourceTag = issue.source.type === 'github' ? issue.source.livestockName : '';
      label = `[${sourceTag}] ${issue.identifier} ${truncate(issue.title, 35)}`;

      return {
        id: String(i),
        label,
        actions: issueActions,
      };
    }
  });

  // Build current filter description
  const getFilterDescription = (): string => {
    if (!isLinear) return '';
    const parts: string[] = [];

    // Assignee - check explicitly for the different states
    // assigneeId can be: string (specific user), null (unassigned), or undefined (anyone)
    if (currentFilter.assigneeId === null) {
      parts.push('Unassigned');
    } else if (filterByMe) {
      // User explicitly selected "Me" filter
      parts.push('Me');
    } else if (currentFilter.assigneeId !== undefined && currentFilter.assigneeId !== '') {
      // Has a specific assignee ID (not "me")
      const assignee = assignees.find((a) => a.id === currentFilter.assigneeId);
      parts.push(assignee?.name ?? 'Assigned');
    } else {
      // undefined or empty string means anyone
      parts.push('Anyone');
    }

    // Cycle
    if (currentFilter.cycleId) {
      const cycle = cycles.find((c) => c.id === currentFilter.cycleId);
      parts.push(cycle?.name ?? 'Cycle');
    } else {
      parts.push('Any cycle');
    }

    // Status
    if (currentFilter.stateType) {
      const stateType = currentFilter.stateType;
      if (Array.isArray(stateType)) {
        // Check if it's the "open" preset
        const isOpenPreset = stateType.length === 3 &&
          stateType.includes('backlog') &&
          stateType.includes('unstarted') &&
          stateType.includes('started');
        parts.push(isOpenPreset ? 'Open' : stateType.join(', '));
      } else {
        // Single status
        const statusLabels: Record<string, string> = {
          backlog: 'Backlog',
          unstarted: 'Todo',
          started: 'In Progress',
          completed: 'Done',
          canceled: 'Canceled',
        };
        parts.push(statusLabels[stateType] ?? stateType);
      }
    } else {
      parts.push('All statuses');
    }

    // Sort
    if (currentFilter.sortBy === 'priority') {
      parts.push(currentFilter.sortDirection === 'asc' ? '↑Priority' : '↓Priority');
    } else if (currentFilter.sortBy === 'createdAt') {
      parts.push('Recent');
    } else if (currentFilter.sortBy === 'updatedAt') {
      parts.push('Updated');
    }

    return parts.join(' • ');
  };

  const detailsHints = 'j/k scroll';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header text={project.name} subtitle={getSubtitle()} color={project.color} />

      {/* Page-level filter info (hotkeys shown in bottom bar) */}
      {isLinear && (
        <Box paddingX={2}>
          <Text dimColor>Showing: </Text>
          <Text color="cyan">{getFilterDescription()}</Text>
          <Text dimColor> ({viewState.issues.length})</Text>
        </Box>
      )}

      <Box flexGrow={1} flexShrink={1} paddingX={1} gap={2} overflow="hidden">
        <Panel title="Issues" focused={focusedPanel === 'list'} width="45%">
          {issueItems.length > 0 ? (
            <List
              items={issueItems}
              focused={focusedPanel === 'list'}
              selectedIndex={selectedIndex}
              onSelectionChange={setSelectedIndex}
              onAction={(item, actionKey) => {
                const issue = viewState.issues[parseInt(item.id, 10)];
                if (!issue) return;

                if (actionKey === 'c') {
                  handleOpenClaude(issue);
                } else if (actionKey === 'o') {
                  openIssueInBrowser(issue.url);
                }
              }}
            />
          ) : (
            <Text dimColor>No issues match the current filter</Text>
          )}
        </Panel>

        <Panel
          title={selectedIssue ? `${selectedIssue.identifier} ${truncate(selectedIssue.title, 30)}` : 'Details'}
          focused={focusedPanel === 'details'}
          width="55%"
          hints={detailsHints}
        >
          {selectedIssue ? (
            <ScrollableMarkdown focused={focusedPanel === 'details'} height={panelContentHeight}>
              {buildIssueDetails(selectedIssue)}
            </ScrollableMarkdown>
          ) : (
            <Text dimColor>Select an issue to view details</Text>
          )}
        </Panel>
      </Box>
    </Box>
  );
}
