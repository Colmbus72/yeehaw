import React, { useState, useCallback } from 'react';
import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { Header } from '../components/Header.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem, type RowAction } from '../components/List.js';
import { PathInput } from '../components/PathInput.js';
import type { Project, Barn, Livestock, Herd, RanchHand, KubernetesConfig, TerraformConfig } from '../types.js';
import { getWindowStatus, type TmuxWindow } from '../lib/tmux.js';
import { detectGitInfo, detectRemoteGitInfo, type GitInfo } from '../lib/git.js';
import { detectLivestockConfig, type DetectedConfig } from '../lib/livestock.js';
import { isLocalBarn, loadRanchHandsForProject, saveRanchHand } from '../lib/config.js';
import { detectTerraformEnvironments, type DetectedTerraformEnv } from '../lib/ranchhand-detect.js';
import { getKubectlContexts } from '../lib/ranchhand-k8s.js';

// HSL color utilities for color picker
function hexToHsl(hex: string): { h: number; s: number; l: number } | null {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (!result) return null;
  const r = parseInt(result[1], 16) / 255;
  const g = parseInt(result[2], 16) / 255;
  const b = parseInt(result[3], 16) / 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let h = 0, s = 0;
  const l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    switch (max) {
      case r: h = ((g - b) / d + (g < b ? 6 : 0)) / 6; break;
      case g: h = ((b - r) / d + 2) / 6; break;
      case b: h = ((r - g) / d + 4) / 6; break;
    }
  }
  return { h: h * 360, s: s * 100, l: l * 100 };
}

function hslToHex(h: number, s: number, l: number): string {
  h = ((h % 360) + 360) % 360;
  s = Math.max(0, Math.min(100, s)) / 100;
  l = Math.max(0, Math.min(100, l)) / 100;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let r = 0, g = 0, b = 0;
  if (h < 60) { r = c; g = x; }
  else if (h < 120) { r = x; g = c; }
  else if (h < 180) { g = c; b = x; }
  else if (h < 240) { g = x; b = c; }
  else if (h < 300) { r = x; b = c; }
  else { r = c; b = x; }
  const toHex = (n: number) => Math.round((n + m) * 255).toString(16).padStart(2, '0');
  return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
}

type FocusedPanel = 'livestock' | 'herds' | 'sessions' | 'ranchhands';
type Mode =
  | 'normal'
  | 'edit-name' | 'edit-path' | 'edit-summary' | 'edit-color' | 'edit-issue-provider' | 'edit-wiki-provider'
  | 'add-livestock-name' | 'add-livestock-barn' | 'add-livestock-path'
  | 'add-livestock-log-path' | 'add-livestock-env-path'
  | 'add-herd-name'
  | 'delete-project-confirm'
  | 'delete-livestock-confirm'
  | 'delete-herd-confirm'
  // Ranch hand creation flow
  | 'add-ranchhand-name'
  | 'add-ranchhand-type'
  | 'add-ranchhand-directory'
  | 'add-ranchhand-scanning'
  | 'add-ranchhand-select-envs'
  | 'add-ranchhand-map-herd'
  | 'add-ranchhand-new-herd';

interface ProjectContextProps {
  project: Project;
  barns: Barn[];
  windows: TmuxWindow[];
  onBack: () => void;
  onNewClaudeForLivestock: (livestock: Livestock) => void;
  onSelectWindow: (window: TmuxWindow) => void;
  onSelectLivestock: (livestock: Livestock, barn: Barn | null) => void;
  onOpenLivestockSession: (livestock: Livestock, barn: Barn | null) => void;
  onUpdateProject: (project: Project) => void;
  onDeleteProject: (projectName: string) => void;
  onOpenWiki: () => void;
  onOpenIssues: () => void;
  onSelectHerd: (herd: Herd) => void;
  onSelectRanchHand: (ranchhand: RanchHand) => void;
}

