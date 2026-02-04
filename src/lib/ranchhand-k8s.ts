/**
 * Kubernetes Ranch Hand Provider
 *
 * Discovers and syncs K8s resources into Yeehaw:
 * - Nodes → Barns
 * - Namespaces → Herds
 * - Pods (private image) → Livestock
 * - Pods (public image) → Critters
 */

import { execSync } from 'child_process';
import { shellEscape } from './shell.js';
import type {
  RanchHand,
  KubernetesConfig,
  Barn,
  Livestock,
  Critter,
  Herd,
  K8sLivestockMetadata,
  K8sCritterMetadata,
  K8sBarnConnectionConfig,
  EntitySource,
} from '../types.js';

// ============================================================================
// Types for K8s API responses
// ============================================================================

interface K8sNode {
  metadata: {
    name: string;
  };
  status: {
    addresses: Array<{
      type: string;
      address: string;
    }>;
  };
}

interface K8sNamespace {
  metadata: {
    name: string;
    labels?: Record<string, string>;
  };
}

interface K8sPod {
  metadata: {
    name: string;
    namespace: string;
    labels?: Record<string, string>;
    ownerReferences?: Array<{
      kind: string;
      name: string;
    }>;
  };
  spec: {
    nodeName: string;
    containers: Array<{
      name: string;
      image: string;
    }>;
  };
  status: {
    phase: string;
  };
}

// ============================================================================
// Discovery Results
// ============================================================================

export interface K8sDiscoveryResult {
  namespaces: K8sNamespaceInfo[];
  nodes: K8sNodeInfo[];
}

export interface K8sNamespaceInfo {
  name: string;
  livestockCount: number;
  critterCount: number;
  livestock: string[];  // names for preview
  critters: string[];   // names for preview
}

export interface K8sNodeInfo {
  name: string;
  internalIP: string;
  externalIP?: string;
}

export interface K8sSyncResult {
  barns: Barn[];
  livestock: Livestock[];
  critters: Critter[];
  herds: Herd[];
}

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Run kubectl command and return parsed JSON
 */
function kubectl<T>(
  context: string,
  args: string,
  kubeconfigPath?: string
): T {
  const kubeconfigArg = kubeconfigPath ? `--kubeconfig=${shellEscape(kubeconfigPath)}` : '';
  const cmd = `kubectl ${kubeconfigArg} --context=${shellEscape(context)} ${args} -o json`;

  try {
    const result = execSync(cmd, {
      encoding: 'utf-8',
      maxBuffer: 50 * 1024 * 1024, // 50MB buffer for large clusters
    });
    return JSON.parse(result) as T;
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    throw new Error(`kubectl command failed: ${msg}`);
  }
}

/**
 * Check if an image is from a private registry
 */
function isPrivateImage(image: string, privateRegistries: string[]): boolean {
  return privateRegistries.some(registry => image.startsWith(registry));
}

/**
 * Get deployment name from pod's owner references
 */
function getDeploymentName(pod: K8sPod): string | undefined {
  const replicaSet = pod.metadata.ownerReferences?.find(
    ref => ref.kind === 'ReplicaSet'
  );
  if (replicaSet) {
    // ReplicaSet name format: deployment-name-hash
    const parts = replicaSet.name.split('-');
    if (parts.length >= 2) {
      // Remove the hash suffix
      parts.pop();
      return parts.join('-');
    }
  }
  return undefined;
}

/**
 * Extract image tag from full image string
 */
function getImageTag(image: string): string | undefined {
  const parts = image.split(':');
  return parts.length > 1 ? parts[parts.length - 1] : undefined;
}

/**
 * Derive service name from image for critters
 */
function deriveServiceName(image: string): string {
  // Extract the image name without registry and tag
  // e.g., "redis:7.2" -> "redis"
  // e.g., "quay.io/prometheus/prometheus:v2.45" -> "prometheus"
  const withoutTag = image.split(':')[0];
  const parts = withoutTag.split('/');
  return parts[parts.length - 1];
}

// ============================================================================
// Discovery Functions
// ============================================================================

/**
 * Discover available resources from a K8s cluster
 */
export function discoverK8sResources(
  config: KubernetesConfig
): K8sDiscoveryResult {
  const { context, kubeconfig_path, private_registries } = config;

  // Get all namespaces
  const nsResponse = kubectl<{ items: K8sNamespace[] }>(
    context,
    'get namespaces',
    kubeconfig_path
  );

  // Get all nodes
  const nodeResponse = kubectl<{ items: K8sNode[] }>(
    context,
    'get nodes',
    kubeconfig_path
  );

  // Get all pods
  const podResponse = kubectl<{ items: K8sPod[] }>(
    context,
    'get pods --all-namespaces',
    kubeconfig_path
  );

  // Process nodes
  const nodes: K8sNodeInfo[] = nodeResponse.items.map(node => {
    const internalIP = node.status.addresses.find(a => a.type === 'InternalIP')?.address || '';
    const externalIP = node.status.addresses.find(a => a.type === 'ExternalIP')?.address;
    return {
      name: node.metadata.name,
      internalIP,
      externalIP,
    };
  });

  // Process namespaces with pod counts
  const namespaces: K8sNamespaceInfo[] = nsResponse.items.map(ns => {
    const nsPods = podResponse.items.filter(
      pod => pod.metadata.namespace === ns.metadata.name && pod.status.phase === 'Running'
    );

    const livestock: string[] = [];
    const critters: string[] = [];

    for (const pod of nsPods) {
      const mainContainer = pod.spec.containers[0];
      if (mainContainer) {
        const isLivestock = isPrivateImage(mainContainer.image, private_registries);
        const deploymentName = getDeploymentName(pod);
        const displayName = deploymentName || pod.metadata.name;

        if (isLivestock) {
          if (!livestock.includes(displayName)) {
            livestock.push(displayName);
          }
        } else {
          if (!critters.includes(displayName)) {
            critters.push(displayName);
          }
        }
      }
    }

    return {
      name: ns.metadata.name,
      livestockCount: livestock.length,
      critterCount: critters.length,
      livestock,
      critters,
    };
  });

  return { namespaces, nodes };
}

