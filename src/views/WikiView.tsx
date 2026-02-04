import React, { useState, useEffect, useCallback } from 'react';
import { Box, Text, useInput, useStdout } from 'ink';
import TextInput from 'ink-text-input';
import { Header } from '../components/Header.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem } from '../components/List.js';
import { TextArea } from '../components/TextArea.js';
import { ScrollableMarkdown } from '../components/ScrollableMarkdown.js';
import type { Project } from '../types.js';
import {
  getWikiProvider,
  LocalWikiProvider,
  LinearWikiProvider,
  type WikiSection,
  type WikiProvider,
  type LinearTeam,
} from '../lib/wiki/index.js';
import { saveProject } from '../lib/config.js';
import { saveLinearApiKey, validateLinearApiKey, LINEAR_API_KEY_URL, clearLinearToken } from '../lib/auth/linear.js';

type FocusedPanel = 'sections' | 'content';

type ViewState =
  | { type: 'loading' }
  | { type: 'error'; message: string }
  | { type: 'not-authenticated' }
  | { type: 'linear-auth-input' }
  | { type: 'linear-auth-validating' }
  | { type: 'select-team'; teams: LinearTeam[] }
  | { type: 'ready' }
  | { type: 'add-title' }
  | { type: 'add-content' }
  | { type: 'edit-title' }
  | { type: 'edit-content' }
  | { type: 'delete-confirm' };

interface WikiViewProps {
  project: Project;
  onBack: () => void;
  onUpdateProject: (project: Project) => void;
}