export function ProjectContext({
  project,
  barns,
  windows,
  onBack,
  onNewClaudeForLivestock,
  onSelectWindow,
  onSelectLivestock,
  onOpenLivestockSession,
  onUpdateProject,
  onDeleteProject,
  onOpenWiki,
  onOpenIssues,
  onSelectHerd,
  onSelectRanchHand,
}: ProjectContextProps) {
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('livestock');
  const [mode, setMode] = useState<Mode>('normal');

  // Edit form state
  const [editName, setEditName] = useState(project.name);
  const [editPath, setEditPath] = useState(project.path);
  const [editSummary, setEditSummary] = useState(project.summary || '');
  const [editColor, setEditColor] = useState(project.color || '');
  const [editGradientSpread, setEditGradientSpread] = useState(project.gradientSpread ?? 5);
  const [editGradientInverted, setEditGradientInverted] = useState(project.gradientInverted ?? false);
  const [editIssueProvider, setEditIssueProvider] = useState<'github' | 'linear' | 'none'>(
    (project.issueProvider?.type) ?? 'github'
  );
  const [editWikiProvider, setEditWikiProvider] = useState<'local' | 'linear'>(
    (project.wikiProvider?.type) ?? 'local'
  );

  // New livestock form state
  const [newLivestockName, setNewLivestockName] = useState('');
  const [newLivestockPath, setNewLivestockPath] = useState('');
  const [newLivestockLogPath, setNewLivestockLogPath] = useState('');
  const [newLivestockEnvPath, setNewLivestockEnvPath] = useState('');
  const [selectedBarn, setSelectedBarn] = useState<Barn | null>(null); // null = local

  // Track selected livestock index for deletion
  const [selectedLivestockIndex, setSelectedLivestockIndex] = useState(0);

  // Delete project confirmation
  const [deleteConfirmInput, setDeleteConfirmInput] = useState('');

  // Delete livestock confirmation
  const [deleteLivestockTarget, setDeleteLivestockTarget] = useState<Livestock | null>(null);

  // Herd state
  const [selectedHerdIndex, setSelectedHerdIndex] = useState(0);
  const [newHerdName, setNewHerdName] = useState('');
  const [deleteHerdTarget, setDeleteHerdTarget] = useState<Herd | null>(null);

  // Ranch hand state
  const [selectedRanchHandIndex, setSelectedRanchHandIndex] = useState(0);
  const ranchHands = loadRanchHandsForProject(project.name);

  // Ranch hand creation flow
  const [newRanchHandName, setNewRanchHandName] = useState('');
  const [newRanchHandType, setNewRanchHandType] = useState<'kubernetes' | 'terraform'>('terraform');
  const [newRanchHandDirectory, setNewRanchHandDirectory] = useState('');
  const [detectedEnvs, setDetectedEnvs] = useState<DetectedTerraformEnv[]>([]);
  const [detectedK8sContexts, setDetectedK8sContexts] = useState<string[]>([]);
  const [selectedEnvIndex, setSelectedEnvIndex] = useState(0);
  const [selectedEnvs, setSelectedEnvs] = useState<Set<string>>(new Set());
  const [envHerdMappings, setEnvHerdMappings] = useState<Map<string, string>>(new Map());
  const [currentMappingEnv, setCurrentMappingEnv] = useState<string | null>(null);
  const [newHerdNameForEnv, setNewHerdNameForEnv] = useState('');
  const [scanError, setScanError] = useState<string | null>(null);

  // Git detection for new livestock
  const [detectedGit, setDetectedGit] = useState<GitInfo | null>(null);

  // Framework detection for config suggestions
  const [detectedConfig, setDetectedConfig] = useState<DetectedConfig | null>(null);

  // Color picker keyboard handlers
  const shiftHue = useCallback((delta: number) => {
    const currentColor = editColor || '#f0c040';
    const hsl = hexToHsl(currentColor);
    if (hsl) {
      const newHex = hslToHex(hsl.h + delta, hsl.s, hsl.l);
      setEditColor(newHex);
    } else if (!editColor) {
      setEditColor(hslToHex(delta > 0 ? 10 : 350, 70, 50));
    }
  }, [editColor]);

  const adjustSpread = useCallback((delta: number) => {
    setEditGradientSpread((s) => Math.max(0, Math.min(10, s + delta)));
  }, []);

  const toggleInvert = useCallback(() => {
    setEditGradientInverted((i) => !i);
  }, []);

  // Finish ranch hand creation
  const finishRanchHandCreation = useCallback((mappings: Map<string, string>) => {
    // Create herds that don't exist
    const existingHerdNames = new Set((project.herds || []).map(h => h.name));
    const newHerds: Herd[] = [];

    for (const herdName of mappings.values()) {
      if (!existingHerdNames.has(herdName) && !newHerds.find(h => h.name === herdName)) {
        newHerds.push({
          name: herdName,
          livestock: [],
          critters: [],
          connections: [],
        });
      }
    }

    // Update project with new herds if any
    if (newHerds.length > 0) {
      onUpdateProject({
        ...project,
        herds: [...(project.herds || []), ...newHerds],
      });
    }

    // Create ranch hands for each selected environment/context
    for (const envId of selectedEnvs) {
      const herd = mappings.get(envId);
      if (!herd) continue;

      let config: RanchHand['config'];
      if (newRanchHandType === 'terraform') {
        const env = detectedEnvs.find(e => e.id === envId);
        if (!env) continue;
        config = {
          backend: env.backendType,
          bucket: env.bucket,
          key: env.key,
          region: env.region,
          local_path: env.localPath,
        };
      } else {
        // K8s: envId is the context name
        config = {
          context: envId,
          kubeconfig_path: newRanchHandDirectory.trim() || undefined,
          private_registries: [],
        };
      }

      const ranchhand: RanchHand = {
        name: `${newRanchHandName}-${envId.replace(/\//g, '-')}`,
        project: project.name,
        type: newRanchHandType,
        config,
        sync_settings: { auto_sync: false },
        herd: herd,
        resource_mappings: [],
      };

      saveRanchHand(ranchhand);
    }

    // Reset state and return to normal
    setNewRanchHandName('');
    setNewRanchHandType('terraform');
    setNewRanchHandDirectory('');
    setDetectedEnvs([]);
    setDetectedK8sContexts([]);
    setSelectedEnvs(new Set());
    setEnvHerdMappings(new Map());
    setCurrentMappingEnv(null);
    setMode('normal');
  }, [project, detectedEnvs, detectedK8sContexts, selectedEnvs, newRanchHandName, newRanchHandType, newRanchHandDirectory, onUpdateProject]);

  // Handle color picker inputs (must be before any conditional returns)
  // Using [ ] for hue shift and ! for invert to avoid conflicts with text input
  useInput((input, key) => {
    if (mode !== 'edit-color') return;
    if (input === '[') shiftHue(-10);
    else if (input === ']') shiftHue(10);
    else if (key.upArrow) adjustSpread(1);
    else if (key.downArrow) adjustSpread(-1);
    else if (input === '!') toggleInvert();
  }, { isActive: mode === 'edit-color' });

  // Filter windows to this project (by name prefix)
  const projectWindows = windows.filter((w) => w.index > 0 && w.name.startsWith(project.name));

  // Create a map from display number (1-9) to window for quick access
  const windowsByDisplayNum = new Map<number, TmuxWindow>();
  projectWindows.forEach((w, i) => {
    if (i < 9) {
      windowsByDisplayNum.set(i + 1, w);
    }
  });

  const startEdit = () => {
    setEditName(project.name);
    setEditPath(project.path);
    setEditSummary(project.summary || '');
    setEditColor(project.color || '');
    setEditGradientSpread(project.gradientSpread ?? 5);
    setEditGradientInverted(project.gradientInverted ?? false);
    setEditIssueProvider((project.issueProvider?.type) ?? 'github');
    setMode('edit-name');
  };

  const cancelEdit = () => {
    setMode('normal');
  };

  const saveAndNext = (nextMode: Mode | 'done') => {
    if (nextMode === 'done') {
      // Save the project
      onUpdateProject({
        ...project,
        name: editName,
        path: editPath,
        summary: editSummary || undefined,
        color: editColor || undefined,
        gradientSpread: editGradientSpread,
        gradientInverted: editGradientInverted,
      });
      setMode('normal');
    } else {
      setMode(nextMode);
    }
  };

  const startAddLivestock = () => {
    setNewLivestockName('');
    setNewLivestockPath('');
    setNewLivestockLogPath('');
    setNewLivestockEnvPath('');
    setSelectedBarn(null);
    setDetectedGit(null);
    setDetectedConfig(null);
    setMode('add-livestock-name');
  };

  const handlePathSubmit = async () => {
    // Detect git info from the path (local or remote)
    if (newLivestockPath) {
      const gitInfo = selectedBarn
        ? detectRemoteGitInfo(newLivestockPath, selectedBarn)
        : detectGitInfo(newLivestockPath);
      setDetectedGit(gitInfo);

      // Detect framework and pre-fill config suggestions
      const config = await detectLivestockConfig(newLivestockPath, selectedBarn || undefined);
      setDetectedConfig(config);
      if (config.log_path) {
        setNewLivestockLogPath(config.log_path);
      }
      if (config.env_path) {
        setNewLivestockEnvPath(config.env_path);
      }

      // Continue to optional config fields
      setMode('add-livestock-log-path');
    }
  };

  const saveLivestock = () => {
    const newLivestock: Livestock = {
      name: newLivestockName,
      path: newLivestockPath,
      barn: selectedBarn?.name,
      repo: detectedGit?.remoteUrl,
      branch: detectedGit?.branch,
      log_path: newLivestockLogPath || undefined,
      env_path: newLivestockEnvPath || undefined,
    };
    const updatedLivestock = [...(project.livestock || [])];
    // Replace if same name exists, otherwise add
    const existingIdx = updatedLivestock.findIndex((l) => l.name === newLivestock.name);
    if (existingIdx >= 0) {
      updatedLivestock[existingIdx] = newLivestock;
    } else {
      updatedLivestock.push(newLivestock);
    }
    onUpdateProject({
      ...project,
      livestock: updatedLivestock,
    });
    setMode('normal');
  };

  useInput((input, key) => {
    // Handle escape - with step-back for ranch hand creation flow
    if (key.escape) {
      if (mode === 'add-ranchhand-name') {
        setMode('normal');
        return;
      }
      if (mode === 'add-ranchhand-type') {
        setMode('add-ranchhand-name');
        return;
      }
      if (mode === 'add-ranchhand-directory') {
        setMode('add-ranchhand-type');
        return;
      }
      if (mode === 'add-ranchhand-select-envs') {
        setMode('add-ranchhand-directory');
        return;
      }
      if (mode === 'add-ranchhand-map-herd') {
        setMode('add-ranchhand-select-envs');
        return;
      }
      if (mode === 'add-ranchhand-new-herd') {
        setMode('add-ranchhand-map-herd');
        return;
      }
      if (mode !== 'normal') {
        cancelEdit();
        setDeleteLivestockTarget(null);
      } else {
        onBack();
      }
      return;
    }

    // Handle ranch hand type selection (j/k navigation)
    if (mode === 'add-ranchhand-type') {
      if (input === 'j' || key.downArrow) {
        setNewRanchHandType('kubernetes');
        return;
      }
      if (input === 'k' || key.upArrow) {
        setNewRanchHandType('terraform');
        return;
      }
      if (key.return) {
        setMode('add-ranchhand-directory');
        return;
      }
    }

    // Handle 'c' to continue in select-envs mode
    if (mode === 'add-ranchhand-select-envs' && input === 'c' && selectedEnvs.size > 0) {
      setCurrentMappingEnv(Array.from(selectedEnvs)[0]);
      setMode('add-ranchhand-map-herd');
      return;
    }

    // Handle delete livestock confirmation
    if (mode === 'delete-livestock-confirm' && deleteLivestockTarget) {
      if (input === 'y') {
        // Perform the actual delete
        const updatedLivestock = (project.livestock || []).filter(
          (l) => l.name !== deleteLivestockTarget.name
        );
        onUpdateProject({
          ...project,
          livestock: updatedLivestock,
        });
        // Adjust selection if needed
        if (selectedLivestockIndex >= updatedLivestock.length && updatedLivestock.length > 0) {
          setSelectedLivestockIndex(updatedLivestock.length - 1);
        }
        setDeleteLivestockTarget(null);
        setMode('normal');
      } else if (input === 'n') {
        setDeleteLivestockTarget(null);
        setMode('normal');
      }
      return;
    }

    // Handle delete herd confirmation
    if (mode === 'delete-herd-confirm' && deleteHerdTarget) {
      if (input === 'y') {
        const updatedHerds = (project.herds || []).filter(
          (h) => h.name !== deleteHerdTarget.name
        );
        onUpdateProject({
          ...project,
          herds: updatedHerds,
        });
        if (selectedHerdIndex >= updatedHerds.length && updatedHerds.length > 0) {
          setSelectedHerdIndex(updatedHerds.length - 1);
        }
        setDeleteHerdTarget(null);
        setMode('normal');
      } else if (input === 'n') {
        setDeleteHerdTarget(null);
        setMode('normal');
      }
      return;
    }

    if (mode !== 'normal') return


    if (key.tab) {
      // Tab order: left→right on each row, then down
      // Top row: Livestock → Sessions
      // Bottom row: Herds → Ranch Hands
      setFocusedPanel((p) => {
        if (p === 'livestock') return 'sessions';
        if (p === 'sessions') return 'herds';
        if (p === 'herds') return 'ranchhands';
        return 'livestock';
      });
      return;
    }

    // NOTE: 'c' for Claude is handled at row-level in the List component
    // via the onAction callback - no page-level 'c' handler here

    if (input === 'e') {
      startEdit();
      return;
    }

    if (input === 'w') {
      onOpenWiki();
      return;
    }

    if (input === 'i') {
      onOpenIssues();
      return;
    }

    if (input === 'D') {
      setDeleteConfirmInput('');
      setMode('delete-project-confirm');
      return;
    }

    // Livestock management (when livestock panel focused)
    if (focusedPanel === 'livestock') {
      if (input === 'n') {
        startAddLivestock();
        return;
      }
      if (input === 'd') {
        const livestock = project.livestock || [];
        if (livestock.length > 0 && selectedLivestockIndex < livestock.length) {
          setDeleteLivestockTarget(livestock[selectedLivestockIndex]);
          setMode('delete-livestock-confirm');
        }
        return;
      }
    }

    // Herd management (when herds panel focused)
    if (focusedPanel === 'herds') {
      if (input === 'n') {
        setNewHerdName('');
        setMode('add-herd-name');
        return;
      }
      if (input === 'd') {
        const herds = project.herds || [];
        if (herds.length > 0 && selectedHerdIndex < herds.length) {
          setDeleteHerdTarget(herds[selectedHerdIndex]);
          setMode('delete-herd-confirm');
        }
        return;
      }
    }

    // Ranch hand management (when ranchhands panel focused)
    if (focusedPanel === 'ranchhands') {
      if (input === 'n') {
        // Reset all creation flow state
        setNewRanchHandName('');
        setNewRanchHandType('terraform');
        setNewRanchHandDirectory('');
        setDetectedEnvs([]);
        setDetectedK8sContexts([]);
        setSelectedEnvs(new Set());
        setEnvHerdMappings(new Map());
        setCurrentMappingEnv(null);
        setScanError(null);
        setMode('add-ranchhand-name');
        return;
      }
    }

    // Number hotkeys 1-9 for quick window switching
    const num = parseInt(input, 10);
    if (num >= 1 && num <= 9) {
      const window = windowsByDisplayNum.get(num);
      if (window) {
        onSelectWindow(window);
      }
      return;
    }
  });

  // Edit mode screens
  if (mode === 'edit-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Editing project" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Project</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={editName}
              onChange={setEditName}
              onSubmit={() => saveAndNext('edit-path')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Editing project" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Project: {editName}</Text>
          <Box marginTop={1}>
            <Text>Path: </Text>
            <PathInput
              value={editPath}
              onChange={setEditPath}
              onSubmit={() => saveAndNext('edit-summary')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Tab: autocomplete, Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-summary') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Editing project" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Project: {editName}</Text>
          <Box marginTop={1}>
            <Text>Summary: </Text>
            <TextInput
              value={editSummary}
              onChange={setEditSummary}
              onSubmit={() => saveAndNext('edit-color')}
              placeholder="Short description..."
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel (leave blank to skip)</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-color') {
    const displayColor = editColor || '#f0c040';
    // Filter out special keys used for color picker controls
    const handleColorChange = (value: string) => {
      const filtered = value.replace(/[\[\]!]/g, '');
      setEditColor(filtered);
    };
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header
          text={project.name}
          subtitle="Editing project"
          color={displayColor}
          gradientSpread={editGradientSpread}
          gradientInverted={editGradientInverted}
        />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Project: {editName}</Text>
          <Box marginTop={1}>
            <Text>Color (hex): </Text>
            <TextInput
              value={editColor}
              onChange={handleColorChange}
              onSubmit={() => saveAndNext('edit-issue-provider')}
              placeholder="#ff6b6b"
            />
          </Box>
          <Box marginTop={1} flexDirection="column">
            <Text dimColor>
              Spread: {editGradientSpread}/10 {editGradientInverted ? '(inverted)' : ''}
            </Text>
            <Box marginTop={1}>
              <Text color={displayColor}>████</Text>
              <Text> Preview</Text>
            </Box>
          </Box>
          <Box marginTop={1} flexDirection="column">
            <Text dimColor>[ ] shift color   ↑/↓ gradient spread   ! invert</Text>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-issue-provider') {
    const providerOptions = [
      { id: 'github', label: 'GitHub Issues', description: 'Use GitHub CLI (gh) for issues' },
      { id: 'linear', label: 'Linear', description: 'Use Linear for issue tracking' },
      { id: 'none', label: 'None', description: 'Disable issue tracking' },
    ];

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header
          text={project.name}
          subtitle="Editing project"
          color={editColor || project.color}
          gradientSpread={editGradientSpread}
          gradientInverted={editGradientInverted}
        />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Project: {editName}</Text>
          <Box marginTop={1}>
            <Text>Issue Tracking:</Text>
          </Box>
          <Box marginTop={1} flexDirection="column">
            <List
              items={providerOptions.map((p) => ({
                id: p.id,
                label: p.label,
                status: editIssueProvider === p.id ? 'active' : 'inactive',
                meta: p.description,
              }))}
              focused={true}
              onSelect={(item) => {
                setEditIssueProvider(item.id as 'github' | 'linear' | 'none');
                setMode('edit-wiki-provider');
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-wiki-provider') {
    const providerOptions = [
      { id: 'local', label: 'Local', description: 'Store wiki sections in project config' },
      { id: 'linear', label: 'Linear Projects', description: 'Fetch wiki from Linear Projects (read-only)' },
    ];

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header
          text={project.name}
          subtitle="Editing project"
          color={editColor || project.color}
          gradientSpread={editGradientSpread}
          gradientInverted={editGradientInverted}
        />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Project: {editName}</Text>
          <Box marginTop={1}>
            <Text>Wiki Provider:</Text>
          </Box>
          <Box marginTop={1} flexDirection="column">
            <List
              items={providerOptions.map((p) => ({
                id: p.id,
                label: p.label,
                status: editWikiProvider === p.id ? 'active' : 'inactive',
                meta: p.description,
              }))}
              focused={true}
              onSelect={(item) => {
                setEditWikiProvider(item.id as 'local' | 'linear');
                // Save the project with all updated fields
                const issueProvider = editIssueProvider === 'github'
                  ? { type: 'github' as const }
                  : editIssueProvider === 'linear'
                  ? { type: 'linear' as const }
                  : { type: 'none' as const };
                const wikiProvider = item.id === 'local'
                  ? { type: 'local' as const }
                  : { type: 'linear' as const };
                onUpdateProject({
                  ...project,
                  name: editName,
                  path: editPath,
                  summary: editSummary || undefined,
                  color: editColor || undefined,
                  gradientSpread: editGradientSpread,
                  gradientInverted: editGradientInverted,
                  issueProvider,
                  wikiProvider,
                });
                setMode('normal');
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: select and save, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add livestock flow: name → barn → path (with auto-detect)
  if (mode === 'add-livestock-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Adding livestock" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock</Text>
          <Text dimColor>Livestock are deployed instances of your app</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newLivestockName}
              onChange={setNewLivestockName}
              onSubmit={() => {
                if (newLivestockName.trim()) {
                  setMode('add-livestock-barn');
                }
              }}
              placeholder="local, dev, production..."
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'add-livestock-barn') {
    // Build barn options including "Local" option (filter out local barn from array to avoid duplicates)
    const remoteBarns = barns.filter((b) => !isLocalBarn(b));
    const barnOptions: ListItem[] = [
      { id: '__local__', label: 'Local (this machine)', status: 'active' },
      ...remoteBarns.map((b) => ({
        id: b.name,
        label: b.name,
        status: 'active' as const,
        meta: `${b.user}@${b.host}`,
      })),
    ];

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Adding livestock" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock: {newLivestockName}</Text>
          <Text dimColor>Where is this livestock deployed?</Text>
          <Box marginTop={1} flexDirection="column">
            <List
              items={barnOptions}
              focused={true}
              onSelect={(item) => {
                if (item.id === '__local__') {
                  setSelectedBarn(null);
                } else {
                  const barn = barns.find((b) => b.name === item.id);
                  setSelectedBarn(barn || null);
                }
                setMode('add-livestock-path');
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

  if (mode === 'add-livestock-path') {
    const locationLabel = selectedBarn
      ? `on ${selectedBarn.name} (${selectedBarn.host})`
      : 'local';

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Adding livestock" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock: {newLivestockName}</Text>
          <Text dimColor>Location: {locationLabel}</Text>
          <Box marginTop={1}>
            <Text>Path: </Text>
            <PathInput
              value={newLivestockPath}
              onChange={setNewLivestockPath}
              onSubmit={handlePathSubmit}
              barn={selectedBarn || undefined}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Tab: autocomplete, Enter: next, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'add-livestock-log-path') {
    const locationLabel = selectedBarn
      ? `on ${selectedBarn.name} (${selectedBarn.host})`
      : 'local';
    const frameworkLabel = detectedConfig?.framework && detectedConfig.framework !== 'unknown'
      ? ` (${detectedConfig.framework} detected)`
      : '';

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Adding livestock" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock: {newLivestockName}</Text>
          <Text dimColor>Location: {locationLabel} • Path: {newLivestockPath}</Text>
          {frameworkLabel && <Text color="cyan">{frameworkLabel}</Text>}
          <Box marginTop={1}>
            <Text>Log path (optional): </Text>
            <TextInput
              value={newLivestockLogPath}
              onChange={setNewLivestockLogPath}
              onSubmit={() => setMode('add-livestock-env-path')}
              placeholder="storage/logs/ or logs/"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Relative to livestock path. Enter: next (leave blank to skip), Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'add-livestock-env-path') {
    const locationLabel = selectedBarn
      ? `on ${selectedBarn.name} (${selectedBarn.host})`
      : 'local';

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Adding livestock" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock: {newLivestockName}</Text>
          <Text dimColor>Location: {locationLabel} • Path: {newLivestockPath}</Text>
          {newLivestockLogPath && <Text dimColor>Log path: {newLivestockLogPath}</Text>}
          <Box marginTop={1}>
            <Text>Env path (optional): </Text>
            <TextInput
              value={newLivestockEnvPath}
              onChange={setNewLivestockEnvPath}
              onSubmit={saveLivestock}
              placeholder=".env"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Relative to livestock path. Enter: save livestock, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'delete-project-confirm') {
    const handleDeleteConfirm = () => {
      if (deleteConfirmInput === project.name) {
        onDeleteProject(project.name);
      }
    };
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="DELETE PROJECT" color="#ff0000" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">⚠️  Delete Project</Text>
          <Box marginTop={1}>
            <Text>This will permanently delete the project configuration.</Text>
          </Box>
          <Box marginTop={1}>
            <Text>Type the project name to confirm: </Text>
            <Text bold color="yellow">{project.name}</Text>
          </Box>
          <Box marginTop={1}>
            <Text>Confirm: </Text>
            <TextInput
              value={deleteConfirmInput}
              onChange={setDeleteConfirmInput}
              onSubmit={handleDeleteConfirm}
            />
          </Box>
          {deleteConfirmInput && deleteConfirmInput !== project.name && (
            <Box marginTop={1}>
              <Text color="red">Name does not match</Text>
            </Box>
          )}
          <Box marginTop={1}>
            <Text dimColor>Enter: delete (if name matches), Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Delete livestock confirmation
  if (mode === 'delete-livestock-confirm' && deleteLivestockTarget) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Remove livestock" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Remove Livestock</Text>
          <Box marginTop={1}>
            <Text>Remove "{deleteLivestockTarget.name}" from this project?</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Path: {deleteLivestockTarget.path}</Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, remove</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add herd mode
  if (mode === 'add-herd-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Adding herd" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Herd</Text>
          <Text dimColor>Herds group livestock and critters that work together</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newHerdName}
              onChange={setNewHerdName}
              onSubmit={() => {
                if (newHerdName.trim()) {
                  const herds = project.herds || [];
                  if (!herds.some((h) => h.name === newHerdName.trim())) {
                    const newHerd: Herd = {
                      name: newHerdName.trim(),
                      livestock: [],
                      critters: [],
                      connections: [],
                    };
                    onUpdateProject({
                      ...project,
                      herds: [...herds, newHerd],
                    });
                  }
                  setMode('normal');
                }
              }}
              placeholder="production, staging, client-a..."
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: create, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Delete herd confirmation
  if (mode === 'delete-herd-confirm' && deleteHerdTarget) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Remove herd" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Remove Herd</Text>
          <Box marginTop={1}>
            <Text>Remove "{deleteHerdTarget.name}" from this project?</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>
              {deleteHerdTarget.livestock.length} livestock, {deleteHerdTarget.critters.length} critters
            </Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, remove</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ranch hand creation: Name
  if (mode === 'add-ranchhand-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="New Ranch Hand" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Create Ranch Hand</Text>
          <Text dimColor>Ranch hands sync infrastructure from IaC tools into Yeehaw</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newRanchHandName}
              onChange={setNewRanchHandName}
              onSubmit={() => {
                if (newRanchHandName.trim()) {
                  setMode('add-ranchhand-type');
                }
              }}
              placeholder="prod-terraform"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: continue, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ranch hand creation: Type selection
  if (mode === 'add-ranchhand-type') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`New Ranch Hand: ${newRanchHandName}`} color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Select Type</Text>
          <Box marginTop={1} flexDirection="column" gap={1}>
            <Box>
              <Text color={newRanchHandType === 'terraform' ? 'cyan' : undefined}>
                {newRanchHandType === 'terraform' ? '› ' : '  '}Terraform
              </Text>
              <Text dimColor> - Sync from Terraform state (S3/local)</Text>
            </Box>
            <Box>
              <Text color={newRanchHandType === 'kubernetes' ? 'cyan' : undefined}>
                {newRanchHandType === 'kubernetes' ? '› ' : '  '}Kubernetes
              </Text>
              <Text dimColor> - Sync from K8s cluster contexts</Text>
            </Box>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>j/k: select, Enter: continue, Esc: back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ranch hand creation: Directory input
  if (mode === 'add-ranchhand-directory') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`New Ranch Hand: ${newRanchHandName} (${newRanchHandType})`} color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">{newRanchHandType === 'terraform' ? 'Scan Directory' : 'Scan Contexts'}</Text>
          <Text dimColor>
            {newRanchHandType === 'terraform'
              ? 'Enter path to your Terraform configuration'
              : 'Enter kubeconfig path (leave empty for default ~/.kube/config)'}
          </Text>
          <Box marginTop={1}>
            <Text>{newRanchHandType === 'terraform' ? 'Path: ' : 'Kubeconfig: '}</Text>
            <PathInput
              value={newRanchHandDirectory}
              onChange={setNewRanchHandDirectory}
              onSubmit={() => {
                if (newRanchHandDirectory.trim()) {
                  setScanError(null);
                  if (newRanchHandType === 'terraform') {
                    try {
                      const envs = detectTerraformEnvironments(newRanchHandDirectory);
                      if (envs.length === 0) {
                        setScanError('No Terraform configurations found in this directory.');
                      } else {
                        setDetectedEnvs(envs);
                        setSelectedEnvIndex(0);
                        setSelectedEnvs(new Set());
                        setMode('add-ranchhand-select-envs');
                      }
                    } catch (err) {
                      setScanError(err instanceof Error ? err.message : 'Scan failed');
                    }
                  } else {
                    // K8s: directory is optional kubeconfig path
                    try {
                      const kubeconfigPath = newRanchHandDirectory.trim() || undefined;
                      const contexts = getKubectlContexts(kubeconfigPath);
                      if (contexts.length === 0) {
                        setScanError('No kubectl contexts found. Is kubectl configured?');
                      } else {
                        setDetectedK8sContexts(contexts);
                        setSelectedEnvIndex(0);
                        setSelectedEnvs(new Set());
                        setMode('add-ranchhand-select-envs');
                      }
                    } catch (err) {
                      setScanError(err instanceof Error ? err.message : 'Failed to get kubectl contexts');
                    }
                  }
                }
              }}
            />
          </Box>
          {scanError && (
            <Box marginTop={1}>
              <Text color="red">{scanError}</Text>
            </Box>
          )}
          <Box marginTop={1}>
            <Text dimColor>Enter: scan, Esc: back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ranch hand creation: Select environments (works for both Terraform envs and K8s contexts)
  if (mode === 'add-ranchhand-select-envs') {
    const isTerraform = newRanchHandType === 'terraform';
    const items = isTerraform ? detectedEnvs : detectedK8sContexts;
    const envItems: ListItem[] = isTerraform
      ? detectedEnvs.map(env => ({
          id: env.id,
          label: `${selectedEnvs.has(env.id) ? '[x]' : '[ ]'} ${env.id}`,
          status: selectedEnvs.has(env.id) ? 'active' as const : 'inactive' as const,
          meta: env.backendType === 's3'
            ? `S3: ${env.bucket}/${env.key}`
            : `Local: ${env.localPath || 'default'}`,
        }))
      : detectedK8sContexts.map(ctx => ({
          id: ctx,
          label: `${selectedEnvs.has(ctx) ? '[x]' : '[ ]'} ${ctx}`,
          status: selectedEnvs.has(ctx) ? 'active' as const : 'inactive' as const,
          meta: 'kubectl context',
        }));

    const getEnvId = () => isTerraform
      ? detectedEnvs[selectedEnvIndex]?.id
      : detectedK8sContexts[selectedEnvIndex];

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`New Ranch Hand: ${newRanchHandName}`} color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">{isTerraform ? 'Select Environments' : 'Select Contexts'}</Text>
          <Text dimColor>
            Found {items.length} {isTerraform ? 'Terraform environment(s)' : 'kubectl context(s)'}. Select which to import:
          </Text>
          <Box marginTop={1} flexDirection="column" height={10}>
            <List
              items={envItems}
              focused={true}
              selectedIndex={selectedEnvIndex}
              onSelectionChange={setSelectedEnvIndex}
              onSelect={() => {
                const envId = getEnvId();
                if (envId) {
                  setSelectedEnvs(prev => {
                    const next = new Set(prev);
                    if (next.has(envId)) {
                      next.delete(envId);
                    } else {
                      next.add(envId);
                    }
                    return next;
                  });
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Space/Enter: toggle, c: continue ({selectedEnvs.size} selected), Esc: back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ranch hand creation: Map environment to herd
  if (mode === 'add-ranchhand-map-herd') {
    const envToMap = currentMappingEnv || Array.from(selectedEnvs)[0] || '';
    const existingHerds = project.herds || [];
    const herdItems: ListItem[] = [
      ...existingHerds.map(h => ({
        id: h.name,
        label: h.name,
        status: 'active' as const,
        meta: `${h.livestock.length} livestock, ${h.critters.length} critters`,
      })),
      { id: '__new__', label: '+ Create new herd', status: 'inactive' as const, meta: '' },
    ];

    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Assign: ${envToMap}`} color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Map to Herd</Text>
          <Text dimColor>Which herd should "{envToMap}" sync to?</Text>
          <Box marginTop={1} flexDirection="column" height={8}>
            <List
              items={herdItems}
              focused={true}
              selectedIndex={0}
              onSelect={(item) => {
                if (item.id === '__new__') {
                  setNewHerdNameForEnv('');
                  setMode('add-ranchhand-new-herd');
                } else {
                  // Assign and move to next env or finish
                  const newMappings = new Map(envHerdMappings);
                  newMappings.set(envToMap, item.id);
                  setEnvHerdMappings(newMappings);

                  const remaining = Array.from(selectedEnvs).filter(
                    e => e !== envToMap && !newMappings.has(e)
                  );
                  if (remaining.length > 0) {
                    setCurrentMappingEnv(remaining[0]);
                  } else {
                    // All mapped, finish creation
                    finishRanchHandCreation(newMappings);
                  }
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: select, Esc: back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Ranch hand creation: New herd name
  if (mode === 'add-ranchhand-new-herd') {
    const envToMap = currentMappingEnv || Array.from(selectedEnvs)[0] || '';
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle="Create New Herd" color={project.color} gradientSpread={project.gradientSpread} gradientInverted={project.gradientInverted} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">New Herd</Text>
          <Text dimColor>Enter name for new herd (for "{envToMap}")</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newHerdNameForEnv}
              onChange={setNewHerdNameForEnv}
              onSubmit={() => {
                if (newHerdNameForEnv.trim()) {
                  const newMappings = new Map(envHerdMappings);
                  newMappings.set(envToMap, newHerdNameForEnv.trim());
                  setEnvHerdMappings(newMappings);

                  const remaining = Array.from(selectedEnvs).filter(
                    e => e !== envToMap && !newMappings.has(e)
                  );
                  if (remaining.length > 0) {
                    setCurrentMappingEnv(remaining[0]);
                    setMode('add-ranchhand-map-herd');
                  } else {
                    finishRanchHandCreation(newMappings);
                  }
                }
              }}
              placeholder="production"
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: create herd, Esc: back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Get livestock with resolved barn info
  const livestockWithBarns = (project.livestock || []).map((livestock) => ({
    livestock,
    barn: livestock.barn ? barns.find((b) => b.name === livestock.barn) || null : null,
  }));

  const livestockItems: ListItem[] = livestockWithBarns.map(({ livestock, barn }) => {
    const isLocal = !barn || isLocalBarn(barn);

    // Local livestock: claude + shell
    // Remote livestock: shell only
    const actions: RowAction[] = isLocal
      ? [{ key: 'c', label: 'claude' }, { key: 's', label: 'shell' }]
      : [{ key: 's', label: 'shell' }];

    return {
      id: livestock.name,
      label: barn ? `${livestock.name} (${barn.host})` : `${livestock.name} (local)`,
      status: 'active',
      meta: livestock.path,
      actions,
    };
  });

  // Parse session name for type hint (consistent with GlobalDashboard)
  const getSessionTypeHint = (name: string): string => {
    if (name.endsWith('-claude')) return 'claude';
    return 'shell';
  };

  // Use display numbers (1-9) instead of tmux window index
  const sessionItems: ListItem[] = projectWindows.map((w, i) => {
    const sessionName = w.name.replace(`${project.name}-`, '');
    const typeHint = getSessionTypeHint(w.name);
    const displayName = sessionName.replace('-claude', '');
    const statusInfo = getWindowStatus(w);
    return {
      id: String(w.index),
      label: `[${i + 1}] ${displayName}`,
      status: w.active ? 'active' : 'inactive',
      meta: `${typeHint} · ${statusInfo.text}`,
      sessionStatus: statusInfo.status,
    };
  });

  // Build herd list items
  const herdItems: ListItem[] = (project.herds || []).map((herd) => ({
    id: herd.name,
    label: herd.name,
    status: 'active',
    meta: `${herd.livestock.length} livestock, ${herd.critters.length} critters`,
  }));

  // Build ranch hand list items
  const ranchHandItems: ListItem[] = ranchHands.map((ranchhand) => ({
    id: ranchhand.name,
    label: ranchhand.name,
    status: ranchhand.last_sync ? 'active' : 'inactive',
    meta: `${ranchhand.type} • ${ranchhand.herd || '(no herd)'}`,
  }));

  // Panel-specific hints (page-level hotkeys like c/w/i are in BottomBar)
  const livestockHints = '[n] new  [d] delete';
  const herdHints = '[n] new  [d] delete';
  const ranchHandHints = '[n] new';
  const sessionHints = '';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header
        text={project.name}
        subtitle={project.path}
        summary={project.summary}
        color={project.color}
        gradientSpread={project.gradientSpread}
        gradientInverted={project.gradientInverted}
      />

      <Box flexGrow={1} marginY={1} paddingX={1} flexDirection="column" gap={1}>
        {/* Top row: Livestock and Sessions side by side */}
        <Box flexGrow={1} gap={2}>
          {/* Left: Livestock */}
          <Panel
            title="Livestock"
            focused={focusedPanel === 'livestock'}
            width="50%"
            hints={livestockHints}
          >
            {livestockItems.length > 0 ? (
              <List
                items={livestockItems}
                focused={focusedPanel === 'livestock'}
                selectedIndex={selectedLivestockIndex}
                onSelectionChange={setSelectedLivestockIndex}
                onSelect={(item) => {
                  const found = livestockWithBarns.find((l) => l.livestock.name === item.id);
                  if (found) {
                    onSelectLivestock(found.livestock, found.barn);
                  }
                }}
                onAction={(item, actionKey) => {
                  const found = livestockWithBarns.find((l) => l.livestock.name === item.id);
                  if (!found) return;

                  if (actionKey === 's') {
                    onOpenLivestockSession(found.livestock, found.barn);
                  }
                  if (actionKey === 'c') {
                    onNewClaudeForLivestock(found.livestock);
                  }
                }}
              />
            ) : (
              <Box flexDirection="column">
                <Text dimColor>No livestock configured</Text>
                <Text dimColor italic>Livestock are your deployed app instances</Text>
              </Box>
            )}
          </Panel>

          {/* Right: Sessions */}
          <Panel
            title="Sessions"
            focused={focusedPanel === 'sessions'}
            width="50%"
            hints={sessionHints}
          >
            {sessionItems.length > 0 ? (
              <List
                items={sessionItems}
                focused={focusedPanel === 'sessions'}
                onSelect={(item) => {
                  const window = projectWindows.find((w) => String(w.index) === item.id);
                  if (window) onSelectWindow(window);
                }}
              />
            ) : (
              <Text dimColor>No active sessions</Text>
            )}
          </Panel>
        </Box>

        {/* Bottom row: Herds and Ranch Hands side by side */}
        <Box flexGrow={1} gap={2}>
          {/* Left: Herds panel */}
          <Panel
            title="Herds"
            focused={focusedPanel === 'herds'}
            width="50%"
            hints={herdHints}
          >
            {herdItems.length > 0 ? (
              <List
                items={herdItems}
                focused={focusedPanel === 'herds'}
                selectedIndex={selectedHerdIndex}
                onSelectionChange={setSelectedHerdIndex}
                onSelect={(item) => {
                  const herd = (project.herds || []).find((h) => h.name === item.id);
                  if (herd) {
                    onSelectHerd(herd);
                  }
                }}
              />
            ) : (
              <Box flexDirection="column">
                <Text dimColor>No herds configured</Text>
                <Text dimColor italic>Herds group livestock + critters</Text>
              </Box>
            )}
          </Panel>

          {/* Right: Ranch Hands panel */}
          <Panel
            title="Ranch Hands"
            focused={focusedPanel === 'ranchhands'}
            width="50%"
            hints={ranchHandHints}
          >
            {ranchHandItems.length > 0 ? (
              <List
                items={ranchHandItems}
                focused={focusedPanel === 'ranchhands'}
                selectedIndex={selectedRanchHandIndex}
                onSelectionChange={setSelectedRanchHandIndex}
                onSelect={(item) => {
                  const ranchhand = ranchHands.find((r) => r.name === item.id);
                  if (ranchhand) {
                    onSelectRanchHand(ranchhand);
                  }
                }}
              />
            ) : (
              <Box flexDirection="column">
                <Text dimColor>No ranch hands configured</Text>
                <Text dimColor italic>Create via MCP tools</Text>
              </Box>
            )}
          </Panel>
        </Box>
      </Box>
    </Box>
  );
}