// ============================================================================
// Sync Functions
// ============================================================================

/**
 * Sync resources from K8s cluster based on ranch hand configuration
 */
export function syncK8sResources(
  ranchhand: RanchHand
): K8sSyncResult {
  const config = ranchhand.config as KubernetesConfig;
  const { context, kubeconfig_path, private_registries } = config;
  const sourceTag: EntitySource = `ranchhand:${ranchhand.name}`;

  // Get nodes
  const nodeResponse = kubectl<{ items: K8sNode[] }>(
    context,
    'get nodes',
    kubeconfig_path
  );

  // Get pods for the single herd/namespace
  const allPods: K8sPod[] = [];
  const ns = ranchhand.herd;
  if (ns) {
    try {
      const podResponse = kubectl<{ items: K8sPod[] }>(
        context,
        `get pods -n ${shellEscape(ns)}`,
        kubeconfig_path
      );
      allPods.push(...podResponse.items);
    } catch {
      // Namespace might not exist, skip it
    }
  }

  // Create barns from nodes
  const barns: Barn[] = nodeResponse.items.map(node => {
    const internalIP = node.status.addresses.find(a => a.type === 'InternalIP')?.address || '';

    const connectionConfig: K8sBarnConnectionConfig = {
      context,
      node: node.metadata.name,
    };

    return {
      name: node.metadata.name,
      host: internalIP,
      source: sourceTag,
      connection_type: 'kubernetes',
      connection_config: connectionConfig,
      connectable: false,
      critters: [],
    } as Barn;
  });

  // Process pods into livestock and critters
  const livestock: Livestock[] = [];
  const critters: Critter[] = [];
  const herdMap = new Map<string, { livestock: string[]; critters: Array<{ barn: string; critter: string }> }>();

  for (const pod of allPods) {
    if (pod.status.phase !== 'Running') continue;

    const namespace = pod.metadata.namespace;
    const mainContainer = pod.spec.containers[0];
    if (!mainContainer) continue;

    const isLivestockPod = isPrivateImage(mainContainer.image, private_registries);
    const deploymentName = getDeploymentName(pod);

    // Initialize herd tracking
    if (!herdMap.has(namespace)) {
      herdMap.set(namespace, { livestock: [], critters: [] });
    }
    const herdData = herdMap.get(namespace)!;

    if (isLivestockPod) {
      const k8sMetadata: K8sLivestockMetadata = {
        namespace,
        pod_name: pod.metadata.name,
        deployment: deploymentName,
        image: mainContainer.image,
        image_tag: getImageTag(mainContainer.image),
      };

      const ls: Livestock = {
        name: pod.metadata.name,
        path: `/var/run/containers/${pod.metadata.name}`, // Conceptual path
        barn: pod.spec.nodeName,
        source: sourceTag,
        k8s_metadata: k8sMetadata,
      };

      livestock.push(ls);

      // Track for herd
      if (!herdData.livestock.includes(pod.metadata.name)) {
        herdData.livestock.push(pod.metadata.name);
      }
    } else {
      const k8sMetadata: K8sCritterMetadata = {
        namespace,
        pod_name: pod.metadata.name,
        image: mainContainer.image,
      };

      const cr: Critter = {
        name: pod.metadata.name,
        service: deriveServiceName(mainContainer.image),
        source: sourceTag,
        k8s_metadata: k8sMetadata,
      };

      critters.push(cr);

      // Track for herd
      const critterRef = { barn: pod.spec.nodeName, critter: pod.metadata.name };
      if (!herdData.critters.some(c => c.critter === pod.metadata.name)) {
        herdData.critters.push(critterRef);
      }
    }
  }

  // Create herd for the single namespace
  const herdData = herdMap.get(ranchhand.herd) || { livestock: [], critters: [] };
  const herds: Herd[] = ranchhand.herd ? [{
    name: ranchhand.herd,
    livestock: herdData.livestock,
    critters: herdData.critters,
    connections: [], // Connections not auto-discovered yet
  }] : [];

  return { barns, livestock, critters, herds };
}

/**
 * Get available kubectl contexts from kubeconfig
 */
export function getKubectlContexts(kubeconfigPath?: string): string[] {
  const kubeconfigArg = kubeconfigPath ? `--kubeconfig=${shellEscape(kubeconfigPath)}` : '';
  const cmd = kubeconfigArg
    ? `kubectl ${kubeconfigArg} config get-contexts -o name`
    : `kubectl config get-contexts -o name`;

  try {
    const result = execSync(cmd, { encoding: 'utf-8' });
    return result.trim().split('\n').filter(Boolean);
  } catch (err) {
    // Re-throw with more context instead of silently returning empty
    const msg = err instanceof Error ? err.message : String(err);
    throw new Error(`Failed to get kubectl contexts: ${msg}`);
  }
}

/**
 * Get current kubectl context
 */
export function getCurrentKubectlContext(kubeconfigPath?: string): string | null {
  const kubeconfigArg = kubeconfigPath ? `--kubeconfig=${shellEscape(kubeconfigPath)}` : '';
  const cmd = `kubectl ${kubeconfigArg} config current-context`;

  try {
    const result = execSync(cmd, { encoding: 'utf-8' });
    return result.trim();
  } catch {
    return null;
  }
}
