use std::process::Command;

use crate::tmux::shell_escape;
use crate::types::*;

// ============================================================================
// K8s API response types
// ============================================================================

#[derive(Debug, serde::Deserialize)]
struct K8sListResponse<T> {
    items: Vec<T>,
}

#[derive(Debug, serde::Deserialize)]
struct K8sNode {
    metadata: K8sNodeMeta,
    status: K8sNodeStatus,
}

#[derive(Debug, serde::Deserialize)]
struct K8sNodeMeta {
    name: String,
}

#[derive(Debug, serde::Deserialize)]
struct K8sNodeStatus {
    addresses: Vec<K8sAddress>,
}

#[derive(Debug, serde::Deserialize)]
struct K8sAddress {
    #[serde(rename = "type")]
    addr_type: String,
    address: String,
}

#[derive(Debug, serde::Deserialize)]
struct K8sNamespace {
    metadata: K8sNamespaceMeta,
}

#[derive(Debug, serde::Deserialize)]
struct K8sNamespaceMeta {
    name: String,
}

#[derive(Debug, serde::Deserialize)]
struct K8sPod {
    metadata: K8sPodMeta,
    spec: K8sPodSpec,
    status: K8sPodStatus,
}

#[derive(Debug, serde::Deserialize)]
struct K8sPodMeta {
    name: String,
    namespace: String,
    #[serde(default, rename = "ownerReferences")]
    owner_references: Vec<K8sOwnerRef>,
}

#[derive(Debug, serde::Deserialize)]
struct K8sOwnerRef {
    kind: String,
    name: String,
}

#[derive(Debug, serde::Deserialize)]
struct K8sPodSpec {
    #[serde(default, rename = "nodeName")]
    node_name: String,
    containers: Vec<K8sContainer>,
}

#[derive(Debug, serde::Deserialize)]
struct K8sContainer {
    image: String,
}

#[derive(Debug, serde::Deserialize)]
struct K8sPodStatus {
    phase: String,
}

// ============================================================================
// Discovery result types
// ============================================================================

