import React, { useState, useMemo } from 'react';
import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { Header } from '../components/Header.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem, type RowAction } from '../components/List.js';
import { PathInput } from '../components/PathInput.js';
import { parseSshConfig, type SshHost } from '../lib/ssh.js';
import type { Project, Barn } from '../types.js';
import { getWindowStatus, type TmuxWindow } from '../lib/tmux.js';
import { isLocalBarn } from '../lib/config.js';

type FocusedPanel = 'projects' | 'sessions' | 'barns';
type Mode =
  | 'normal'
  | 'new-project-name' | 'new-project-path'
  | 'new-barn-select-ssh' | 'new-barn-name' | 'new-barn-host' | 'new-barn-user' | 'new-barn-port' | 'new-barn-key';

interface GlobalDashboardProps {
  projects: Project[];
  barns: Barn[];
  windows: TmuxWindow[];
  versionInfo?: {
    current: string;
    latest: string | null;
  };
  onSelectProject: (project: Project) => void;
  onSelectBarn: (barn: Barn) => void;
  onSelectWindow: (window: TmuxWindow) => void;
  onNewClaudeForProject: (project: Project) => void;
  onCreateProject: (name: string, path: string) => void;
  onCreateBarn: (barn: Barn) => void;
  onSshToBarn: (barn: Barn) => void;
  onInputModeChange?: (isInputMode: boolean) => void;
}

function countSessionsForProject(projectName: string, windows: TmuxWindow[]): number {
  return windows.filter((w) => w.name.startsWith(projectName)).length;
}

