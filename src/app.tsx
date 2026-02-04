import React, { useState, useCallback, useMemo, useEffect } from 'react';
import { Box, Text, useApp, useInput, useStdout } from 'ink';
import { homedir } from 'os';
import { join } from 'path';
import { HelpOverlay } from './components/HelpOverlay.js';
import { BottomBar } from './components/BottomBar.js';
import { SplashScreen } from './components/SplashScreen.js';
import { ClaudeSplashScreen } from './components/ClaudeSplashScreen.js';
import { GlobalDashboard } from './views/GlobalDashboard.js';
import { ProjectContext } from './views/ProjectContext.js';
import { BarnContext } from './views/BarnContext.js';
import { WikiView } from './views/WikiView.js';
import { IssuesView } from './views/IssuesView.js';
import { LivestockDetailView } from './views/LivestockDetailView.js';
import { LogsView } from './views/LogsView.js';
import { CritterDetailView } from './views/CritterDetailView.js';
import { CritterLogsView } from './views/CritterLogsView.js';
import { NightSkyView } from './views/NightSkyView.js';
import { HerdDetailView } from './views/HerdDetailView.js';
import { RanchHandDetailView } from './views/RanchHandDetailView.js';
import { useConfig } from './hooks/useConfig.js';
import { useSessions } from './hooks/useSessions.js';
import { useRemoteYeehaw } from './hooks/useRemoteYeehaw.js';
import {
  hasTmux,
  switchToWindow,
  updateStatusBar,
  createShellWindow,
  createSshWindow,
  detachFromSession,
  killYeehawSession,
  restartYeehaw,
  enterRemoteMode,
  ensureCorrectStatusBar,
  createClaudeWindowWithPrompt,
  killWindow,
  getWindowStatus,
  YEEHAW_MCP_TOOLS,
} from './lib/tmux.js';
import { saveProject, deleteProject, saveBarn, deleteBarn, getLivestockForBarn, isLocalBarn, hasValidSshConfig, addCritterToBarn, removeCritterFromBarn, saveRanchHand, loadRanchHandsForProject, updateRanchHandLastSync, loadBarn } from './lib/config.js';
import { syncK8sResources } from './lib/ranchhand-k8s.js';
import { syncTerraformResources } from './lib/ranchhand-terraform.js';
import { buildProjectContext, buildLivestockContext } from './lib/context.js';
import { getVersionInfo } from './lib/update-check.js';
import type { AppView, Project, Barn, Livestock, Critter, Herd, RanchHand, EntitySource, NightSkyContext, VisualizerSession } from './types.js';
import type { TmuxWindow } from './lib/tmux.js';
import type { HotkeyScope } from './lib/hotkeys.js';

function getHotkeyScope(view: AppView): HotkeyScope {
  switch (view.type) {
    case 'global': return 'global-dashboard';
    case 'project': return 'project-context';
    case 'barn': return 'barn-context';
    case 'wiki': return 'wiki-view';
    case 'issues': return 'issues-view';
    case 'livestock': return 'livestock-detail';
    case 'logs': return 'logs-view';
    case 'critter': return 'critter-detail';
    case 'critter-logs': return 'critter-logs';
    case 'herd': return 'herd-detail';
    case 'ranchhand': return 'ranchhand-detail';
    case 'night-sky': return 'night-sky';
    default: return 'global-dashboard';
  }
}

function expandPath(path: string): string {
  if (path.startsWith('~/')) {
    return join(homedir(), path.slice(2));
  }
  return path;
}