#[derive(Debug, Clone, serde::Serialize)]
pub struct K8sDiscoveryResult {
    pub namespaces: Vec<K8sNamespaceInfo>,
    pub nodes: Vec<K8sNodeInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct K8sNamespaceInfo {
    pub name: String,
    pub livestock_count: usize,
    pub critter_count: usize,
    pub livestock: Vec<String>,
    pub critters: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct K8sNodeInfo {
    pub name: String,
    pub internal_ip: String,
    pub external_ip: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct K8sSyncResult {
    pub barns: Vec<Barn>,
    pub livestock: Vec<Livestock>,
    pub critters: Vec<Critter>,
    pub herds: Vec<Herd>,
}

// ============================================================================
// Helper functions
// ============================================================================

fn kubectl<T: serde::de::DeserializeOwned>(
    context: &str,
    args: &str,
    kubeconfig_path: Option<&str>,
) -> Result<T, String> {
    let mut cmd_str = String::from("kubectl");
    if let Some(kp) = kubeconfig_path {
        cmd_str.push_str(&format!(" --kubeconfig={}", shell_escape(kp)));
    }
    cmd_str.push_str(&format!(" --context={} {} -o json", shell_escape(context), args));

    let output = Command::new("sh")
        .args(["-c", &cmd_str])
        .output()
        .map_err(|e| format!("kubectl command failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("kubectl command failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("Failed to parse kubectl output: {}", e))
}

fn is_private_image(image: &str, private_registries: &[String]) -> bool {
    private_registries.iter().any(|r| image.starts_with(r.as_str()))
}

fn get_deployment_name(pod: &K8sPod) -> Option<String> {
    let rs = pod.metadata.owner_references.iter().find(|r| r.kind == "ReplicaSet")?;
    let parts: Vec<&str> = rs.name.rsplitn(2, '-').collect();
    if parts.len() >= 2 {
        Some(parts[1].to_string())
    } else {
        None
    }
}

fn get_image_tag(image: &str) -> Option<String> {
    let parts: Vec<&str> = image.split(':').collect();
    if parts.len() > 1 {
        Some(parts.last().unwrap().to_string())
    } else {
        None
    }
}

fn derive_service_name(image: &str) -> String {
    let without_tag = image.split(':').next().unwrap_or(image);
    let parts: Vec<&str> = without_tag.split('/').collect();
    parts.last().unwrap_or(&"unknown").to_string()
}

// ============================================================================
// Public API
// ============================================================================

/// Discover available resources from a K8s cluster
pub fn discover_k8s_resources(
    context: &str,
    kubeconfig_path: Option<&str>,
    private_registries: &[String],
) -> Result<K8sDiscoveryResult, String> {
    let ns_response: K8sListResponse<K8sNamespace> =
        kubectl(context, "get namespaces", kubeconfig_path)?;

    let node_response: K8sListResponse<K8sNode> =
        kubectl(context, "get nodes", kubeconfig_path)?;

    let pod_response: K8sListResponse<K8sPod> =
        kubectl(context, "get pods --all-namespaces", kubeconfig_path)?;

    let nodes: Vec<K8sNodeInfo> = node_response.items.iter().map(|node| {
        let internal_ip = node.status.addresses.iter()
            .find(|a| a.addr_type == "InternalIP")
            .map(|a| a.address.clone())
            .unwrap_or_default();
        let external_ip = node.status.addresses.iter()
            .find(|a| a.addr_type == "ExternalIP")
            .map(|a| a.address.clone());
        K8sNodeInfo { name: node.metadata.name.clone(), internal_ip, external_ip }
    }).collect();

    let namespaces: Vec<K8sNamespaceInfo> = ns_response.items.iter().map(|ns| {
        let ns_pods: Vec<&K8sPod> = pod_response.items.iter()
            .filter(|p| p.metadata.namespace == ns.metadata.name && p.status.phase == "Running")
            .collect();

        let mut livestock = Vec::new();
        let mut critters = Vec::new();

        for pod in &ns_pods {
            if let Some(container) = pod.spec.containers.first() {
                let is_ls = is_private_image(&container.image, private_registries);
                let deployment_name = get_deployment_name(pod);
                let display_name = deployment_name.unwrap_or_else(|| pod.metadata.name.clone());

                if is_ls {
                    if !livestock.contains(&display_name) {
                        livestock.push(display_name);
                    }
                } else if !critters.contains(&display_name) {
                    critters.push(display_name);
                }
            }
        }

        K8sNamespaceInfo {
            name: ns.metadata.name.clone(),
            livestock_count: livestock.len(),
            critter_count: critters.len(),
            livestock,
            critters,
        }
    }).collect();

    Ok(K8sDiscoveryResult { namespaces, nodes })
}

/// Sync resources from K8s cluster based on ranch hand configuration
pub fn sync_k8s_resources(ranchhand: &RanchHand) -> Result<K8sSyncResult, String> {
    let config = &ranchhand.config;
    let context = config["context"].as_str().ok_or("Missing context in ranchhand config")?;
    let kubeconfig_path = config["kubeconfig_path"].as_str();
    let private_registries: Vec<String> = config["private_registries"]
        .as_sequence()
        .map(|seq| seq.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let source_tag = format!("ranchhand:{}", ranchhand.name);

    // Get nodes
    let node_response: K8sListResponse<K8sNode> =
        kubectl(context, "get nodes", kubeconfig_path)?;

    // Get pods for the herd namespace
    let mut all_pods: Vec<K8sPod> = Vec::new();
    if !ranchhand.herd.is_empty() {
        let pod_cmd = format!("get pods -n {}", shell_escape(&ranchhand.herd));
        if let Ok(resp) = kubectl::<K8sListResponse<K8sPod>>(context, &pod_cmd, kubeconfig_path) {
            all_pods = resp.items;
        }
    }

    // Create barns from nodes
    let barns: Vec<Barn> = node_response.items.iter().map(|node| {
        let internal_ip = node.status.addresses.iter()
            .find(|a| a.addr_type == "InternalIP")
            .map(|a| a.address.clone())
            .unwrap_or_default();

        Barn {
            name: node.metadata.name.clone(),
            host: Some(internal_ip),
            user: None,
            port: None,
            identity_file: None,
            critters: vec![],
            source: Some(source_tag.clone()),
            connection_type: Some("kubernetes".to_string()),
            connection_config: Some(K8sBarnConnectionConfig {
                context: context.to_string(),
                node: node.metadata.name.clone(),
            }),
            connectable: Some(false),
        }
    }).collect();

    // Process pods
    let mut livestock_list = Vec::new();
    let mut critter_list = Vec::new();
    let mut herd_livestock = Vec::new();
    let mut herd_critter_refs = Vec::new();

    for pod in &all_pods {
        if pod.status.phase != "Running" {
            continue;
        }
        let namespace = &pod.metadata.namespace;
        let container = match pod.spec.containers.first() {
            Some(c) => c,
            None => continue,
        };

        let is_ls = is_private_image(&container.image, &private_registries);
        let deployment_name = get_deployment_name(pod);

        if is_ls {
            let ls = Livestock {
                name: pod.metadata.name.clone(),
                path: format!("/var/run/containers/{}", pod.metadata.name),
                barn: Some(pod.spec.node_name.clone()),
                repo: None,
                branch: None,
                log_path: None,
                env_path: None,
                source: Some(source_tag.clone()),
                k8s_metadata: Some(K8sLivestockMetadata {
                    namespace: namespace.clone(),
                    pod_name: pod.metadata.name.clone(),
                    deployment: deployment_name,
                    image: container.image.clone(),
                    image_tag: get_image_tag(&container.image),
                }),
                trails: vec![],
            };
            if !herd_livestock.contains(&pod.metadata.name) {
                herd_livestock.push(pod.metadata.name.clone());
            }
            livestock_list.push(ls);
        } else {
            let cr = Critter {
                name: pod.metadata.name.clone(),
                service: derive_service_name(&container.image),
                service_path: None,
                config_path: None,
                log_path: None,
                use_journald: None,
                source: Some(source_tag.clone()),
                endpoint: None,
                port: None,
                k8s_metadata: Some(K8sCritterMetadata {
                    namespace: namespace.clone(),
                    pod_name: pod.metadata.name.clone(),
                    image: container.image.clone(),
                }),
                tf_metadata: None,
            };
            let ref_entry = HerdCritterRef {
                barn: pod.spec.node_name.clone(),
                critter: pod.metadata.name.clone(),
            };
            if !herd_critter_refs.iter().any(|r: &HerdCritterRef| r.critter == pod.metadata.name) {
                herd_critter_refs.push(ref_entry);
            }
            critter_list.push(cr);
        }
    }

    let herds = if !ranchhand.herd.is_empty() {
        vec![Herd {
            name: ranchhand.herd.clone(),
            livestock: herd_livestock,
            critters: herd_critter_refs,
            connections: vec![],
        }]
    } else {
        vec![]
    };

    Ok(K8sSyncResult {
        barns,
        livestock: livestock_list,
        critters: critter_list,
        herds,
    })
}

/// Get available kubectl contexts
pub fn get_kubectl_contexts(kubeconfig_path: Option<&str>) -> Result<Vec<String>, String> {
    let mut cmd_str = String::from("kubectl");
    if let Some(kp) = kubeconfig_path {
        cmd_str.push_str(&format!(" --kubeconfig={}", shell_escape(kp)));
    }
    cmd_str.push_str(" config get-contexts -o name");

    let output = Command::new("sh")
        .args(["-c", &cmd_str])
        .output()
        .map_err(|e| format!("Failed to get kubectl contexts: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to get kubectl contexts: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().lines().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect())
}

/// Get current kubectl context
pub fn get_current_kubectl_context(kubeconfig_path: Option<&str>) -> Option<String> {
    let mut cmd_str = String::from("kubectl");
    if let Some(kp) = kubeconfig_path {
        cmd_str.push_str(&format!(" --kubeconfig={}", shell_escape(kp)));
    }
    cmd_str.push_str(" config current-context");

    Command::new("sh")
        .args(["-c", &cmd_str])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}