export function WikiView({ project, onBack, onUpdateProject }: WikiViewProps) {
  const { stdout } = useStdout();
  const [viewState, setViewState] = useState<ViewState>({ type: 'loading' });
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('sections');

  // Provider and data state
  const [provider, setProvider] = useState<WikiProvider | null>(null);
  const [sections, setSections] = useState<WikiSection[]>([]);
  const [linearProvider, setLinearProvider] = useState<LinearWikiProvider | null>(null);

  // Form state
  const [editTitle, setEditTitle] = useState('');
  const [editContent, setEditContent] = useState('');
  const [apiKeyInput, setApiKeyInput] = useState('');
  const [sectionToDelete, setSectionToDelete] = useState<WikiSection | null>(null);

  // Team name for display
  const [teamName, setTeamName] = useState<string | undefined>();

  // Calculate dynamic height for ScrollableMarkdown
  const terminalHeight = stdout?.rows ?? 24;
  const panelContentHeight = Math.max(8, terminalHeight - 10);

  const selectedSection = sections[selectedIndex];

  const resetForm = () => {
    setEditTitle('');
    setEditContent('');
  };

  // Load wiki sections
  const loadSections = useCallback(async () => {
    setViewState({ type: 'loading' });

    const wikiProvider = getWikiProvider(project, onUpdateProject);
    setProvider(wikiProvider);

    // Local provider - just load directly
    if (wikiProvider.type === 'local') {
      try {
        const fetchedSections = await wikiProvider.fetchSections();
        setSections(fetchedSections);
        setViewState({ type: 'ready' });
      } catch (err) {
        setViewState({ type: 'error', message: `Failed to load wiki: ${err}` });
      }
      return;
    }

    // Linear provider - check auth and team selection
    if (wikiProvider.type === 'linear') {
      const linearProv = wikiProvider as LinearWikiProvider;
      setLinearProvider(linearProv);

      // Check authentication
      const isAuthed = await linearProv.isAuthenticated();
      if (!isAuthed) {
        setViewState({ type: 'not-authenticated' });
        return;
      }

      // Check if team selection is needed
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

      // Fetch sections
      try {
        const fetchedSections = await linearProv.fetchSections();
        setSections(fetchedSections);
        setViewState({ type: 'ready' });
      } catch (err) {
        setViewState({ type: 'error', message: `Failed to load wiki: ${err}` });
      }
    }
  }, [project, onUpdateProject]);

  useEffect(() => {
    loadSections();
  }, [loadSections]);

  // Handle API key submission
  const handleApiKeySubmit = async () => {
    if (!apiKeyInput.trim()) return;

    setViewState({ type: 'linear-auth-validating' });

    const isValid = await validateLinearApiKey(apiKeyInput.trim());
    if (isValid) {
      saveLinearApiKey(apiKeyInput.trim());
      setApiKeyInput('');
      loadSections();
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

    // Save team ID and name to project wiki provider config
    const updatedProject: Project = {
      ...project,
      wikiProvider: { type: 'linear', teamId: team.id, teamName: team.name },
    };
    saveProject(updatedProject);
    onUpdateProject(updatedProject);

    // Continue with loading sections
    setViewState({ type: 'loading' });

    try {
      const fetchedSections = await linearProvider.fetchSections();
      setSections(fetchedSections);
      setViewState({ type: 'ready' });
    } catch (err) {
      setViewState({ type: 'error', message: `Failed to load wiki: ${err}` });
    }
  };

  // Save section (local provider only)
  const saveSection = async (title: string, content: string, isNew: boolean) => {
    if (!provider || provider.isReadOnly) return;

    const localProvider = provider as LocalWikiProvider;

    try {
      if (isNew) {
        await localProvider.addSection({ title, content });
        // Update local state to reflect new section
        const newSections = [...sections, { title, content }];
        setSections(newSections);
        setSelectedIndex(newSections.length - 1);
      } else {
        const oldTitle = sections[selectedIndex].title;
        await localProvider.updateSection(oldTitle, { title, content });
        // Update local state
        const newSections = [...sections];
        newSections[selectedIndex] = { title, content };
        setSections(newSections);
      }
      setViewState({ type: 'ready' });
      resetForm();
    } catch (err) {
      setViewState({ type: 'error', message: `Failed to save: ${err}` });
    }
  };

  // Delete section (local provider only)
  const deleteSection = async () => {
    if (!provider || provider.isReadOnly || !sectionToDelete) return;

    const localProvider = provider as LocalWikiProvider;

    try {
      await localProvider.deleteSection(sectionToDelete.title);
      // Update local state
      const deleteIndex = sections.findIndex(s => s.title === sectionToDelete.title);
      const newSections = sections.filter(s => s.title !== sectionToDelete.title);
      setSections(newSections);
      // Adjust selected index if needed
      const newIndex = newSections.length === 0 ? 0 : Math.min(deleteIndex, newSections.length - 1);
      setSelectedIndex(newIndex);
      setSectionToDelete(null);
      setViewState({ type: 'ready' });
    } catch (err) {
      setSectionToDelete(null);
      setViewState({ type: 'error', message: `Failed to delete: ${err}` });
    }
  };

  // Handle input
  useInput((input, key) => {
    // Don't intercept input during text input modes
    if (viewState.type === 'linear-auth-input') {
      if (key.escape) {
        setApiKeyInput('');
        setViewState({ type: 'not-authenticated' });
      }
      return;
    }

    // Handle escape - works in most modes
    if (key.escape) {
      if (viewState.type === 'add-title' || viewState.type === 'add-content' ||
          viewState.type === 'edit-title' || viewState.type === 'edit-content') {
        setViewState({ type: 'ready' });
        resetForm();
      } else if (viewState.type === 'delete-confirm') {
        setSectionToDelete(null);
        setViewState({ type: 'ready' });
      } else if (viewState.type === 'ready') {
        onBack();
      } else if (viewState.type === 'error' || viewState.type === 'not-authenticated') {
        onBack();
      }
      return;
    }

    // Delete confirmation mode
    if (viewState.type === 'delete-confirm') {
      if (input === 'y') {
        deleteSection();
      } else if (input === 'n') {
        setSectionToDelete(null);
        setViewState({ type: 'ready' });
      }
      return;
    }

    // Handle auth prompt - press Enter to start API key input
    if (viewState.type === 'not-authenticated') {
      if (key.return) {
        setViewState({ type: 'linear-auth-input' });
      }
      return;
    }

    // Handle error state - 'r' to retry, 'a' to re-authenticate (Linear only)
    if (viewState.type === 'error') {
      if (input === 'r') {
        loadSections();
        return;
      }
      if (input === 'a' && project.wikiProvider?.type === 'linear') {
        clearLinearToken();
        setViewState({ type: 'linear-auth-input' });
        return;
      }
      return;
    }

    // Only process navigation/actions in ready state
    if (viewState.type !== 'ready') return;

    // Tab to switch focus between panels
    if (key.tab) {
      setFocusedPanel((prev) => (prev === 'sections' ? 'content' : 'sections'));
      return;
    }

    // Refresh ('r' key)
    if (input === 'r') {
      loadSections();
      return;
    }

    // Only handle section operations when sections panel is focused
    if (focusedPanel !== 'sections') return;

    // Check if read-only before allowing edits
    const isReadOnly = provider?.isReadOnly ?? false;

    if (input === 'n' && !isReadOnly) {
      setEditTitle('');
      setEditContent('');
      setViewState({ type: 'add-title' });
      return;
    }

    if (input === 'e' && selectedSection && !isReadOnly) {
      setEditTitle(selectedSection.title);
      setEditContent(selectedSection.content);
      setViewState({ type: 'edit-title' });
      return;
    }

    if (input === 'd' && selectedSection && !isReadOnly) {
      setSectionToDelete(selectedSection);
      setViewState({ type: 'delete-confirm' });
      return;
    }
  });

  // Get subtitle for header
  const getSubtitle = (): string => {
    if (project.wikiProvider?.type === 'linear' && teamName) {
      return `Wiki • ${teamName} (read-only)`;
    }
    return 'Wiki';
  };

  // Loading state
  if (viewState.type === 'loading') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={getSubtitle()} color={project.color} />
        <Box padding={2}>
          <Text>Loading wiki...</Text>
        </Box>
      </Box>
    );
  }

  // Not authenticated - Linear
  if (viewState.type === 'not-authenticated') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Wiki" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="yellow">Linear authentication required</Text>
          <Box marginTop={1}>
            <Text>You need a Personal API Key from Linear.</Text>
          </Box>
          <Box marginTop={1}>
            <Text>Create one at: <Text color="cyan">{LINEAR_API_KEY_URL}</Text></Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Press Enter to paste your API key, Esc to go back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Linear API key input
  if (viewState.type === 'linear-auth-input') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Wiki" color={project.color} />
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
        <Header text={project.name} subtitle="Wiki" color={project.color} />
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
        <Header text={project.name} subtitle="Wiki" color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text bold color="yellow">Select a Linear team</Text>
          <Box marginBottom={1}>
            <Text dimColor>Which team's projects should be shown as wiki sections?</Text>
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

  // Error state
  if (viewState.type === 'error') {
    const isLinear = project.wikiProvider?.type === 'linear';
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={getSubtitle()} color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="red">Error: {viewState.message}</Text>
          <Box marginTop={1}>
            <Text dimColor>
              Press 'r' to retry{isLinear ? ", 'a' to re-authenticate" : ''}, Esc to go back
            </Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add/Edit title screen
  if (viewState.type === 'add-title' || viewState.type === 'edit-title') {
    const isNew = viewState.type === 'add-title';
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Wiki" color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">{isNew ? 'New Wiki Section' : 'Edit Wiki Section'}</Text>
          <Box marginTop={1}>
            <Text>Title: </Text>
            <TextInput
              value={editTitle}
              onChange={setEditTitle}
              onSubmit={() => {
                if (editTitle.trim()) {
                  setViewState({ type: isNew ? 'add-content' : 'edit-content' });
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add/Edit content screen
  if (viewState.type === 'add-content' || viewState.type === 'edit-content') {
    const isNew = viewState.type === 'add-content';
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Wiki" color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">{editTitle}</Text>
          <Box marginTop={1} flexDirection="column">
            <Text>Content (markdown):</Text>
            <Box marginTop={1}>
              <TextArea
                value={editContent}
                onChange={setEditContent}
                onSubmit={() => {
                  if (editContent.trim()) {
                    saveSection(editTitle, editContent, isNew);
                  }
                }}
                placeholder="Write your content here..."
                height={10}
              />
            </Box>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: new line  |  Ctrl+S: save  |  Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Delete confirmation screen
  if (viewState.type === 'delete-confirm') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Wiki" color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Delete Section</Text>
          <Box marginTop={1}>
            <Text>Delete "{sectionToDelete?.title}"?</Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, delete</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ready state - build section items for list
  const sectionItems: ListItem[] = sections.map((section, i) => ({
    id: String(i),
    label: section.title,
  }));

  // Panel-specific hints based on provider type
  const isReadOnly = provider?.isReadOnly ?? false;
  const sectionsHints = isReadOnly
    ? '[r] refresh'
    : '[n] new  [e] edit  [d] delete  [r] refresh';
  const contentHints = 'j/k scroll';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header text={project.name} subtitle={getSubtitle()} color={project.color} />

      <Box flexGrow={1} paddingX={1} gap={2}>
        {/* Left: Section list */}
        <Panel title="Sections" focused={focusedPanel === 'sections'} width="30%" hints={sectionsHints}>
          {sectionItems.length > 0 ? (
            <List
              items={sectionItems}
              focused={focusedPanel === 'sections'}
              selectedIndex={selectedIndex}
              onSelectionChange={setSelectedIndex}
              onSelect={(item) => {
                setSelectedIndex(parseInt(item.id, 10));
              }}
            />
          ) : (
            <Text dimColor>
              {isReadOnly ? 'No projects found in Linear team' : 'No wiki sections yet'}
            </Text>
          )}
        </Panel>

        {/* Right: Content */}
        <Panel title={selectedSection?.title || 'Content'} focused={focusedPanel === 'content'} width="70%" hints={contentHints}>
          {selectedSection ? (
            <ScrollableMarkdown focused={focusedPanel === 'content'} height={panelContentHeight}>
              {selectedSection.content}
            </ScrollableMarkdown>
          ) : (
            <Text dimColor>Select a section to view its content</Text>
          )}
        </Panel>
      </Box>
    </Box>
  );
}