// Global bottom bar items - minimal, consistent across all views
// Every hotkey should be visible somewhere in our 3-tier system
function getBottomBarItems(viewType: AppView['type'], options?: { isLocalLivestock?: boolean; isLocalBarn?: boolean }): Array<{ key: string; label: string }> {
  // Night sky has its own unique actions
  if (viewType === 'night-sky') {
    return [
      { key: 'c', label: 'cloud' },
      { key: 'r', label: 'randomize' },
      { key: 'Esc', label: 'exit' },
    ];
  }

  // Global dashboard: exit options + visualizer
  if (viewType === 'global') {
    return [
      { key: 'v', label: 'visualizer' },
      { key: 'q', label: 'detach' },
      { key: 'Q', label: 'quit' },
      { key: 'Tab', label: '' },
      { key: '?', label: 'help' },
    ];
  }

  // Project context: page-level actions
  if (viewType === 'project') {
    return [
      { key: 'v', label: 'visualizer' },
      { key: 'w', label: 'wiki' },
      { key: 'i', label: 'issues' },
      { key: 'e', label: 'edit' },
      { key: 'Esc', label: 'back' },
      { key: 'Tab', label: '' },
      { key: '?', label: 'help' },
    ];
  }

  // Barn context: page-level actions (edit only for remote barns)
  if (viewType === 'barn') {
    const items: Array<{ key: string; label: string }> = [
      { key: 'v', label: 'visualizer' },
    ];
    if (!options?.isLocalBarn) {
      items.push({ key: 'e', label: 'edit' });
    }
    items.push(
      { key: 'Esc', label: 'back' },
      { key: 'Tab', label: '' },
      { key: '?', label: 'help' },
    );
    return items;
  }

  // Wiki view: panel hints handle n/e/d, bottom bar just needs navigation
  if (viewType === 'wiki') {
    return [
      { key: 'Esc', label: 'back' },
      { key: 'Tab', label: '' },
      { key: '?', label: 'help' },
    ];
  }

  // Issues view: page-level actions (f/r are page-level, c/o are row-level shown on selected item)
  if (viewType === 'issues') {
    return [
      { key: 'f', label: 'filter' },
      { key: 'r', label: 'refresh' },
      { key: 'Esc', label: 'back' },
      { key: 'Tab', label: '' },
      { key: '?', label: 'help' },
    ];
  }

  // Livestock detail: page-level actions
  // Local livestock gets [c] claude, remote only gets [s] shell
  if (viewType === 'livestock') {
    const items: Array<{ key: string; label: string }> = [
      { key: 'v', label: 'visualizer' },
    ];
    if (options?.isLocalLivestock) {
      items.push({ key: 'c', label: 'claude' });
    }
    items.push(
      { key: 's', label: 'shell' },
      { key: 'l', label: 'logs' },
      { key: 'e', label: 'edit' },
      { key: 'Esc', label: 'back' },
      { key: '?', label: 'help' },
    );
    return items;
  }

  // Logs view: page-level actions
  if (viewType === 'logs') {
    return [
      { key: 'r', label: 'refresh' },
      { key: 'Esc', label: 'back' },
      { key: '?', label: 'help' },
    ];
  }

  // Critter detail: page-level actions
  if (viewType === 'critter') {
    return [
      { key: 'v', label: 'visualizer' },
      { key: 'l', label: 'logs' },
      { key: 'e', label: 'edit' },
      { key: 'Esc', label: 'back' },
      { key: '?', label: 'help' },
    ];
  }

  // Critter logs view: page-level actions
  if (viewType === 'critter-logs') {
    return [
      { key: 'r', label: 'refresh' },
      { key: 'Esc', label: 'back' },
      { key: '?', label: 'help' },
    ];
  }

  // Herd detail view: page-level actions
  if (viewType === 'herd') {
    return [
      { key: 'Tab', label: 'switch panel' },
      { key: 'n', label: 'add' },
      { key: 'd', label: 'remove' },
      { key: 'Esc', label: 'back' },
      { key: '?', label: 'help' },
    ];
  }

  // Ranch hand detail view: page-level actions
  if (viewType === 'ranchhand') {
    return [
      { key: 'r', label: 'refresh' },
      { key: 'space', label: 'toggle' },
      { key: 's', label: 'sync' },
      { key: 'Esc', label: 'back' },
      { key: '?', label: 'help' },
    ];
  }

  // Fallback (shouldn't reach here)
  return [
    { key: 'Esc', label: 'back' },
    { key: '?', label: 'help' },
  ];
}

