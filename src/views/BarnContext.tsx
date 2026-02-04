import React, { useState, useCallback, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { BarnHeader } from '../components/BarnHeader.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem, type RowAction } from '../components/List.js';
import { PathInput } from '../components/PathInput.js';
import { detectRemoteGitInfo, detectGitInfo, type GitInfo } from '../lib/git.js';
import { isLocalBarn } from '../lib/config.js';
import { parseGitHubUrl } from '../lib/github.js';
import { discoverCritters, getServiceDetails, type DiscoveredCritter, type ServiceDetails } from '../lib/critters.js';
import type { Barn, Project, Livestock, Critter } from '../types.js';
import type { TmuxWindow } from '../lib/tmux.js';

type FocusedPanel = 'livestock' | 'critters';
type Mode =
  | 'normal'
  | 'edit-host' | 'edit-user' | 'edit-port'
  | 'delete-barn-confirm'
  | 'add-livestock-path' | 'add-livestock-project' | 'add-livestock-name'
  | 'delete-livestock-confirm'
  | 'add-critter-name' | 'add-critter-service'
  | 'add-critter-service-path' | 'add-critter-config-path' | 'add-critter-log-path' | 'add-critter-use-journald'
  | 'delete-critter-confirm';

interface LivestockWithProject {
  project: Project;
  livestock: Livestock;
}

interface BarnContextProps {
  barn: Barn;
  livestock: LivestockWithProject[];
  projects: Project[];
  windows: TmuxWindow[];
  onBack: () => void;
  onSshToBarn: () => void;
  onSelectLivestock: (project: Project, livestock: Livestock) => void;
  onOpenLivestockSession: (project: Project, livestock: Livestock) => void;
  onUpdateBarn: (barn: Barn) => void;
  onDeleteBarn: (barnName: string) => void;
  onAddLivestock: (project: Project, livestock: Livestock) => void;
  onRemoveLivestock: (project: Project, livestockName: string) => void;
  onAddCritter: (critter: Critter) => void;
  onRemoveCritter: (critterName: string) => void;
  onSelectCritter: (critter: Critter) => void;
}

export function BarnContext({
  barn,
  livestock,
  projects,
  windows,
  onBack,
  onSshToBarn,
  onSelectLivestock,
  onOpenLivestockSession,
  onUpdateBarn,
  onDeleteBarn,
  onAddLivestock,
  onRemoveLivestock,
  onAddCritter,
  onRemoveCritter,
  onSelectCritter,
}: BarnContextProps) {
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('livestock');
  const [mode, setMode] = useState<Mode>('normal');

  // Check if this is the local barn
  const isLocal = isLocalBarn(barn);

  // Edit form state (only used for remote barns)
  const [editHost, setEditHost] = useState(barn.host || '');
  const [editUser, setEditUser] = useState(barn.user || '');
  const [editPort, setEditPort] = useState(String(barn.port || 22));

  // Delete confirmation
  const [deleteConfirmInput, setDeleteConfirmInput] = useState('');

  // Add livestock state (new flow: path → detect git → match/pick project → name)
  const [newLivestockPath, setNewLivestockPath] = useState('');
  const [detectedGit, setDetectedGit] = useState<GitInfo | null>(null);
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [newLivestockName, setNewLivestockName] = useState('');

  // Delete livestock state
  const [selectedLivestockIndex, setSelectedLivestockIndex] = useState(0);
  const [deleteLivestockTarget, setDeleteLivestockTarget] = useState<LivestockWithProject | null>(null);

  // Critter state
  const [selectedCritterIndex, setSelectedCritterIndex] = useState(0);
  const [newCritterName, setNewCritterName] = useState('');
  const [newCritterService, setNewCritterService] = useState('');
  const [deleteCritterTarget, setDeleteCritterTarget] = useState<Critter | null>(null);

  // Service selection state
  const [availableServices, setAvailableServices] = useState<DiscoveredCritter[]>([]);
  const [serviceFilter, setServiceFilter] = useState('');
  const [servicesLoading, setServicesLoading] = useState(false);
  const [servicesError, setServicesError] = useState<string | null>(null);
  const [selectedServiceIndex, setSelectedServiceIndex] = useState(0);

  // Auto-detected critter details (used to pre-fill editable fields)
  const [detectedDetails, setDetectedDetails] = useState<ServiceDetails | null>(null);
  const [detectedLoading, setDetectedLoading] = useState(false);

  // Editable critter fields (pre-filled from detection, user can modify)
  const [newCritterServicePath, setNewCritterServicePath] = useState('');
  const [newCritterConfigPath, setNewCritterConfigPath] = useState('');
  const [newCritterLogPath, setNewCritterLogPath] = useState('');
  const [newCritterUseJournald, setNewCritterUseJournald] = useState(true);

  // Filter windows that are barn sessions
  const barnWindows = windows.filter((w) => w.index > 0 && w.name.startsWith(`barn-${barn.name}`));

  // Fetch available services from the barn
  const fetchServices = useCallback(async () => {
    setServicesLoading(true);
    setServicesError(null);
    const result = await discoverCritters(barn);
    setServicesLoading(false);
    if (result.error) {
      setServicesError(result.error);
    }
    setAvailableServices(result.critters);
    setSelectedServiceIndex(0);
  }, [barn]);

  // Fetch services when entering service selection mode
  useEffect(() => {
    if (mode === 'add-critter-service') {
      fetchServices();
    }
  }, [mode, fetchServices]);

  const startEdit = () => {
    if (isLocal) return; // Cannot edit local barn
    setEditHost(barn.host || '');
    setEditUser(barn.user || '');
    setEditPort(String(barn.port || 22));
    setMode('edit-host');
  };

  const cancelEdit = () => {
    setMode('normal');
  };

  const saveAndNext = (nextMode: Mode | 'done') => {
    if (nextMode === 'done') {
      onUpdateBarn({
        ...barn,
        host: editHost,
        user: editUser,
        port: parseInt(editPort, 10) || 22,
      });
      setMode('normal');
    } else {
      setMode(nextMode);
    }
  };

  useInput((input, key) => {
    // Handle escape
    if (key.escape) {
      if (mode !== 'normal') {
        cancelEdit();
      } else {
        onBack();
      }
      return;
    }

    // Handle delete livestock confirmation
    if (mode === 'delete-livestock-confirm' && deleteLivestockTarget) {
      if (input === 'y') {
        onRemoveLivestock(deleteLivestockTarget.project, deleteLivestockTarget.livestock.name);
        setDeleteLivestockTarget(null);
        setSelectedLivestockIndex(Math.max(0, selectedLivestockIndex - 1));
        setMode('normal');
      } else if (input === 'n') {
        setDeleteLivestockTarget(null);
        setMode('normal');
      }
      return;
    }

    // Handle delete critter confirmation
    if (mode === 'delete-critter-confirm' && deleteCritterTarget) {
      if (input === 'y') {
        onRemoveCritter(deleteCritterTarget.name);
        setDeleteCritterTarget(null);
        setSelectedCritterIndex(Math.max(0, selectedCritterIndex - 1));
        setMode('normal');
      } else if (input === 'n') {
        setDeleteCritterTarget(null);
        setMode('normal');
      }
      return;
    }

    // Handle arrow keys for service selection
    if (mode === 'add-critter-service') {
      const filteredServices = availableServices.filter((s) =>
        s.suggested_name.toLowerCase().includes(serviceFilter.toLowerCase()) ||
        s.service.toLowerCase().includes(serviceFilter.toLowerCase()) ||
        (s.command && s.command.toLowerCase().includes(serviceFilter.toLowerCase()))
      );

      if (key.upArrow) {
        setSelectedServiceIndex((prev) => Math.max(0, prev - 1));
        return;
      }
      if (key.downArrow) {
        setSelectedServiceIndex((prev) => Math.min(filteredServices.length - 1, prev + 1));
        return;
      }
    }

    // Handle use-journald toggle and save
    if (mode === 'add-critter-use-journald') {
      if (input === ' ') {
        setNewCritterUseJournald(!newCritterUseJournald);
        return;
      }
      if (key.return) {
        const critter: Critter = {
          name: newCritterName.trim(),
          service: newCritterService,
          service_path: newCritterServicePath.trim() || undefined,
          config_path: newCritterConfigPath.trim() || undefined,
          log_path: newCritterLogPath.trim() || undefined,
          use_journald: newCritterUseJournald,
        };
        onAddCritter(critter);
        setMode('normal');
        return;
      }
    }

    if (mode !== 'normal') return

    if (key.tab) {
      setFocusedPanel((p) => (p === 'livestock' ? 'critters' : 'livestock'));
      return;
    }

    if (input === 'e' && !isLocal) {
      startEdit();
      return;
    }

    if (input === 'D' && !isLocal) {
      setDeleteConfirmInput('');
      setMode('delete-barn-confirm');
      return;
    }

    if (input === 'n' && focusedPanel === 'livestock') {
      // Start add livestock flow with path first
      setNewLivestockPath('');
      setDetectedGit(null);
      setSelectedProject(null);
      setNewLivestockName('');
      setMode('add-livestock-path');
      return;
    }

    if (input === 'd' && focusedPanel === 'livestock' && livestock.length > 0) {
      // Delete selected livestock
      const target = livestock[selectedLivestockIndex];
      if (target) {
        setDeleteLivestockTarget(target);
        setMode('delete-livestock-confirm');
      }
      return;
    }

    if (input === 'n' && focusedPanel === 'critters') {
      // Start add critter flow
      setNewCritterName('');
      setNewCritterService('');
      setServiceFilter('');
      setSelectedServiceIndex(0);
      setDetectedDetails(null);
      // Reset editable fields
      setNewCritterServicePath('');
      setNewCritterConfigPath('');
      setNewCritterLogPath('');
      setNewCritterUseJournald(true);
      setMode('add-critter-name');
      return;
    }

    if (input === 'd' && focusedPanel === 'critters' && (barn.critters || []).length > 0) {
      // Delete selected critter
      const critters = barn.critters || [];
      const target = critters[selectedCritterIndex];
      if (target) {
        setDeleteCritterTarget(target);
        setMode('delete-critter-confirm');
      }
      return;
    }
  });

  // Edit mode screens
  if (mode === 'edit-host') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Editing barn" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Barn</Text>
          <Box marginTop={1}>
            <Text>Host: </Text>
            <TextInput
              value={editHost}
              onChange={setEditHost}
              onSubmit={() => saveAndNext('edit-user')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-user') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Editing barn" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Barn: {barn.name}</Text>
          <Box marginTop={1}>
            <Text>User: </Text>
            <TextInput
              value={editUser}
              onChange={setEditUser}
              onSubmit={() => saveAndNext('edit-port')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'edit-port') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Editing barn" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Barn: {barn.name}</Text>
          <Box marginTop={1}>
            <Text>Port: </Text>
            <TextInput
              value={editPort}
              onChange={setEditPort}
              onSubmit={() => saveAndNext('done')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: save barn, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'delete-barn-confirm') {
    const handleDeleteConfirm = () => {
      if (deleteConfirmInput === barn.name) {
        onDeleteBarn(barn.name);
      }
    };

    const hasLivestock = livestock.length > 0;

    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="DELETE BARN" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">⚠️  Delete Barn</Text>
          {hasLivestock ? (
            <Box flexDirection="column" marginTop={1}>
              <Text color="red">Cannot delete barn - livestock still deployed:</Text>
              {livestock.map((l) => (
                <Text key={`${l.project.name}-${l.livestock.name}`} dimColor>
                  • {l.project.name}/{l.livestock.name}
                </Text>
              ))}
              <Box marginTop={1}>
                <Text dimColor>Remove livestock from projects first, then delete the barn.</Text>
              </Box>
            </Box>
          ) : (
            <>
              <Box marginTop={1}>
                <Text>This will permanently delete the barn configuration.</Text>
              </Box>
              <Box marginTop={1}>
                <Text>Type the barn name to confirm: </Text>
                <Text bold color="yellow">{barn.name}</Text>
              </Box>
              <Box marginTop={1}>
                <Text>Confirm: </Text>
                <TextInput
                  value={deleteConfirmInput}
                  onChange={setDeleteConfirmInput}
                  onSubmit={handleDeleteConfirm}
                />
              </Box>
              {deleteConfirmInput && deleteConfirmInput !== barn.name && (
                <Box marginTop={1}>
                  <Text color="red">Name does not match</Text>
                </Box>
              )}
            </>
          )}
          <Box marginTop={1}>
            <Text dimColor>Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add livestock flow: path → detect git → match/pick project → name

  // Step 1: Enter path (with remote tab completion)
  if (mode === 'add-livestock-path') {
    const handlePathSubmit = () => {
      if (!newLivestockPath.trim()) return;

      // Detect git info from path (local or remote)
      const gitInfo = isLocal
        ? detectGitInfo(newLivestockPath)
        : detectRemoteGitInfo(newLivestockPath, barn);
      setDetectedGit(gitInfo);

      // Try to match project by repo URL
      if (gitInfo.isGitRepo && gitInfo.remoteUrl) {
        const parsed = parseGitHubUrl(gitInfo.remoteUrl);
        if (parsed) {
          // Find project with matching repo in any livestock
          const matchedProject = projects.find((p) =>
            (p.livestock || []).some((l) => {
              if (!l.repo) return false;
              const lParsed = parseGitHubUrl(l.repo);
              return lParsed && lParsed.owner === parsed.owner && lParsed.repo === parsed.repo;
            })
          );
          if (matchedProject) {
            setSelectedProject(matchedProject);
            setMode('add-livestock-name');
            return;
          }
        }
      }

      // No match found - let user pick project
      if (projects.length === 0) {
        // No projects available
        setMode('normal');
        return;
      }
      setMode('add-livestock-project');
    };

    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding livestock" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock to {barn.name}</Text>
          <Text dimColor>Enter the {isLocal ? 'local' : `path on ${barn.host}`} path</Text>
          <Box marginTop={1}>
            <Text>Path: </Text>
            <PathInput
              value={newLivestockPath}
              onChange={setNewLivestockPath}
              onSubmit={handlePathSubmit}
              barn={isLocal ? undefined : barn}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Tab: autocomplete, Enter: next (auto-detects git), Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Step 2 (if no auto-match): Select project
  if (mode === 'add-livestock-project') {
    const projectOptions: ListItem[] = projects.map((p) => ({
      id: p.name,
      label: p.name,
      status: 'active' as const,
      meta: p.path,
    }));

    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding livestock" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock to Barn</Text>
          {detectedGit?.isGitRepo ? (
            <Text dimColor>Repo detected: {detectedGit.remoteUrl} (no matching project found)</Text>
          ) : (
            <Text dimColor>No git repo detected. Which project does this belong to?</Text>
          )}
          <Box marginTop={1} flexDirection="column">
            <List
              items={projectOptions}
              focused={true}
              onSelect={(item) => {
                const project = projects.find((p) => p.name === item.id);
                if (project) {
                  setSelectedProject(project);
                  setMode('add-livestock-name');
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

  // Step 3: Enter name
  if (mode === 'add-livestock-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding livestock" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Livestock: {selectedProject?.name}</Text>
          {detectedGit?.isGitRepo && (
            <Text color="cyan">Git detected: {detectedGit.branch || 'unknown branch'}</Text>
          )}
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newLivestockName}
              onChange={setNewLivestockName}
              onSubmit={() => {
                if (newLivestockName.trim() && selectedProject) {
                  const newLivestock: Livestock = {
                    name: newLivestockName,
                    path: newLivestockPath,
                    barn: isLocal ? undefined : barn.name,  // Don't set barn for local livestock
                    repo: detectedGit?.remoteUrl,
                    branch: detectedGit?.branch,
                  };
                  onAddLivestock(selectedProject, newLivestock);
                  setMode('normal');
                }
              }}
              placeholder="production, staging, etc."
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: save, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Delete livestock confirmation
  if (mode === 'delete-livestock-confirm' && deleteLivestockTarget) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Remove livestock" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Remove Livestock</Text>
          <Box marginTop={1}>
            <Text>Remove "{deleteLivestockTarget.livestock.name}" from {deleteLivestockTarget.project.name}?</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Path: {deleteLivestockTarget.livestock.path}</Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, remove</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter flow: name → service
  if (mode === 'add-critter-name') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Critter to {barn.name}</Text>
          <Text dimColor>A critter is a system service like mysql, redis, nginx, etc.</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={newCritterName}
              onChange={setNewCritterName}
              onSubmit={() => {
                if (newCritterName.trim()) {
                  // Auto-suggest service name
                  setNewCritterService(`${newCritterName.trim()}.service`);
                  setMode('add-critter-service');
                }
              }}
              placeholder="mysql, redis, nginx..."
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  if (mode === 'add-critter-service') {
    const filteredServices = availableServices.filter((s) =>
      s.suggested_name.toLowerCase().includes(serviceFilter.toLowerCase()) ||
      s.service.toLowerCase().includes(serviceFilter.toLowerCase()) ||
      (s.command && s.command.toLowerCase().includes(serviceFilter.toLowerCase()))
    );

    // Calculate visible window (scroll if needed)
    const maxVisible = 8;
    const startIndex = Math.max(0, selectedServiceIndex - Math.floor(maxVisible / 2));
    const visibleServices = filteredServices.slice(startIndex, startIndex + maxVisible);

    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Critter: {newCritterName}</Text>
          <Text dimColor>
            Discovered services ({filteredServices.length} shown)
          </Text>

          {/* Filter input */}
          <Box marginTop={1}>
            <Text>Filter: </Text>
            <TextInput
              value={serviceFilter}
              onChange={(val) => {
                setServiceFilter(val);
                setSelectedServiceIndex(0);
              }}
              onSubmit={() => {
                if (filteredServices.length > 0) {
                  const selected = filteredServices[selectedServiceIndex];
                  setNewCritterService(selected.service);
                  // Pre-fill from discovered critter data
                  setNewCritterServicePath(selected.config_path || '');
                  setNewCritterConfigPath(selected.config_path || '');
                  setNewCritterLogPath(selected.log_path || '');
                  // Supervisor services don't use journald
                  setNewCritterUseJournald(selected.manager !== 'supervisor');
                  setMode('add-critter-service-path');
                }
              }}
            />
          </Box>

          {/* Service list */}
          <Box marginTop={1} flexDirection="column">
            {servicesLoading ? (
              <Text dimColor>Loading services...</Text>
            ) : servicesError ? (
              <Text color="red">{servicesError}</Text>
            ) : filteredServices.length === 0 ? (
              <Text dimColor>No services match filter</Text>
            ) : (
              visibleServices.map((service, i) => {
                const actualIndex = startIndex + i;
                const isSelected = actualIndex === selectedServiceIndex;
                const statusColor = service.status === 'running' ? 'green' : service.status === 'unknown' ? 'yellow' : 'red';
                const statusIndicator = service.status === 'running' ? '●' : service.status === 'unknown' ? '○' : '○';
                return (
                  <Box key={service.service} flexDirection="column">
                    <Text>
                      {isSelected ? <Text color="cyan">{'> '}</Text> : '  '}
                      <Text color={statusColor}>{statusIndicator}</Text>
                      {' '}
                      <Text bold={isSelected}>{service.suggested_name}</Text>
                      <Text dimColor> ({service.service})</Text>
                    </Text>
                    {service.command && (
                      <Text>
                        {'    '}
                        <Text dimColor>{service.command}</Text>
                      </Text>
                    )}
                  </Box>
                );
              })
            )}
            {filteredServices.length > maxVisible && (
              <Text dimColor>
                {startIndex > 0 ? '↑ ' : '  '}
                {startIndex + maxVisible < filteredServices.length ? '↓ more...' : ''}
              </Text>
            )}
          </Box>

          <Box marginTop={1}>
            <Text dimColor>Up/Down: navigate, Enter: select, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter: service path (pre-filled from detection)
  if (mode === 'add-critter-service-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Critter: {newCritterName}</Text>
          <Text dimColor>Service: {newCritterService}</Text>
          <Box marginTop={1}>
            <Text>Service file path (optional): </Text>
            <TextInput
              value={newCritterServicePath}
              onChange={setNewCritterServicePath}
              onSubmit={() => setMode('add-critter-config-path')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter: config path (pre-filled from detection)
  if (mode === 'add-critter-config-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Critter: {newCritterName}</Text>
          <Text dimColor>Service: {newCritterService}</Text>
          <Box marginTop={1}>
            <Text>Config path (optional): </Text>
            <TextInput
              value={newCritterConfigPath}
              onChange={setNewCritterConfigPath}
              onSubmit={() => setMode('add-critter-log-path')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter: log path (pre-filled from detection)
  if (mode === 'add-critter-log-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Critter: {newCritterName}</Text>
          <Text dimColor>Service: {newCritterService}</Text>
          <Box marginTop={1}>
            <Text>Log path (optional, if not using journald): </Text>
            <TextInput
              value={newCritterLogPath}
              onChange={setNewCritterLogPath}
              onSubmit={() => setMode('add-critter-use-journald')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter: use journald toggle (pre-filled from detection)
  if (mode === 'add-critter-use-journald') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Adding critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Add Critter: {newCritterName}</Text>
          <Text dimColor>Service: {newCritterService}</Text>
          <Box marginTop={1}>
            <Text>Use journald for logs: </Text>
            <Text bold color={newCritterUseJournald ? 'green' : 'red'}>
              {newCritterUseJournald ? 'Yes' : 'No'}
            </Text>
            <Text dimColor> (press space to toggle)</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Space: toggle, Enter: save critter, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Delete critter confirmation
  if (mode === 'delete-critter-confirm' && deleteCritterTarget) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <BarnHeader name={barn.name} subtitle="Remove critter" />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Remove Critter</Text>
          <Box marginTop={1}>
            <Text>Remove "{deleteCritterTarget.name}" from {barn.name}?</Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Service: {deleteCritterTarget.service}</Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, remove</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Build livestock items
  const livestockItems: ListItem[] = livestock.map((l) => ({
    id: `${l.project.name}/${l.livestock.name}`,
    label: `${l.project.name}/${l.livestock.name}`,
    status: 'active',
    meta: l.livestock.path,
    actions: [{ key: 's', label: 'shell' }],
  }));

  // Build critter items
  const critterItems: ListItem[] = (barn.critters || []).map((c) => ({
    id: c.name,
    label: c.name,
    status: 'active' as const, // Critters are assumed active (discovery only finds running services)
    meta: c.service,
  }));

  // Panel-specific hints (page-level hotkeys like s are in BottomBar)
  const livestockHints = '[n] new  [d] delete';
  const crittersHints = '[n] new  [d] delete';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <BarnHeader name={barn.name} subtitle={isLocal ? 'Local machine' : `${barn.user}@${barn.host}:${barn.port}`} />

      {/* Connection info (only for remote barns) */}
      {!isLocal && (
        <Box paddingX={2} marginY={1}>
          <Box gap={4}>
            <Text>
              <Text dimColor>Host:</Text> {barn.host}
            </Text>
            <Text>
              <Text dimColor>User:</Text> {barn.user}
            </Text>
            <Text>
              <Text dimColor>Port:</Text> {barn.port}
            </Text>
            <Text>
              <Text dimColor>Key:</Text> {barn.identity_file}
            </Text>
          </Box>
        </Box>
      )}

      <Box flexGrow={1} marginY={1} paddingX={1} gap={2}>
        {/* Left: Livestock on this barn */}
        <Panel title="Livestock" focused={focusedPanel === 'livestock'} width="50%" hints={livestockHints}>
          {livestockItems.length > 0 ? (
            <List
              items={livestockItems}
              focused={focusedPanel === 'livestock'}
              selectedIndex={selectedLivestockIndex}
              onSelectionChange={setSelectedLivestockIndex}
              onSelect={(item) => {
                const found = livestock.find(
                  (l) => `${l.project.name}/${l.livestock.name}` === item.id
                );
                if (found) {
                  onSelectLivestock(found.project, found.livestock);
                }
              }}
              onAction={(item, actionKey) => {
                if (actionKey === 's') {
                  const found = livestock.find(
                    (l) => `${l.project.name}/${l.livestock.name}` === item.id
                  );
                  if (found) {
                    onOpenLivestockSession(found.project, found.livestock);
                  }
                }
              }}
            />
          ) : (
            <Box flexDirection="column">
              <Text dimColor>No livestock deployed to this barn</Text>
            </Box>
          )}
        </Panel>

        {/* Right: Critters (system services) */}
        <Panel title="Critters" focused={focusedPanel === 'critters'} width="50%" hints={crittersHints}>
          {critterItems.length > 0 ? (
            <List
              items={critterItems}
              focused={focusedPanel === 'critters'}
              selectedIndex={selectedCritterIndex}
              onSelectionChange={setSelectedCritterIndex}
              onSelect={(item) => {
                const critter = (barn.critters || []).find((c) => c.name === item.id);
                if (critter) {
                  onSelectCritter(critter);
                }
              }}
            />
          ) : (
            <Box flexDirection="column">
              <Text dimColor>No critters configured</Text>
              <Text dimColor italic>Press [n] to add a critter</Text>
            </Box>
          )}
        </Panel>
      </Box>

    </Box>
  );
}
