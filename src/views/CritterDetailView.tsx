import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { CritterHeader } from '../components/CritterHeader.js';
import type { Barn, Critter } from '../types.js';

type Mode =
  | 'normal'
  | 'edit-name'
  | 'edit-service'
  | 'edit-config-path'
  | 'edit-log-path'
  | 'edit-use-journald';

interface CritterDetailViewProps {
  barn: Barn;
  critter: Critter;
  onBack: () => void;
  onOpenLogs: () => void;
  onUpdateCritter: (originalCritter: Critter, updatedCritter: Critter) => void;
}

export function CritterDetailView({
  barn,
  critter,
  onBack,
  onOpenLogs,
  onUpdateCritter,
}: CritterDetailViewProps) {
  const [mode, setMode] = useState<Mode>('normal');

  // Edit form state
  const [editName, setEditName] = useState(critter.name);
  const [editService, setEditService] = useState(critter.service);
  const [editConfigPath, setEditConfigPath] = useState(critter.config_path || '');
  const [editLogPath, setEditLogPath] = useState(critter.log_path || '');
  const [editUseJournald, setEditUseJournald] = useState(critter.use_journald !== false);

  // Sync form state when critter prop changes (e.g., after save)
  useEffect(() => {
    setEditName(critter.name);
    setEditService(critter.service);
    setEditConfigPath(critter.config_path || '');
    setEditLogPath(critter.log_path || '');
    setEditUseJournald(critter.use_journald !== false);
  }, [critter]);

  const resetForm = () => {
    setEditName(critter.name);
    setEditService(critter.service);
    setEditConfigPath(critter.config_path || '');
    setEditLogPath(critter.log_path || '');
    setEditUseJournald(critter.use_journald !== false);
  };

  // Save all pending changes at once
  const saveAllChanges = () => {
    const updated: Critter = {
      ...critter,
      name: editName.trim() || critter.name,
      service: editService.trim() || critter.service,
      config_path: editConfigPath.trim() || undefined,
      log_path: editLogPath.trim() || undefined,
      use_journald: editUseJournald,
    };
    onUpdateCritter(critter, updated);
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

    // Handle space to toggle journald in edit mode
    if (mode === 'edit-use-journald') {
      if (input === ' ') {
        setEditUseJournald(!editUseJournald);
        return;
      }
      if (key.return) {
        saveAllChanges();
        return;
      }
      return;
    }

    // Only process these in normal mode
    if (mode !== 'normal') return;

    if (input === 'l') {
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
        <CritterHeader barn={barn} critter={critter} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Critter</Text>
          <Box marginTop={1}>
            <Text>Name: </Text>
            <TextInput
              value={editName}
              onChange={setEditName}
              onSubmit={() => {
                if (editName.trim()) {
                  setMode('edit-service');
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

  // Edit service
  if (mode === 'edit-service') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <CritterHeader barn={barn} critter={critter} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Critter</Text>
          <Box marginTop={1}>
            <Text>Service (systemd): </Text>
            <TextInput
              value={editService}
              onChange={setEditService}
              onSubmit={() => {
                if (editService.trim()) {
                  setMode('edit-config-path');
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

  // Edit config path
  if (mode === 'edit-config-path') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <CritterHeader barn={barn} critter={critter} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Critter</Text>
          <Box marginTop={1}>
            <Text>Config Path (optional): </Text>
            <TextInput
              value={editConfigPath}
              onChange={setEditConfigPath}
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
        <CritterHeader barn={barn} critter={critter} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Critter</Text>
          <Box marginTop={1}>
            <Text>Log Path (optional, if not using journald): </Text>
            <TextInput
              value={editLogPath}
              onChange={setEditLogPath}
              onSubmit={() => setMode('edit-use-journald')}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: next field, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Edit use_journald (toggle with space)
  if (mode === 'edit-use-journald') {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <CritterHeader barn={barn} critter={critter} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="yellow">Edit Critter</Text>
          <Box marginTop={1}>
            <Text>Use Journald: </Text>
            <Text color={editUseJournald ? 'green' : 'red'} bold>
              {editUseJournald ? '[x] Yes' : '[ ] No'}
            </Text>
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Space: toggle, Enter: save & finish, Ctrl+S: save & exit, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Normal view - show critter info
  return (
    <Box flexDirection="column" flexGrow={1}>
      <CritterHeader barn={barn} critter={critter} />

      {/* Details */}
      <Box paddingX={2} flexDirection="column" marginTop={1}>
        <Box gap={3}>
          <Text>
            <Text dimColor>service:</Text> {critter.service}
          </Text>
          {critter.service_path && (
            <Text>
              <Text dimColor>unit:</Text> {critter.service_path}
            </Text>
          )}
        </Box>

        <Box gap={3} marginTop={1}>
          {critter.config_path && (
            <Text>
              <Text dimColor>config:</Text> {critter.config_path}
            </Text>
          )}
          <Text>
            <Text dimColor>logs:</Text>{' '}
            {critter.use_journald !== false ? (
              <Text>journald</Text>
            ) : critter.log_path ? (
              <Text>{critter.log_path}</Text>
            ) : (
              <Text dimColor>not configured</Text>
            )}
          </Text>
        </Box>
      </Box>

      {/* Hints */}
      <Box paddingX={2} marginTop={2}>
        <Text dimColor>
          [l] view logs  [e] edit  [esc] back
        </Text>
      </Box>
    </Box>
  );
}
