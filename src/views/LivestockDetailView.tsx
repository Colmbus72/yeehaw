import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { Header } from '../components/Header.js';
import { LivestockHeader } from '../components/LivestockHeader.js';
import { List, type ListItem } from '../components/List.js';
import { Panel } from '../components/Panel.js';
import { PathInput } from '../components/PathInput.js';
import { loadBarn } from '../lib/config.js';
import { getWindowStatus, type TmuxWindow } from '../lib/tmux.js';
import type { Project, Livestock, Barn } from '../types.js';

type Mode =
  | 'normal'
  | 'edit-name'
  | 'edit-path'
  | 'edit-repo'
  | 'edit-branch'
  | 'edit-log-path'
  | 'edit-env-path';

interface LivestockDetailViewProps {
  project: Project;
  livestock: Livestock;
  source: 'project' | 'barn';
  sourceBarn?: Barn;
  windows: TmuxWindow[];
  onBack: () => void;
  onOpenLogs: () => void;
  onOpenSession: () => void;
  onOpenClaude?: () => void;  // Only available for local livestock
  onSelectWindow: (window: TmuxWindow) => void;
  onUpdateLivestock: (originalLivestock: Livestock, updatedLivestock: Livestock) => void;
}

export function LivestockDetailView({
  project,
  livestock,
  source,
  sourceBarn,
  windows,
  onBack,
  onOpenLogs,
  onOpenSession,
  onOpenClaude,
  onSelectWindow,
  onUpdateLivestock,
}: LivestockDetailViewProps) {
  const [mode, setMode] = useState<Mode>('normal');

  // Edit form state
  const [editName, setEditName] = useState(livestock.name);
  const [editPath, setEditPath] = useState(livestock.path);
  const [editRepo, setEditRepo] = useState(livestock.repo || '');
  const [editBranch, setEditBranch] = useState(livestock.branch || '');
  const [editLogPath, setEditLogPath] = useState(livestock.log_path || '');
  const [editEnvPath, setEditEnvPath] = useState(livestock.env_path || '');

  // Get barn info if remote
  const barn = livestock.barn ? loadBarn(livestock.barn) : null;

  // Sync form state when livestock prop changes (e.g., after save)
  useEffect(() => {
    setEditName(livestock.name);
    setEditPath(livestock.path);
    setEditRepo(livestock.repo || '');
    setEditBranch(livestock.branch || '');
    setEditLogPath(livestock.log_path || '');
    setEditEnvPath(livestock.env_path || '');
  }, [livestock]);

  const resetForm = () => {
    setEditName(livestock.name);
    setEditPath(livestock.path);
    setEditRepo(livestock.repo || '');
    setEditBranch(livestock.branch || '');
    setEditLogPath(livestock.log_path || '');
    setEditEnvPath(livestock.env_path || '');
  };

  // Save all pending changes at once
  const saveAllChanges = () => {
    const updated: Livestock = {
      ...livestock,
      name: editName.trim() || livestock.name,
      path: editPath.trim() || livestock.path,
      repo: editRepo.trim() || undefined,
      branch: editBranch.trim() || undefined,
      log_path: editLogPath.trim() || undefined,
      env_path: editEnvPath.trim() || undefined,
    };
    onUpdateLivestock(livestock, updated);
    setMode('normal');
  };

  useInput((input, key) => {
    // Handle escape - works in all modes
    if (key.escape) {
      if (mode !== 'normal') {
        setMode('normal');
        resetForm();
      } else {
        onBack();
      }
      return;
    }

    // Handle Ctrl+S to save and exit from any edit mode
    // Note: Ctrl+S sends ASCII 19 (\x13), not 's'
    if ((key.ctrl && input === 's') || input === '\x13') {
      if (mode !== 'normal') {
        saveAllChanges();
        return;
      }
    }

    // Only process these in normal mode
    if (mode !== 'normal') return;

    if (input === 'c') {
      // Claude session only available for local livestock
      if (onOpenClaude && !barn) {
        onOpenClaude();
      }
      return;
    }

    if (input === 's') {
      onOpenSession();
      return;
    }

    if (input === 'l') {
      if (!livestock.log_path) {
        // Could show an error, but for now just ignore
        return;
      }
      onOpenLogs();
      return;
    }

    if (input === 'e') {
      // Start edit flow with name
      setMode('edit-name');
      return;
    }
  });

  // Edit name
  if (mode === 'edit-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Edit: ${livestock.name}`} color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Livestock</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={editName}
              onChange={setEditName}
              onSubmit={() => {
                if (editName.trim()) {
                  setMode('edit-path');
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Edit path
  if (mode === 'edit-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Edit: ${livestock.name}`} color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Livestock</Text>
          <Box marginTop={1}>
            <Text>Path: </Text>
            <PathInput
              value={editPath}
              onChange={setEditPath}
              onSubmit={() => {
                if (editPath.trim()) {
                  setMode('edit-repo');
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next, Tab: autocomplete, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Edit repo
  if (mode === 'edit-repo') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Edit: ${livestock.name}`} color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Livestock</Text>
          <Box marginTop={1}>
            <Text>Git Repo (optional): </Text>
            <TextInput
              value={editRepo}
              onChange={setEditRepo}
              onSubmit={() => setMode('edit-branch')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Edit branch
  if (mode === 'edit-branch') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Edit: ${livestock.name}`} color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Livestock</Text>
          <Box marginTop={1}>
            <Text>Git Branch (optional): </Text>
            <TextInput
              value={editBranch}
              onChange={setEditBranch}
              onSubmit={() => setMode('edit-log-path')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Edit log path
  if (mode === 'edit-log-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Edit: ${livestock.name}`} color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Livestock</Text>
          <Box marginTop={1}>
            <Text>Log Path (optional, relative): </Text>
            <TextInput
              value={editLogPath}
              onChange={setEditLogPath}
              onSubmit={() => setMode('edit-env-path')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Edit env path (last field - saves all changes)
  if (mode === 'edit-env-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Edit: ${livestock.name}`} color={project.color} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Livestock</Text>
          <Box marginTop={1}>
            <Text>Env Path (optional, relative): </Text>
            <TextInput
              value={editEnvPath}
              onChange={setEditEnvPath}
              onSubmit={saveAllChanges}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: save & finish, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Filter windows to this livestock (match pattern: projectname-livestockname)
  const livestockWindowName = `${project.name}-${livestock.name}`;
  const livestockWindows = windows.filter(w => w.name === livestockWindowName || w.name.startsWith(`${livestockWindowName}-`));

  const sessionItems: ListItem[] = livestockWindows.map((w, i) => {
    const statusInfo = getWindowStatus(w);
    return {
      id: String(w.index),
      label: `[${i + 1}] shell`,
      status: w.active ? 'active' : 'inactive',
      meta: statusInfo.text,
      sessionStatus: statusInfo.status,
    };
  });

  // Normal view - show livestock info inline with sessions
  return (
    <Box flexDirection="column" flexGrow={1}>
      <LivestockHeader project={project} livestock={livestock} />

      {/* Details line */}
      <Box paddingX={2} gap={3}>
        <Text>
          <Text dimColor>path:</Text> {livestock.path}
        </Text>
        {barn && (
          <Text>
            <Text dimColor>barn:</Text> {barn.name} <Text dimColor>({barn.host})</Text>
          </Text>
        )}
      </Box>

      {/* Secondary details */}
      <Box paddingX={2} gap={3} marginBottom={1}>
        {livestock.repo && (
          <Text>
            <Text dimColor>repo:</Text> {livestock.repo}
          </Text>
        )}
        {livestock.log_path && (
          <Text>
            <Text dimColor>logs:</Text> {livestock.log_path}
          </Text>
        )}
        {livestock.env_path && (
          <Text>
            <Text dimColor>env:</Text> {livestock.env_path}
          </Text>
        )}
      </Box>

      {/* Sessions for this livestock - no card hints, actions are page-level */}
      <Box paddingX={1} flexGrow={1}>
        <Panel title="Sessions" focused={true}>
          {sessionItems.length > 0 ? (
            <List
              items={sessionItems}
              focused={true}
              onSelect={(item) => {
                const window = livestockWindows.find(w => String(w.index) === item.id);
                if (window) onSelectWindow(window);
              }}
            />
          ) : (
            <Text dimColor italic>No active sessions. Press [s] to start a shell.</Text>
          )}
        </Panel>
      </Box>
    </Box>
  );
}
