import React, { useState, useEffect, useMemo } from 'react';
import { Box, Text, useInput } from 'ink';
import { RanchHandHeader } from '../components/RanchHandHeader.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem } from '../components/List.js';
import type { Project, RanchHand, KubernetesConfig, TerraformConfig } from '../types.js';
import {
  discoverK8sResources,
  type K8sDiscoveryResult,
} from '../lib/ranchhand-k8s.js';
import {
  discoverTerraformResources,
  type TerraformDiscoveryResult,
} from '../lib/ranchhand-terraform.js';

type Mode =
  | 'normal'
  | 'discovering'
  | 'syncing'
  | 'error';

interface RanchHandDetailViewProps {
  project: Project;
  ranchhand: RanchHand;
  onBack: () => void;
  onUpdateRanchHand: (ranchhand: RanchHand) => void;
  onSyncComplete: () => void;
}

export function RanchHandDetailView({
  project,
  ranchhand,
  onBack,
  onUpdateRanchHand,
  onSyncComplete,
}: RanchHandDetailViewProps) {
  const [mode, setMode] = useState<Mode>('normal');
  const [selectedResourceIndex, setSelectedResourceIndex] = useState(0);

  // Discovery state
  const [k8sDiscovery, setK8sDiscovery] = useState<K8sDiscoveryResult | null>(null);
  const [tfDiscovery, setTfDiscovery] = useState<TerraformDiscoveryResult | null>(null);
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);

  // Auto-discover on mount if no discovery data
  useEffect(() => {
    if (!k8sDiscovery && !tfDiscovery && mode === 'normal') {
      handleDiscover();
    }
  }, []);

  const handleDiscover = async () => {
    setMode('discovering');
    setDiscoveryError(null);

    try {
      if (ranchhand.type === 'kubernetes') {
        const config = ranchhand.config as KubernetesConfig;
        const result = discoverK8sResources(config);
        setK8sDiscovery(result);
      } else {
        const config = ranchhand.config as TerraformConfig;
        const existingHerds = (project.herds || []).map(h => h.name);
        const result = discoverTerraformResources(config, existingHerds);
        setTfDiscovery(result);
      }
      setMode('normal');
    } catch (err) {
      setDiscoveryError(err instanceof Error ? err.message : String(err));
      setMode('error');
    }
  };

  const handleSync = async () => {
    if (!ranchhand.herd) {
      setDiscoveryError('No herd assigned to this ranch hand');
      setMode('error');
      return;
    }

    setMode('syncing');

    try {
      onSyncComplete();
    } catch (err) {
      setDiscoveryError(err instanceof Error ? err.message : String(err));
      setMode('error');
    }
  };

  // Build resource items (for K8s shows pods/services, for TF shows resources)
  const resourceItems: ListItem[] = useMemo(() => {
    if (ranchhand.type === 'kubernetes' && k8sDiscovery) {
      // Show namespace info for the assigned herd
      const ns = k8sDiscovery.namespaces.find(n => n.name === ranchhand.herd);
      if (ns) {
        return [
          { id: 'livestock', label: `${ns.livestockCount} livestock`, status: 'active', meta: 'pods from private registries' },
          { id: 'critters', label: `${ns.critterCount} critters`, status: 'active', meta: 'system services' },
        ];
      }
      // Show nodes if no specific namespace
      return k8sDiscovery.nodes.map((node) => ({
        id: node.name,
        label: node.name,
        status: 'active',
        meta: node.internalIP,
      }));
    } else if (ranchhand.type === 'terraform' && tfDiscovery) {
      // Filter resources for this ranch hand's herd
      const herdResources = tfDiscovery.resources.filter(
        r => r.suggestedHerd === ranchhand.herd
      );
      if (herdResources.length > 0) {
        return herdResources.map((r) => ({
          id: r.id,
          label: r.displayName,
          status: 'active',
          meta: r.type,
        }));
      }
      // Show all resources if none match the herd
      return tfDiscovery.resources.map((r) => ({
        id: r.id,
        label: r.displayName,
        status: r.suggestedHerd ? 'active' : 'inactive',
        meta: `${r.type} → ${r.suggestedHerd || '?'}`,
      }));
    }
    return [];
  }, [k8sDiscovery, tfDiscovery, ranchhand.herd, ranchhand.type]);

  useInput((input, key) => {
    // Handle escape
    if (key.escape) {
      if (mode === 'error') {
        setMode('normal');
        setDiscoveryError(null);
      } else {
        onBack();
      }
      return;
    }

    // Don't handle input during async operations
    if (mode === 'discovering' || mode === 'syncing') {
      return;
    }

    // List navigation
    if (resourceItems.length > 0) {
      if (input === 'j' || key.downArrow) {
        setSelectedResourceIndex((i) => Math.min(i + 1, resourceItems.length - 1));
        return;
      }
      if (input === 'k' || key.upArrow) {
        setSelectedResourceIndex((i) => Math.max(i - 1, 0));
        return;
      }
    }

    // Actions
    if (input === 'r') {
      handleDiscover();
      return;
    }
    if (input === 's' && ranchhand.herd) {
      handleSync();
      return;
    }
  });

  // Get config details for inline display
  const getConfigDetails = () => {
    if (ranchhand.type === 'kubernetes') {
      const config = ranchhand.config as KubernetesConfig;
      return {
        primary: `context: ${config.context}`,
        secondary: config.private_registries.length > 0
          ? `registries: ${config.private_registries.join(', ')}`
          : null,
      };
    } else {
      const config = ranchhand.config as TerraformConfig;
      if (config.backend === 's3') {
        return {
          primary: `s3://${config.bucket}/${config.key}`,
          secondary: config.region ? `region: ${config.region}` : null,
        };
      } else {
        return {
          primary: `local: ${config.local_path || 'terraform.tfstate'}`,
          secondary: null,
        };
      }
    }
  };

  const configDetails = getConfigDetails();

  const renderStatusLine = () => {
    if (mode === 'discovering') {
      return <Text color="yellow">Discovering resources...</Text>;
    }
    if (mode === 'syncing') {
      return <Text color="yellow">Syncing resources...</Text>;
    }
    if (mode === 'error' && discoveryError) {
      return <Text color="red">Error: {discoveryError}</Text>;
    }
    return null;
  };

  const panelHints = '[r] refresh  [s] sync';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <RanchHandHeader ranchhand={ranchhand} projectColor={project.color} />

      {/* Config details inline */}
      <Box paddingX={2} gap={3}>
        <Text>
          <Text dimColor>backend:</Text> {configDetails.primary}
        </Text>
        {ranchhand.herd && (
          <Text>
            <Text dimColor>herd:</Text> {ranchhand.herd}
          </Text>
        )}
      </Box>

      {/* Secondary details */}
      <Box paddingX={2} gap={3} marginBottom={1}>
        {configDetails.secondary && (
          <Text dimColor>{configDetails.secondary}</Text>
        )}
        {ranchhand.last_sync && (
          <Text>
            <Text dimColor>last sync:</Text> {new Date(ranchhand.last_sync).toLocaleString()}
          </Text>
        )}
      </Box>

      {/* Status line if needed */}
      {(mode === 'discovering' || mode === 'syncing' || mode === 'error') && (
        <Box paddingX={2} marginBottom={1}>
          {renderStatusLine()}
        </Box>
      )}

      {/* Resources panel */}
      <Box paddingX={1} flexGrow={1}>
        <Panel
          title={ranchhand.type === 'kubernetes' ? 'Resources' : 'Terraform Resources'}
          focused={true}
          hints={panelHints}
        >
          {resourceItems.length > 0 ? (
            <List
              items={resourceItems}
              focused={true}
              selectedIndex={selectedResourceIndex}
              onSelectionChange={setSelectedResourceIndex}
            />
          ) : (
            <Text dimColor italic>
              {mode === 'discovering' ? 'Discovering...' : 'No resources found. Press [r] to refresh.'}
            </Text>
          )}
        </Panel>
      </Box>
    </Box>
  );
}