export function App() {
  const { exit } = useApp();
  const { projects, barns, reload } = useConfig();
  const { windows, createClaude, attachToWindow } = useSessions();
  const { stdout } = useStdout();
  const { environments, isDetecting } = useRemoteYeehaw(barns);

  const [showSplash, setShowSplash] = useState(true);
  const [view, setView] = useState<AppView>({ type: 'global' });
  const [previousView, setPreviousView] = useState<AppView | null>(null);
  const [showHelp, setShowHelp] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingGo, setPendingGo] = useState(false); // For g+number sequence
  const [isChildInputMode, setIsChildInputMode] = useState(false); // Track when child views have text input active
  const [claudeSplash, setClaudeSplash] = useState<{
    windowIndex: number;
    systemPrompt: string;
    mcpTools: string[];
    projectColor?: string;
  } | null>(null); // Claude session splash screen state

  // Get terminal height for full-height layout
  const terminalHeight = stdout?.rows || 24;

  // Check tmux availability
  const tmuxAvailable = hasTmux();

  // Get version info (cached, so safe to call synchronously)
  const versionInfo = useMemo(() => getVersionInfo(), []);

  // Ensure status bar is hidden when on global dashboard
  useEffect(() => {
    if (view.type === 'global') {
      ensureCorrectStatusBar();
    }
  }, [view.type]);

  // Hot-reload: sync view state when projects/barns change (e.g., from MCP server updates)
  useEffect(() => {
    setView((currentView) => {
      // Update project data in views that contain a project
      if (currentView.type === 'project' || currentView.type === 'wiki' || currentView.type === 'issues') {
        const freshProject = projects.find((p) => p.name === currentView.project.name);
        if (freshProject && freshProject !== currentView.project) {
          return { ...currentView, project: freshProject };
        }
      }
      // Update livestock detail view
      if (currentView.type === 'livestock' || currentView.type === 'logs') {
        const freshProject = projects.find((p) => p.name === currentView.project.name);
        if (freshProject && freshProject !== currentView.project) {
          const freshLivestock = (freshProject.livestock || []).find((l) => l.name === currentView.livestock.name);
          if (freshLivestock) {
            return { ...currentView, project: freshProject, livestock: freshLivestock };
          }
        }
      }
      // Update barn data in barn view
      if (currentView.type === 'barn') {
        const freshBarn = barns.find((b) => b.name === currentView.barn.name);
        if (freshBarn && freshBarn !== currentView.barn) {
          return { ...currentView, barn: freshBarn };
        }
      }
      // Update critter detail view
      if (currentView.type === 'critter' || currentView.type === 'critter-logs') {
        const freshBarn = barns.find((b) => b.name === currentView.barn.name);
        if (freshBarn && freshBarn !== currentView.barn) {
          const freshCritter = (freshBarn.critters || []).find((c) => c.name === currentView.critter.name);
          if (freshCritter) {
            return { ...currentView, barn: freshBarn, critter: freshCritter };
          }
        }
      }
      // Update herd detail view
      if (currentView.type === 'herd') {
        const freshProject = projects.find((p) => p.name === currentView.project.name);
        if (freshProject && freshProject !== currentView.project) {
          const freshHerd = (freshProject.herds || []).find((h) => h.name === currentView.herd.name);
          if (freshHerd) {
            return { ...currentView, project: freshProject, herd: freshHerd };
          }
        }
      }
      // Update ranch hand detail view
      if (currentView.type === 'ranchhand') {
        const freshProject = projects.find((p) => p.name === currentView.project.name);
        const ranchhands = loadRanchHandsForProject(currentView.project.name);
        const freshRanchHand = ranchhands.find((r) => r.name === currentView.ranchhand.name);
        if (freshProject && freshRanchHand) {
          return { ...currentView, project: freshProject, ranchhand: freshRanchHand };
        }
      }
      return currentView;
    });
  }, [projects, barns]);

  const handleSelectProject = useCallback((project: Project) => {
    setView({ type: 'project', project });
    updateStatusBar(project.name);
  }, []);

  const handleSelectBarn = useCallback((barn: Barn) => {
    setView({ type: 'barn', barn });
    updateStatusBar(`Barn: ${barn.name}`);
  }, []);

  const handleBack = useCallback(() => {
    setView({ type: 'global' });
    updateStatusBar();
  }, []);

  const handleSelectWindow = useCallback((window: TmuxWindow) => {
    attachToWindow(window.index);
  }, [attachToWindow]);

  const handleNewClaude = useCallback(() => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }
    try {
      const projectName = view.type === 'project' ? view.project.name : 'yeehaw';
      const workingDir = view.type === 'project' ? expandPath(view.project.path) : process.cwd();
      const windowName = `${projectName}-claude`;
      const projectColor = view.type === 'project' ? view.project.color : undefined;

      // Inject context when in a project view
      const context = view.type === 'project' ? buildProjectContext(view.project.name) : null;
      const windowIndex = context
        ? createClaudeWindowWithPrompt(workingDir, windowName, context)
        : createClaude(workingDir, windowName);

      // Show splash screen instead of immediately switching
      setClaudeSplash({
        windowIndex,
        systemPrompt: context || '',
        mcpTools: YEEHAW_MCP_TOOLS,
        projectColor,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(`Failed to create Claude session: ${message}`);
    }
  }, [tmuxAvailable, view, createClaude]);

  const handleNewClaudeForProject = useCallback((project: Project) => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }
    try {
      const workingDir = expandPath(project.path);
      const windowName = `${project.name}-claude`;
      const context = buildProjectContext(project.name);

      // Use context injection if we have project context, otherwise fall back to basic
      const windowIndex = context
        ? createClaudeWindowWithPrompt(workingDir, windowName, context)
        : createClaude(workingDir, windowName);

      // Show splash screen instead of immediately switching
      setClaudeSplash({
        windowIndex,
        systemPrompt: context || '',
        mcpTools: YEEHAW_MCP_TOOLS,
        projectColor: project.color,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(`Failed to create Claude session: ${message}`);
    }
  }, [tmuxAvailable, createClaude]);

  const handleNewClaudeForLivestock = useCallback((livestock: Livestock, projectName: string) => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }
    try {
      const workingDir = expandPath(livestock.path);
      const windowName = `${projectName}-${livestock.name}-claude`;
      const context = buildLivestockContext(projectName, livestock.name);
      const project = projects.find((p) => p.name === projectName);

      // Use context injection if we have project context, otherwise fall back to basic
      const windowIndex = context
        ? createClaudeWindowWithPrompt(workingDir, windowName, context)
        : createClaude(workingDir, windowName);

      // Show splash screen instead of immediately switching
      setClaudeSplash({
        windowIndex,
        systemPrompt: context || '',
        mcpTools: YEEHAW_MCP_TOOLS,
        projectColor: project?.color,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(`Failed to create Claude session: ${message}`);
    }
  }, [tmuxAvailable, createClaude, projects]);

  const handleOpenClaudeWithContext = useCallback((workingDir: string, issueContext: string) => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }
    try {
      // Get the project name and color for the window name
      const project = view.type === 'issues' ? view.project : null;
      const projectName = project?.name || 'issue';
      const windowName = `${projectName}-claude`;

      // Create claude window with system prompt
      const windowIndex = createClaudeWindowWithPrompt(workingDir, windowName, issueContext);

      // Show splash screen instead of immediately switching
      setClaudeSplash({
        windowIndex,
        systemPrompt: issueContext,
        mcpTools: YEEHAW_MCP_TOOLS,
        projectColor: project?.color,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(`Failed to create Claude session: ${message}`);
    }
  }, [tmuxAvailable, view]);

  const handleOpenLivestockSession = useCallback((livestock: Livestock, barn: Barn | null, projectName: string) => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }

    const windowName = `${projectName}-${livestock.name}`;

    if (barn && !isLocalBarn(barn)) {
      // Remote livestock - SSH into it
      if (!hasValidSshConfig(barn)) {
        setError(`Barn '${barn.name}' is missing SSH configuration`);
        return;
      }
      const windowIndex = createSshWindow(
        windowName,
        barn.host,
        barn.user,
        barn.port,
        barn.identity_file,
        livestock.path
      );
      switchToWindow(windowIndex);
    } else {
      // Local livestock - open shell window
      const workingDir = expandPath(livestock.path);
      const windowIndex = createShellWindow(workingDir, windowName);
      switchToWindow(windowIndex);
    }
  }, [tmuxAvailable]);

  const handleCreateProject = useCallback((name: string, path: string) => {
    const project: Project = {
      name,
      path,
      livestock: [],
    };
    saveProject(project);
    reload();
  }, [reload]);

  const handleUpdateProject = useCallback((updatedProject: Project) => {
    saveProject(updatedProject);
    reload();
    // Update the view with the new project data, preserving wiki view if active
    setView((currentView) => {
      if (currentView.type === 'wiki') {
        return { type: 'wiki', project: updatedProject };
      }
      return { type: 'project', project: updatedProject };
    });
  }, [reload]);

  const handleOpenWiki = useCallback((project: Project) => {
    setView({ type: 'wiki', project });
    updateStatusBar(`${project.name} Wiki`);
  }, []);

  const handleOpenIssues = useCallback((project: Project) => {
    setView({ type: 'issues', project });
    updateStatusBar(`${project.name} Issues`);
  }, []);

  const handleBackFromSubview = useCallback((project: Project) => {
    setView({ type: 'project', project });
    updateStatusBar(project.name);
  }, []);

  const handleOpenLivestockDetail = useCallback((project: Project, livestock: Livestock, source: 'project' | 'barn', sourceBarn?: Barn) => {
    setView({ type: 'livestock', project, livestock, source, sourceBarn });
    updateStatusBar(`${project.name} / ${livestock.name}`);
  }, []);

  const handleOpenLogs = useCallback((project: Project, livestock: Livestock, source: 'project' | 'barn', sourceBarn?: Barn) => {
    setView({ type: 'logs', project, livestock, source, sourceBarn });
    updateStatusBar(`${project.name} / ${livestock.name} Logs`);
  }, []);

  const handleBackFromLivestock = useCallback((source: 'project' | 'barn', project: Project, sourceBarn?: Barn) => {
    if (source === 'barn' && sourceBarn) {
      setView({ type: 'barn', barn: sourceBarn });
      updateStatusBar(`Barn: ${sourceBarn.name}`);
    } else {
      setView({ type: 'project', project });
      updateStatusBar(project.name);
    }
  }, []);

  const handleUpdateLivestock = useCallback((project: Project, originalLivestock: Livestock, updatedLivestock: Livestock, source: 'project' | 'barn', sourceBarn?: Barn) => {
    const updatedProject = {
      ...project,
      livestock: (project.livestock || []).map((l) =>
        l.name === originalLivestock.name ? updatedLivestock : l
      ),
    };
    saveProject(updatedProject);
    reload();
    // Update the view with the new livestock data, preserving navigation context
    setView({ type: 'livestock', project: updatedProject, livestock: updatedLivestock, source, sourceBarn });
  }, [reload]);

  const handleOpenCritterDetail = useCallback((barn: Barn, critter: Critter) => {
    setView({ type: 'critter', barn, critter });
    updateStatusBar(`${barn.name} / ${critter.name}`);
  }, []);

  const handleOpenCritterLogs = useCallback((barn: Barn, critter: Critter) => {
    setView({ type: 'critter-logs', barn, critter });
    updateStatusBar(`${barn.name} / ${critter.name} Logs`);
  }, []);

  const handleBackFromCritter = useCallback((barn: Barn) => {
    setView({ type: 'barn', barn });
    updateStatusBar(`Barn: ${barn.name}`);
  }, []);

  const handleOpenHerdDetail = useCallback((project: Project, herd: Herd) => {
    setView({ type: 'herd', project, herd });
    updateStatusBar(`${project.name} / ${herd.name}`);
  }, []);

  const handleBackFromHerd = useCallback((project: Project) => {
    setView({ type: 'project', project });
    updateStatusBar(`Project: ${project.name}`);
  }, []);

  const handleUpdateHerd = useCallback((project: Project, herd: Herd) => {
    // Find and update the herd in the project
    const herds = project.herds || [];
    const herdIndex = herds.findIndex((h) => h.name === herd.name);
    if (herdIndex >= 0) {
      const updatedHerds = [...herds];
      updatedHerds[herdIndex] = herd;
      const updatedProject = { ...project, herds: updatedHerds };
      saveProject(updatedProject);
      reload();
      // Update view with fresh data
      setView({ type: 'herd', project: updatedProject, herd });
    }
  }, [reload]);

  const handleOpenRanchHandDetail = useCallback((project: Project, ranchhand: RanchHand) => {
    setView({ type: 'ranchhand', project, ranchhand });
    updateStatusBar(`${project.name} / ${ranchhand.name}`);
  }, []);

  const handleBackFromRanchHand = useCallback((project: Project) => {
    setView({ type: 'project', project });
    updateStatusBar(`Project: ${project.name}`);
  }, []);

  const handleUpdateRanchHand = useCallback((ranchhand: RanchHand) => {
    saveRanchHand(ranchhand);
    reload();
  }, [reload]);

  const handleSyncRanchHand = useCallback((project: Project, ranchhand: RanchHand) => {
    const sourceTag: EntitySource = `ranchhand:${ranchhand.name}`;

    // Remove existing entities from this ranch hand before re-syncing
    const cleanedLivestock = (project.livestock || []).filter(ls => ls.source !== sourceTag);
    const cleanedHerds = (project.herds || []).filter(h => {
      // Keep herds that have any manual content
      const hasManualLivestock = h.livestock.some(lsName =>
        cleanedLivestock.find(ls => ls.name === lsName && ls.source !== sourceTag)
      );
      return hasManualLivestock || h.critters.length > 0;
    });

    // Perform sync based on ranch hand type
    if (ranchhand.type === 'kubernetes') {
      const result = syncK8sResources(ranchhand);

      // Save new barns (skip if already exists)
      for (const barn of result.barns) {
        const existing = loadBarn(barn.name);
        if (!existing) {
          saveBarn(barn);
        }
      }

      // Build a map of critter -> barn from herd refs
      const critterToBarn = new Map<string, string>();
      for (const herd of result.herds) {
        for (const ref of herd.critters) {
          critterToBarn.set(ref.critter, ref.barn);
        }
      }

      // Add critters to their respective barns
      for (const critter of result.critters) {
        const barnName = critterToBarn.get(critter.name);
        if (barnName) {
          const barn = loadBarn(barnName);
          if (barn) {
            // Check if critter already exists
            const existingCritter = barn.critters?.find(c => c.name === critter.name);
            if (!existingCritter) {
              addCritterToBarn(barnName, critter);
            }
          }
        }
      }

      // Merge livestock into project
      const mergedLivestock = [...cleanedLivestock, ...result.livestock];

      // Merge herds into project
      const mergedHerds = [...cleanedHerds];
      for (const herd of result.herds) {
        const existingIdx = mergedHerds.findIndex(h => h.name === herd.name);
        if (existingIdx >= 0) {
          // Merge with existing herd
          const existing = mergedHerds[existingIdx];
          mergedHerds[existingIdx] = {
            ...existing,
            livestock: [...new Set([...existing.livestock, ...herd.livestock])],
            critters: [...existing.critters, ...herd.critters.filter(
              newRef => !existing.critters.some(
                existRef => existRef.barn === newRef.barn && existRef.critter === newRef.critter
              )
            )],
          };
        } else {
          mergedHerds.push(herd);
        }
      }

      // Save updated project
      const updatedProject = { ...project, livestock: mergedLivestock, herds: mergedHerds };
      saveProject(updatedProject);

    } else {
      const result = syncTerraformResources(ranchhand);

      // Save new barns (skip if already exists with different source)
      for (const barn of result.barns) {
        const existing = loadBarn(barn.name);
        if (!existing) {
          saveBarn(barn);
        } else if (existing.source === sourceTag) {
          // Update if it's from the same ranch hand
          saveBarn({ ...existing, ...barn, critters: existing.critters });
        }
      }

      // For Terraform, create/update a synthetic barn for critters
      if (result.critters.length > 0) {
        const tfBarnName = `terraform-${ranchhand.name}`;
        const existingTfBarn = loadBarn(tfBarnName);

        const tfBarn: Barn = existingTfBarn ?? {
          name: tfBarnName,
          source: sourceTag,
          connection_type: 'terraform',
          connectable: false,
          critters: [],
        };

        // Only modify if this barn belongs to this ranch hand (or is new)
        if (!tfBarn.source || tfBarn.source === sourceTag) {
          // Remove old critters from this ranch hand
          const cleanedCritters = (tfBarn.critters || []).filter(c => c.source !== sourceTag);
          tfBarn.critters = [...cleanedCritters, ...result.critters];
          tfBarn.source = sourceTag;
          saveBarn(tfBarn);
        }
      }

      // Terraform doesn't produce livestock, just save cleaned project
      const updatedProject = { ...project, livestock: cleanedLivestock, herds: cleanedHerds };
      saveProject(updatedProject);
    }

    // Update last sync time
    updateRanchHandLastSync(ranchhand.name);
    reload();
    // Go back to project view
    const freshProject = projects.find((p) => p.name === project.name);
    if (freshProject) {
      setView({ type: 'project', project: freshProject });
      updateStatusBar(`Project: ${freshProject.name}`);
    }
  }, [projects, reload]);

  const handleUpdateCritter = useCallback((barn: Barn, originalCritter: Critter, updatedCritter: Critter) => {
    // Remove old critter and add updated one
    removeCritterFromBarn(barn.name, originalCritter.name);
    addCritterToBarn(barn.name, updatedCritter);
    reload();
    // Update view with new critter data
    setView({ type: 'critter', barn, critter: updatedCritter });
  }, [reload]);

  const handleDeleteProject = useCallback((projectName: string) => {
    deleteProject(projectName);
    reload();
    // Go back to global view after deletion
    setView({ type: 'global' });
    updateStatusBar();
  }, [reload]);

  const handleCreateBarn = useCallback((barn: Barn) => {
    saveBarn(barn);
    reload();
  }, [reload]);

  const handleUpdateBarn = useCallback((updatedBarn: Barn) => {
    saveBarn(updatedBarn);
    reload();
    // Update the view with the new barn data
    setView({ type: 'barn', barn: updatedBarn });
  }, [reload]);

  const handleDeleteBarn = useCallback((barnName: string) => {
    deleteBarn(barnName);
    reload();
    // Go back to global view after deletion
    setView({ type: 'global' });
    updateStatusBar();
  }, [reload]);

  const handleSshToBarn = useCallback((barn: Barn) => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }
    if (isLocalBarn(barn)) {
      // Local barn - just open a shell in home directory
      const windowName = `barn-${barn.name}`;
      const windowIndex = createShellWindow(homedir(), windowName);
      switchToWindow(windowIndex);
      return;
    }
    if (!hasValidSshConfig(barn)) {
      setError(`Barn '${barn.name}' is missing SSH configuration`);
      return;
    }
    const windowName = `barn-${barn.name}`;
    const windowIndex = createSshWindow(
      windowName,
      barn.host,
      barn.user,
      barn.port,
      barn.identity_file,
      '~'  // SSH to home directory
    );
    switchToWindow(windowIndex);
  }, [tmuxAvailable]);

  const handleEnterNightSky = useCallback(() => {
    setPreviousView(view);
    setView({ type: 'night-sky' });
    updateStatusBar('Night Sky');
  }, [view]);

  const handleExitNightSky = useCallback(() => {
    if (previousView) {
      setView(previousView);
      setPreviousView(null);
      // Restore status bar based on previous view
      if (previousView.type === 'project') {
        updateStatusBar(previousView.project.name);
      } else if (previousView.type === 'barn') {
        updateStatusBar(`Barn: ${previousView.barn.name}`);
      } else {
        updateStatusBar();
      }
    } else {
      setView({ type: 'global' });
      updateStatusBar();
    }
  }, [previousView]);

  // Build context for NightSkyView based on view state
  const buildNightSkyContext = useCallback((srcView: AppView, allWindows: TmuxWindow[]): NightSkyContext | undefined => {
    // Helper to convert TmuxWindow to VisualizerSession
    const toVisualizerSession = (w: TmuxWindow): VisualizerSession => {
      const status = getWindowStatus(w);
      return {
        index: w.index,
        name: w.name,
        type: w.type === 'claude' ? 'claude' : 'shell',
        statusText: status.text,
        statusIcon: status.icon,
      };
    };

    // Filter windows based on context
    const filterSessions = (nameFilter?: string): VisualizerSession[] => {
      if (!nameFilter) return allWindows.map(toVisualizerSession);
      const lowerFilter = nameFilter.toLowerCase();
      return allWindows
        .filter(w => w.name.toLowerCase().includes(lowerFilter))
        .map(toVisualizerSession);
    };

    switch (srcView.type) {
      case 'global':
        return { type: 'global', sessions: allWindows.map(toVisualizerSession) };
      case 'project':
        return {
          type: 'project',
          project: srcView.project,
          sessions: filterSessions(srcView.project.name),
        };
      case 'livestock':
        return {
          type: 'livestock',
          project: srcView.project,
          livestock: srcView.livestock,
          sessions: filterSessions(srcView.livestock.name),
        };
      case 'barn':
        return { type: 'barn', barn: srcView.barn, sessions: [] };
      case 'critter':
        return { type: 'critter', barn: srcView.barn, critter: srcView.critter, sessions: [] };
      default:
        return undefined;
    }
  }, []);

  const handleConnectToRemote = useCallback((envIndex: number) => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }

    const env = environments[envIndex];
    if (!env || env.state !== 'available') {
      setError('Remote Yeehaw not available');
      return;
    }

    const { barn } = env;
    if (!barn.host || !barn.user || !barn.port || !barn.identity_file) {
      setError(`Barn '${barn.name}' is missing SSH configuration`);
      return;
    }

    enterRemoteMode(barn.name, barn.host, barn.user, barn.port, barn.identity_file);
  }, [tmuxAvailable, environments]);

  useInput((input, key) => {
    // Clear error on any input
    if (error) setError(null);

    // Help toggle
    if (input === '?') {
      setShowHelp((s) => !s);
      return;
    }

    // Don't process other keys when help is shown
    if (showHelp) {
      if (key.escape) setShowHelp(false);
      return;
    }

    // q: Detach from session (only on global dashboard)
    if (input === 'q' && view.type === 'global') {
      detachFromSession();
      return;
    }

    // Shift-Q: Kill everything (only on global dashboard)
    if (input === 'Q' && view.type === 'global') {
      killYeehawSession();
      exit();
      return;
    }

    // Ctrl-R: Restart Yeehaw (preserves other tmux windows)
    if (key.ctrl && input === 'r') {
      restartYeehaw();
      return;
    }

    // ESC: Navigate back (handled by individual views for their sub-modes,
    // but also handled here as a fallback for consistent navigation)
    if (key.escape) {
      if (view.type === 'wiki' || view.type === 'issues') {
        handleBackFromSubview(view.project);
      } else if (view.type === 'logs') {
        setView({ type: 'livestock', project: view.project, livestock: view.livestock, source: view.source, sourceBarn: view.sourceBarn });
        updateStatusBar(`${view.project.name} / ${view.livestock.name}`);
      } else if (view.type === 'livestock') {
        handleBackFromLivestock(view.source, view.project, view.sourceBarn);
      } else if (view.type === 'critter-logs') {
        setView({ type: 'critter', barn: view.barn, critter: view.critter });
        updateStatusBar(`${view.barn.name} / ${view.critter.name}`);
      } else if (view.type === 'critter') {
        handleBackFromCritter(view.barn);
      } else if (view.type === 'herd') {
        handleBackFromHerd(view.project);
      } else if (view.type === 'ranchhand') {
        handleBackFromRanchHand(view.project);
      } else if (view.type === 'project' || view.type === 'barn') {
        handleBack();
      }
      return;
    }

    // g+number: Connect to remote environment (two-key sequence)
    if (pendingGo) {
      setPendingGo(false);
      if (/^[1-9]$/.test(input)) {
        const envIndex = parseInt(input, 10) - 1;
        if (envIndex < environments.length) {
          handleConnectToRemote(envIndex);
        }
      }
      // Any key after 'g' clears the pending state
      return;
    }

    // 'g' initiates the go sequence (only when environments exist)
    if (input === 'g' && environments.length > 0) {
      setPendingGo(true);
      return;
    }

    // v: Enter night sky visualizer (from supported views when not in text input mode)
    if (input === 'v' && !isChildInputMode) {
      const supportedViews = ['global', 'project', 'livestock', 'barn', 'critter'];
      if (supportedViews.includes(view.type)) {
        handleEnterNightSky();
        return;
      }
    }
  });

  // Render based on view type
  const renderView = () => {
    if (showHelp) {
      return <HelpOverlay scope={getHotkeyScope(view)} />;
    }

    switch (view.type) {
      case 'global':
        return (
          <GlobalDashboard
            projects={projects}
            barns={barns}
            windows={windows}
            versionInfo={versionInfo}
            onSelectProject={handleSelectProject}
            onSelectBarn={handleSelectBarn}
            onSelectWindow={handleSelectWindow}
            onNewClaudeForProject={handleNewClaudeForProject}
            onCreateProject={handleCreateProject}
            onCreateBarn={handleCreateBarn}
            onSshToBarn={handleSshToBarn}
            onInputModeChange={setIsChildInputMode}
          />
        );

      case 'project':
        return (
          <ProjectContext
            project={view.project}
            barns={barns}
            windows={windows}
            onBack={handleBack}
            onNewClaudeForLivestock={(livestock) => handleNewClaudeForLivestock(livestock, view.project.name)}
            onSelectWindow={handleSelectWindow}
            onSelectLivestock={(livestock, barn) => handleOpenLivestockDetail(view.project, livestock, 'project')}
            onOpenLivestockSession={(livestock, barn) => handleOpenLivestockSession(livestock, barn, view.project.name)}
            onUpdateProject={handleUpdateProject}
            onDeleteProject={handleDeleteProject}
            onOpenWiki={() => handleOpenWiki(view.project)}
            onOpenIssues={() => handleOpenIssues(view.project)}
            onSelectHerd={(herd) => handleOpenHerdDetail(view.project, herd)}
            onSelectRanchHand={(ranchhand) => handleOpenRanchHandDetail(view.project, ranchhand)}
          />
        );

      case 'barn':
        const barnLivestock = getLivestockForBarn(view.barn.name);
        return (
          <BarnContext
            barn={view.barn}
            livestock={barnLivestock}
            projects={projects}
            windows={windows}
            onBack={handleBack}
            onSshToBarn={() => handleSshToBarn(view.barn)}
            onSelectLivestock={(project, livestock) => handleOpenLivestockDetail(project, livestock, 'barn', view.barn)}
            onOpenLivestockSession={(project, livestock) => {
              const barn = barns.find((b) => b.name === livestock.barn) || null;
              handleOpenLivestockSession(livestock, barn, project.name);
            }}
            onUpdateBarn={handleUpdateBarn}
            onDeleteBarn={handleDeleteBarn}
            onAddLivestock={(project, livestock) => {
              // Add livestock to project
              const updatedLivestock = [...(project.livestock || [])];
              const existingIdx = updatedLivestock.findIndex((l) => l.name === livestock.name);
              if (existingIdx >= 0) {
                updatedLivestock[existingIdx] = livestock;
              } else {
                updatedLivestock.push(livestock);
              }
              const updatedProject = { ...project, livestock: updatedLivestock };
              saveProject(updatedProject);
              reload();
            }}
            onRemoveLivestock={(project, livestockName) => {
              // Remove livestock from project
              const updatedLivestock = (project.livestock || []).filter(
                (l) => l.name !== livestockName
              );
              const updatedProject = { ...project, livestock: updatedLivestock };
              saveProject(updatedProject);
              reload();
            }}
            onAddCritter={(critter: Critter) => {
              addCritterToBarn(view.barn.name, critter);
              reload();
            }}
            onRemoveCritter={(critterName: string) => {
              removeCritterFromBarn(view.barn.name, critterName);
              reload();
            }}
            onSelectCritter={(critter) => handleOpenCritterDetail(view.barn, critter)}
          />
        );

      case 'wiki':
        return (
          <WikiView
            project={view.project}
            onBack={() => handleBackFromSubview(view.project)}
            onUpdateProject={handleUpdateProject}
          />
        );

      case 'issues':
        return (
          <IssuesView
            project={view.project}
            onBack={() => handleBackFromSubview(view.project)}
            onOpenClaude={handleOpenClaudeWithContext}
          />
        );

      case 'livestock':
        const livestockBarn = barns.find((b) => b.name === view.livestock.barn) || null;
        const isLocalLivestock = !livestockBarn || isLocalBarn(livestockBarn);
        return (
          <LivestockDetailView
            project={view.project}
            livestock={view.livestock}
            source={view.source}
            sourceBarn={view.sourceBarn}
            windows={windows}
            onBack={() => handleBackFromLivestock(view.source, view.project, view.sourceBarn)}
            onOpenLogs={() => handleOpenLogs(view.project, view.livestock, view.source, view.sourceBarn)}
            onOpenSession={() => {
              handleOpenLivestockSession(view.livestock, livestockBarn, view.project.name);
            }}
            onOpenClaude={isLocalLivestock ? () => handleNewClaudeForLivestock(view.livestock, view.project.name) : undefined}
            onSelectWindow={handleSelectWindow}
            onUpdateLivestock={(originalLivestock, updatedLivestock) => handleUpdateLivestock(view.project, originalLivestock, updatedLivestock, view.source, view.sourceBarn)}
          />
        );

      case 'logs':
        return (
          <LogsView
            project={view.project}
            livestock={view.livestock}
            onBack={() => {
              setView({ type: 'livestock', project: view.project, livestock: view.livestock, source: view.source, sourceBarn: view.sourceBarn });
              updateStatusBar(`${view.project.name} / ${view.livestock.name}`);
            }}
          />
        );

      case 'critter':
        return (
          <CritterDetailView
            barn={view.barn}
            critter={view.critter}
            onBack={() => handleBackFromCritter(view.barn)}
            onOpenLogs={() => handleOpenCritterLogs(view.barn, view.critter)}
            onUpdateCritter={(original, updated) => handleUpdateCritter(view.barn, original, updated)}
          />
        );

      case 'critter-logs':
        return (
          <CritterLogsView
            barn={view.barn}
            critter={view.critter}
            onBack={() => {
              setView({ type: 'critter', barn: view.barn, critter: view.critter });
              updateStatusBar(`${view.barn.name} / ${view.critter.name}`);
            }}
          />
        );

      case 'herd':
        return (
          <HerdDetailView
            project={view.project}
            herd={view.herd}
            barns={barns}
            ranchHands={loadRanchHandsForProject(view.project.name)}
            onBack={() => handleBackFromHerd(view.project)}
            onUpdateHerd={(herd) => handleUpdateHerd(view.project, herd)}
          />
        );

      case 'ranchhand':
        return (
          <RanchHandDetailView
            project={view.project}
            ranchhand={view.ranchhand}
            onBack={() => handleBackFromRanchHand(view.project)}
            onUpdateRanchHand={handleUpdateRanchHand}
            onSyncComplete={() => handleSyncRanchHand(view.project, view.ranchhand)}
          />
        );

      case 'night-sky':
        return (
          <NightSkyView
            context={previousView ? buildNightSkyContext(previousView, windows) : undefined}
            onExit={handleExitNightSky}
          />
        );
    }
  };

  // Memoized callback for splash screen to prevent unnecessary re-renders
  const handleSplashComplete = useCallback(() => setShowSplash(false), []);

  // Show splash screen on first load
  if (showSplash) {
    return <SplashScreen onComplete={handleSplashComplete} />;
  }

  // Show Claude splash screen when launching a Claude session
  if (claudeSplash) {
    return (
      <ClaudeSplashScreen
        systemPrompt={claudeSplash.systemPrompt}
        mcpTools={claudeSplash.mcpTools}
        projectColor={claudeSplash.projectColor}
        onComplete={() => {
          switchToWindow(claudeSplash.windowIndex);
          setClaudeSplash(null);
        }}
        onCancel={() => {
          killWindow(claudeSplash.windowIndex);
          setClaudeSplash(null);
        }}
      />
    );
  }

  return (
    <Box flexDirection="column" height={terminalHeight}>
      {error && (
        <Box paddingX={1}>
          <Text color="red">Error: {error}</Text>
        </Box>
      )}

      <Box flexDirection="column" flexGrow={1}>
        {renderView()}
      </Box>

      {!showHelp && (
        <BottomBar
          items={getBottomBarItems(view.type, {
            isLocalLivestock: view.type === 'livestock'
              ? !view.livestock.barn || isLocalBarn(barns.find((b) => b.name === view.livestock.barn) || { name: 'local' })
              : undefined,
            isLocalBarn: view.type === 'barn'
              ? isLocalBarn(view.barn)
              : undefined,
          })}
          environments={environments}
          isDetecting={isDetecting}
        />
      )}
    </Box>
  );
}
