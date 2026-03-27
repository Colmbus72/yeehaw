use std::fs;
use std::process::Command;

use crate::tmux::shell_escape;
use crate::types::*;

// ============================================================================
// Terraform state types
// ============================================================================

#[derive(Debug, serde::Deserialize)]
struct TerraformState {
    resources: Vec<TerraformResource>,
}

#[derive(Debug, serde::Deserialize)]
struct TerraformResource {
    mode: String,
    #[serde(rename = "type")]
    resource_type: String,
    name: String,
    instances: Vec<TerraformInstance>,
}

#[derive(Debug, serde::Deserialize)]
struct TerraformInstance {
    attributes: serde_json::Value,
}

// ============================================================================
// Discovery result types
// ============================================================================

#[derive(Debug, Clone, serde::Serialize)]
pub struct TerraformDiscoveryResult {
    pub resources: Vec<TerraformResourceInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TerraformResourceInfo {
    pub id: String,
    pub resource_type: String,
    pub name: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    pub port: Option<u16>,
    pub suggested_herd: Option<String>,
    pub yeehaw_type: String, // "critter", "barn", "skip"
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TerraformSyncResult {
    pub critters: Vec<Critter>,
    pub barns: Vec<Barn>,
}

// ============================================================================
// Resource type mappings
// ============================================================================

struct ResourceMapping {
    yeehaw_type: &'static str,
    service: &'static str,
    get_endpoint: fn(&serde_json::Value) -> Option<String>,
    get_port: fn(&serde_json::Value) -> Option<u16>,
}

fn get_resource_mapping(resource_type: &str) -> Option<&'static ResourceMapping> {
    static MAPPINGS: &[(&str, ResourceMapping)] = &[
        ("aws_db_instance", ResourceMapping {
            yeehaw_type: "critter",
            service: "postgresql",
            get_endpoint: |attrs| attrs["endpoint"].as_str().map(|s| s.to_string()),
            get_port: |attrs| attrs["port"].as_u64().map(|p| p as u16),
        }),
        ("aws_rds_cluster", ResourceMapping {
            yeehaw_type: "critter",
            service: "aurora",
            get_endpoint: |attrs| attrs["endpoint"].as_str().map(|s| s.to_string()),
            get_port: |attrs| attrs["port"].as_u64().map(|p| p as u16),
        }),
        ("aws_elasticache_cluster", ResourceMapping {
            yeehaw_type: "critter",
            service: "redis",
            get_endpoint: |attrs| {
                attrs["cache_nodes"].as_array()
                    .and_then(|nodes| nodes.first())
                    .and_then(|n| n["address"].as_str().map(|s| s.to_string()))
            },
            get_port: |attrs| attrs["port"].as_u64().map(|p| p as u16),
        }),
        ("aws_elasticache_replication_group", ResourceMapping {
            yeehaw_type: "critter",
            service: "redis",
            get_endpoint: |attrs| attrs["primary_endpoint_address"].as_str().map(|s| s.to_string()),
            get_port: |attrs| attrs["port"].as_u64().map(|p| p as u16),
        }),
        ("aws_mq_broker", ResourceMapping {
            yeehaw_type: "critter",
            service: "rabbitmq",
            get_endpoint: |attrs| {
                attrs["instances"].as_array()
                    .and_then(|inst| inst.first())
                    .and_then(|i| i["endpoints"].as_array())
                    .and_then(|eps| eps.first())
                    .and_then(|e| e.as_str().map(|s| s.to_string()))
            },
            get_port: |_| Some(5672),
        }),
        ("aws_opensearch_domain", ResourceMapping {
            yeehaw_type: "critter",
            service: "opensearch",
            get_endpoint: |attrs| attrs["endpoint"].as_str().map(|s| s.to_string()),
            get_port: |_| Some(443),
        }),
        ("aws_elasticsearch_domain", ResourceMapping {
            yeehaw_type: "critter",
            service: "elasticsearch",
            get_endpoint: |attrs| attrs["endpoint"].as_str().map(|s| s.to_string()),
            get_port: |_| Some(443),
        }),
        ("aws_instance", ResourceMapping {
            yeehaw_type: "barn",
            service: "ec2",
            get_endpoint: |attrs| {
                attrs["public_ip"].as_str()
                    .or_else(|| attrs["private_ip"].as_str())
                    .map(|s| s.to_string())
            },
            get_port: |_| Some(22),
        }),
    ];

    MAPPINGS.iter().find(|(t, _)| *t == resource_type).map(|(_, m)| m)
}

// ============================================================================
// Helper functions
// ============================================================================

fn fetch_state_from_s3(bucket: &str, key: &str, region: &str) -> Result<TerraformState, String> {
    let s3_path = format!("s3://{}/{}", bucket, key);
    let cmd = format!("aws s3 cp {} - --region {}", shell_escape(&s3_path), shell_escape(region));

    let output = Command::new("sh")
        .args(["-c", &cmd])
        .output()
        .map_err(|e| format!("Failed to fetch Terraform state from S3: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to fetch Terraform state from S3: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| format!("Failed to parse Terraform state: {}", e))
}

fn read_state_from_file(path: &str) -> Result<TerraformState, String> {
    let content = fs::read_to_string(path)
        .map_err(|_| format!("Terraform state file not found: {}", path))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse Terraform state: {}", e))
}

fn suggest_herd(
    resource_name: &str,
    attrs: &serde_json::Value,
    existing_herds: &[String],
) -> Option<String> {
    let name_lower = resource_name.to_lowercase();
    let identifier = attrs["identifier"].as_str().unwrap_or("").to_lowercase();

    // Priority 1: Check tags for environment
    if let Some(env_tag) = attrs["tags"]["environment"].as_str() {
        let env_lower = env_tag.to_lowercase();
        if let Some(m) = existing_herds.iter().find(|h| h.to_lowercase() == env_lower) {
            return Some(m.clone());
        }
    }

    // Priority 2: Check for common environment keywords
    let search_text = format!("{} {}", name_lower, identifier);
    let patterns = [
        (&["prod", "production"][..], "production"),
        (&["staging", "stage"][..], "staging"),
        (&["dev", "develop", "development"][..], "development"),
        (&["test", "testing"][..], "testing"),
    ];

    for (keywords, herd_name) in &patterns {
        for keyword in *keywords {
            if search_text.contains(keyword) {
                if let Some(m) = existing_herds.iter().find(|h| {
                    let hl = h.to_lowercase();
                    hl.contains(keyword) || hl == *herd_name
                }) {
                    return Some(m.clone());
                }
                return Some(herd_name.to_string());
            }
        }
    }

    None
}

fn derive_service_type(resource_type: &str, attrs: &serde_json::Value) -> String {
    let mapping = match get_resource_mapping(resource_type) {
        Some(m) => m,
        None => return "unknown".to_string(),
    };

    if resource_type == "aws_db_instance" {
        let engine = attrs["engine"].as_str().unwrap_or("").to_lowercase();
        if engine.contains("mysql") || engine.contains("mariadb") {
            return "mysql".to_string();
        }
        if engine.contains("postgres") {
            return "postgresql".to_string();
        }
        return if engine.is_empty() { "database".to_string() } else { engine };
    }

    mapping.service.to_string()
}

fn load_state(config: &serde_yaml::Value) -> Result<TerraformState, String> {
    let backend = config["backend"].as_str().unwrap_or("local");
    if backend == "s3" {
        let bucket = config["bucket"].as_str().ok_or("S3 backend requires bucket")?;
        let key = config["key"].as_str().ok_or("S3 backend requires key")?;
        let region = config["region"].as_str().ok_or("S3 backend requires region")?;
        fetch_state_from_s3(bucket, key, region)
    } else {
        let local_path = config["local_path"].as_str().ok_or("Local backend requires local_path")?;
        read_state_from_file(local_path)
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Discover resources from Terraform state
pub fn discover_terraform_resources(
    config: &serde_yaml::Value,
    existing_herds: &[String],
) -> Result<TerraformDiscoveryResult, String> {
    let state = load_state(config)?;

    let mut resources = Vec::new();

    for resource in &state.resources {
        if resource.mode != "managed" {
            continue;
        }
        let mapping = match get_resource_mapping(&resource.resource_type) {
            Some(m) => m,
            None => continue,
        };

        for instance in &resource.instances {
            let attrs = &instance.attributes;
            let id = format!("{}.{}", resource.resource_type, resource.name);

            let identifier = attrs["identifier"].as_str();
            let tag_name = attrs["tags"]["Name"].as_str();
            let display_name = identifier
                .or(tag_name)
                .unwrap_or(&resource.name)
                .to_string();

            resources.push(TerraformResourceInfo {
                id,
                resource_type: resource.resource_type.clone(),
                name: resource.name.clone(),
                display_name,
                endpoint: (mapping.get_endpoint)(attrs),
                port: (mapping.get_port)(attrs),
                suggested_herd: suggest_herd(&resource.name, attrs, existing_herds),
                yeehaw_type: mapping.yeehaw_type.to_string(),
            });
        }
    }

    Ok(TerraformDiscoveryResult { resources })
}

/// Sync resources from Terraform state based on ranch hand configuration
pub fn sync_terraform_resources(ranchhand: &RanchHand) -> Result<TerraformSyncResult, String> {
    let state = load_state(&ranchhand.config)?;
    let source_tag = format!("ranchhand:{}", ranchhand.name);

    let mut critters = Vec::new();
    let mut barns = Vec::new();

    for resource in &state.resources {
        if resource.mode != "managed" {
            continue;
        }
        let mapping = match get_resource_mapping(&resource.resource_type) {
            Some(m) => m,
            None => continue,
        };

        for instance in &resource.instances {
            let attrs = &instance.attributes;
            let id = format!("{}.{}", resource.resource_type, resource.name);

            // Check if this resource has a mapping to the ranchhand's herd
            let resource_mapping = ranchhand.resource_mappings.iter().find(|m| m.resource_id == id);
            match resource_mapping {
                Some(rm) if rm.herd_name == ranchhand.herd => {}
                _ => continue,
            }

            let identifier = attrs["identifier"].as_str();
            let tag_name = attrs["tags"]["Name"].as_str();
            let display_name = identifier
                .or(tag_name)
                .unwrap_or(&resource.name)
                .to_string();

            if mapping.yeehaw_type == "critter" {
                critters.push(Critter {
                    name: display_name,
                    service: derive_service_type(&resource.resource_type, attrs),
                    service_path: None,
                    config_path: None,
                    log_path: None,
                    use_journald: None,
                    source: Some(source_tag.clone()),
                    endpoint: (mapping.get_endpoint)(attrs),
                    port: (mapping.get_port)(attrs),
                    k8s_metadata: None,
                    tf_metadata: Some(TerraformCritterMetadata {
                        resource_type: resource.resource_type.clone(),
                        resource_name: resource.name.clone(),
                    }),
                });
            } else if mapping.yeehaw_type == "barn" {
                let endpoint = (mapping.get_endpoint)(attrs);
                barns.push(Barn {
                    name: display_name,
                    host: endpoint.clone(),
                    user: None,
                    port: None,
                    identity_file: None,
                    critters: vec![],
                    source: Some(source_tag.clone()),
                    connection_type: Some("terraform".to_string()),
                    connection_config: None,
                    connectable: Some(endpoint.is_some()),
                });
            }
        }
    }

    Ok(TerraformSyncResult { critters, barns })
}

/// Test S3 access for Terraform state
pub fn test_s3_access(bucket: &str, key: &str, region: &str) -> bool {
    let s3_path = format!("s3://{}/{}", bucket, key);
    let cmd = format!("aws s3 ls {} --region {}", shell_escape(&s3_path), shell_escape(region));

    Command::new("sh")
        .args(["-c", &cmd])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// List available Terraform state files in an S3 bucket prefix
pub fn list_s3_state_files(bucket: &str, prefix: &str, region: &str) -> Vec<String> {
    let s3_path = format!("s3://{}/{}", bucket, prefix);
    let cmd = format!("aws s3 ls {} --recursive --region {}", shell_escape(&s3_path), shell_escape(region));

    let output = match Command::new("sh").args(["-c", &cmd]).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return vec![],
    };

    output
        .lines()
        .filter(|line| line.contains(".tfstate") && !line.contains(".tfstate.backup"))
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.last().map(|s| s.to_string())
        })
        .collect()
}