export function GlobalDashboard({
  projects,
  barns,
  windows,
  versionInfo,
  onSelectProject,
  onSelectBarn,
  onSelectWindow,
  onNewClaudeForProject,
  onCreateProject,
  onCreateBarn,
  onSshToBarn,
  onInputModeChange,
}: GlobalDashboardProps) {
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('projects');
  const [mode, setModeInternal] = useState<Mode>('normal');

  // Wrapper to notify parent when input mode changes
  const setMode = (newMode: Mode) => {
    setModeInternal(newMode);
    onInputModeChange?.(newMode !== 'normal');
  };

  // New project form state
  const [newProjectName, setNewProjectName] = useState('');
  const [newProjectPath, setNewProjectPath] = useState('');

  // New barn form state
  const [newBarnName, setNewBarnName] = useState('');
  const [newBarnHost, setNewBarnHost] = useState('');
  const [newBarnUser, setNewBarnUser] = useState('');
  const [newBarnPort, setNewBarnPort] = useState('22');
  const [newBarnKey, setNewBarnKey] = useState('');

  // SSH hosts from config
  const sshHosts = useMemo(() => parseSshConfig(), []);

  // Filter out window 0 (yeehaw TUI)
  const sessionWindows = windows.filter((w) => w.index > 0);

  // Create a map from display number (1-9) to window for quick access
  const windowsByDisplayNum = new Map<number, typeof sessionWindows[0]>();
  sessionWindows.forEach((w, i) => {
    if (i < 9) {
      windowsByDisplayNum.set(i + 1, w);
    }
  });

  const resetNewProject = () => {
    setNewProjectName('');
    setNewProjectPath('');
  };

  const resetNewBarn = () => {
    setNewBarnName('');
    setNewBarnHost('');
    setNewBarnUser('');
    setNewBarnPort('22');
    setNewBarnKey('');
  };

  useInput((input, key) => {
    // Handle escape to cancel creation
    if (key.escape && mode !== 'normal') {
      setMode('normal');
      resetNewProject();
      resetNewBarn();
      return;
    }

    // Only process these in normal mode
    if (mode !== 'normal') return;

    // Tab to cycle panels
    if (key.tab) {
      setFocusedPanel((p) => {
        if (p === 'projects') return 'sessions';
        if (p === 'sessions') return 'barns';
        return 'projects';
      });
      return;
    }

    if (input === 'n') {
      if (focusedPanel === 'projects') {
        setMode('new-project-name');
        return;
      }
      if (focusedPanel === 'barns') {
        // If we have SSH hosts, show selection first
        if (sshHosts.length > 0) {
          setMode('new-barn-select-ssh');
        } else {
          setMode('new-barn-name');
        }
        return;
      }
    }

    // Number hotkeys 1-9 for quick session switching
    const num = parseInt(input, 10);
    if (num >= 1 && num <= 9) {
      const window = windowsByDisplayNum.get(num);
      if (window) {
        onSelectWindow(window);
      }
      return;
    }
  });

  const handleProjectNameSubmit = (name: string) => {
    if (name.trim()) {
      setNewProjectName(name.trim());
      setNewProjectPath('');
      setMode('new-project-path');
    }
  };

  const handleProjectPathSubmit = (path: string) => {
    if (path.trim() && newProjectName) {
      onCreateProject(newProjectName, path.trim());
      setMode('normal');
      resetNewProject();
    }
  };

  const handleBarnNameSubmit = (name: string) => {
    if (name.trim()) {
      setNewBarnName(name.trim());
      setMode('new-barn-host');
    }
  };

  const handleBarnHostSubmit = (host: string) => {
    if (host.trim()) {
      setNewBarnHost(host.trim());
      setMode('new-barn-user');
    }
  };

  const handleBarnUserSubmit = (user: string) => {
    if (user.trim()) {
      setNewBarnUser(user.trim());
      setMode('new-barn-port');
    }
  };

  const handleBarnPortSubmit = (port: string) => {
    setNewBarnPort(port.trim() || '22');
    setMode('new-barn-key');
  };

  const handleBarnKeySubmit = (key: string) => {
    if (key.trim()) {
      const barn: Barn = {
        name: newBarnName,
        host: newBarnHost,
        user: newBarnUser,
        port: parseInt(newBarnPort, 10) || 22,
        identity_file: key.trim(),
        critters: [],
      };
      onCreateBarn(barn);
      setMode('normal');
      resetNewBarn();
    }
  };

  const handleSshHostSelect = (host: SshHost) => {
    // Pre-fill from SSH config
    setNewBarnName(host.name);
    setNewBarnHost(host.hostname || host.name);
    setNewBarnUser(host.user || 'root');
    setNewBarnPort(String(host.port || 22));
    setNewBarnKey(host.identityFile || '');
    setMode('new-barn-name');
  };

  const projectItems: ListItem[] = projects.map((p) => {
    const sessionCount = countSessionsForProject(p.name, windows);
    return {
      id: p.name,
      label: p.name,
      status: sessionCount > 0 ? 'active' : 'inactive',
      meta: sessionCount > 0 ? `${sessionCount} session${sessionCount > 1 ? 's' : ''}` : undefined,
      actions: [{ key: 'c', label: 'claude' }],
    };
  });

  // Parse window name to show clearer labels
  const formatSessionLabel = (name: string): { label: string; typeHint: string } => {
    // Remote yeehaw connection
    if (name.startsWith('remote:')) {
      return { label: name.replace('remote:', ''), typeHint: 'remote' };
    }
    // Barn shell session
    if (name.startsWith('barn-')) {
      return { label: name.replace('barn-', ''), typeHint: 'barn' };
    }
    // Claude session
    if (name.endsWith('-claude')) {
      return { label: name.replace('-claude', ''), typeHint: 'claude' };
    }
    // Livestock session (project-livestock format)
    const parts = name.split('-');
    if (parts.length >= 2) {
      const projectName = parts.slice(0, -1).join('-');
      const livestockName = parts[parts.length - 1];
      return { label: `${projectName} · ${livestockName}`, typeHint: 'shell' };
    }
    return { label: name, typeHint: '' };
  };

  // Use display numbers (1-9) instead of window index
  const sessionItems: ListItem[] = sessionWindows.map((w, i) => {
    const { label, typeHint } = formatSessionLabel(w.name);
    const statusInfo = getWindowStatus(w);
    return {
      id: String(w.index),
      label: `[${i + 1}] ${label}`,
      status: w.active ? 'active' : 'inactive',
      meta: typeHint ? `${typeHint} · ${statusInfo.text}` : statusInfo.text,
      sessionStatus: statusInfo.status,
    };
  });

  const barnItems: ListItem[] = barns.map((b) => ({
    id: b.name,
    label: isLocalBarn(b) ? 'local' : b.name,
    status: 'active',
    meta: isLocalBarn(b) ? 'this machine' : `${b.user}@${b.host}`,
    actions: [{ key: 's', label: 'shell' }],
  }));

  // New project modals
  if (mode === 'new-project-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Create New Project</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newProjectName}
              onChange={setNewProjectName}
              onSubmit={handleProjectNameSubmit}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Press Enter to continue, Esc to cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'new-project-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Create New Project: {newProjectName}</Text>
          <Box marginTop={1}>
            <Text>Path: </Text>
            <PathInput
              value={newProjectPath}
              onChange={setNewProjectPath}
              onSubmit={handleProjectPathSubmit}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Tab to autocomplete, Enter to create, Esc to cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // SSH host selection for new barn
  if (mode === 'new-barn-select-ssh') {
    const sshHostItems: ListItem[] = [
      { id: '__manual__', label: 'Enter manually...', status: 'inactive' },
      ...sshHosts.map((h) => ({
        id: h.name,
        label: h.name,
        status: 'active' as const,
        meta: h.hostname ? `${h.user || 'root'}@${h.hostname}` : undefined,
      })),
    ];

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add New Barn</Text>
          <Text dimColor>Select from SSH config or enter manually</Text>
          <Box marginTop={1} flexDirection="column">
            <List
              items={sshHostItems}
              focused={true}
              onSelect={(item) => {
                if (item.id === '__manual__') {
                  setMode('new-barn-name');
                } else {
                  const host = sshHosts.find((h) => h.name === item.id);
                  if (host) handleSshHostSelect(host);
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: select, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // New barn modals
  if (mode === 'new-barn-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add New Barn</Text>
          <Text dimColor>A barn is a server you manage</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newBarnName}
              onChange={setNewBarnName}
              onSubmit={handleBarnNameSubmit}
              placeholder="my-server"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'new-barn-host') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add New Barn: {newBarnName}</Text>
          <Box marginTop={1}>
            <Text>Host: </Text>
            <TextInput
              value={newBarnHost}
              onChange={setNewBarnHost}
              onSubmit={handleBarnHostSubmit}
              placeholder="192.168.1.100 or server.example.com"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'new-barn-user') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add New Barn: {newBarnName}</Text>
          <Box marginTop={1}>
            <Text>SSH User: </Text>
            <TextInput
              value={newBarnUser}
              onChange={setNewBarnUser}
              onSubmit={handleBarnUserSubmit}
              placeholder="root"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'new-barn-port') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add New Barn: {newBarnName}</Text>
          <Box marginTop={1}>
            <Text>SSH Port: </Text>
            <TextInput
              value={newBarnPort}
              onChange={setNewBarnPort}
              onSubmit={handleBarnPortSubmit}
              placeholder="22"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field (leave blank for 22), Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'new-barn-key') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text="YEEHAW" versionInfo={versionInfo} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add New Barn: {newBarnName}</Text>
          <Box marginTop={1}>
            <Text>SSH Key Path: </Text>
            <PathInput
              value={newBarnKey}
              onChange={setNewBarnKey}
              onSubmit={handleBarnKeySubmit}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Tab: autocomplete, Enter: create barn, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  const projectHints = '[n] new';
  const sessionHints = '';
  const barnHints = '[n] new';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header text="YEEHAW" versionInfo={versionInfo} />

      <Box flexGrow={1} marginY={1} paddingX={1} gap={2}>
        {/* Left column: Projects + Barns stacked */}
        <Box flexDirection="column" width="40%" gap={1}>
          <Panel
            title="Projects"
            focused={focusedPanel === 'projects'}
            hints={projectHints}
          >
            {projectItems.length > 0 ? (
              <List
                items={projectItems}
                focused={focusedPanel === 'projects'}
                onSelect={(item) => {
                  const project = projects.find((p) => p.name === item.id);
                  if (project) onSelectProject(project);
                }}
                onAction={(item, actionKey) => {
                  if (actionKey === 'c') {
                    const project = projects.find((p) => p.name === item.id);
                    if (project) onNewClaudeForProject(project);
                  }
                }}
              />
            ) : (
              <Text dimColor>No projects yet</Text>
            )}
          </Panel>

          <Box flexGrow={1} width="100%">
            <Panel title="Barns" focused={focusedPanel === 'barns'} width="100%" hints={barnHints}>
              {barnItems.length > 0 ? (
                <Box flexDirection="column">
                  <List
                    items={barnItems}
                    focused={focusedPanel === 'barns'}
                    onSelect={(item) => {
                      const barn = barns.find((b) => b.name === item.id);
                      if (barn) onSelectBarn(barn);
                    }}
                    onAction={(item, actionKey) => {
                      if (actionKey === 's') {
                        const barn = barns.find((b) => b.name === item.id);
                        if (barn) onSshToBarn(barn);
                      }
                    }}
                  />
                </Box>
              ) : (
                <Box flexDirection="column">
                  <Text dimColor>No barns configured</Text>
                  <Text dimColor italic>Barns are servers you manage</Text>
                </Box>
              )}
            </Panel>
          </Box>
        </Box>

        {/* Right: Sessions (full height) */}
        <Panel
          title="Sessions"
          focused={focusedPanel === 'sessions'}
          width="60%"
          hints={sessionHints}
        >
          {sessionItems.length > 0 ? (
            <List
              items={sessionItems}
              focused={focusedPanel === 'sessions'}
              onSelect={(item) => {
                const window = sessionWindows.find((w) => String(w.index) === item.id);
                if (window) onSelectWindow(window);
              }}
            />
          ) : (
            <Text dimColor>No active sessions</Text>
          )}
        </Panel>
      </Box>
    </Box>
  );
}
