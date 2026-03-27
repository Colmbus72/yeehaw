use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::types::*;

// ============================================================================
// Paths
// ============================================================================

pub fn yeehaw_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".yeehaw")
}

pub fn config_file() -> PathBuf { yeehaw_dir().join("config.yaml") }
pub fn projects_dir() -> PathBuf { yeehaw_dir().join("projects") }
pub fn barns_dir() -> PathBuf { yeehaw_dir().join("barns") }
pub fn ranchhands_dir() -> PathBuf { yeehaw_dir().join("ranchhands") }
pub fn worms_dir() -> PathBuf { yeehaw_dir().join("worms") }
pub fn worm_runs_dir() -> PathBuf { yeehaw_dir().join("worm-runs") }
pub fn worm_triggers_dir() -> PathBuf { yeehaw_dir().join("worm-triggers") }
pub fn sessions_dir() -> PathBuf { yeehaw_dir().join("sessions") }
pub fn slack_dir() -> PathBuf { yeehaw_dir().join("slack") }
pub fn slack_results_dir() -> PathBuf { slack_dir().join("results") }
pub fn signals_dir() -> PathBuf { yeehaw_dir().join("session-signals") }
pub fn bin_dir() -> PathBuf { yeehaw_dir().join("bin") }
pub fn trails_dir() -> PathBuf { yeehaw_dir().join("trails") }
pub fn trail_runs_dir() -> PathBuf { yeehaw_dir().join("trail-runs") }
pub fn poll_state_dir() -> PathBuf { yeehaw_dir().join("poll-state") }
pub fn vault_file() -> PathBuf { yeehaw_dir().join("vault.enc") }
pub fn vault_trigger_file() -> PathBuf { yeehaw_dir().join("vault-trigger") }

pub fn worm_runs_for(worm_name: &str) -> PathBuf {
    worm_runs_dir().join(worm_name)
}

pub fn trail_run_dir_for(livestock_name: &str, trail_name: &str, timestamp: &str) -> PathBuf {
    trail_runs_dir().join(format!("{}--{}--{}", livestock_name, trail_name, timestamp))
}

fn validate_name(name: &str, entity_type: &str) -> Result<()> {
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
        anyhow::bail!("Invalid {} name: contains forbidden characters", entity_type);
    }
    Ok(())
}

// ============================================================================
// Directory setup
// ============================================================================

pub fn ensure_config_dirs() {
    let dirs = [
        yeehaw_dir(),
        projects_dir(),
        barns_dir(),
        ranchhands_dir(),
        sessions_dir(),
        worms_dir(),
        worm_runs_dir(),
        worm_triggers_dir(),
        slack_dir(),
        slack_results_dir(),
        signals_dir(),
        bin_dir(),
        trails_dir(),
        trail_runs_dir(),
        poll_state_dir(),
        skills_dir(),
    ];
    for dir in &dirs {
        if !dir.exists() {
            let _ = fs::create_dir_all(dir);
        }
    }

    // Auto-install bundled skill if not already present
    if !crate::hooks::skill_installed() {
        let _ = crate::hooks::install_skill();
    }
}

pub fn skills_dir() -> PathBuf {
    yeehaw_dir().join("skills")
}

// ============================================================================
// Config
// ============================================================================

pub fn load_config() -> Config {
    ensure_config_dirs();
    let path = config_file();

    if !path.exists() {
        let config = Config::default();
        let content = serde_yaml::to_string(&config).unwrap_or_default();
        let _ = fs::write(&path, content);
        return config;
    }

    let content = fs::read_to_string(&path).unwrap_or_default();
    serde_yaml::from_str(&content).unwrap_or_default()
}

// ============================================================================
// Projects
// ============================================================================

pub fn load_projects() -> Vec<Project> {
    ensure_config_dirs();
    let dir = projects_dir();
    if !dir.exists() {
        return vec![];
    }

    let mut projects = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(project) = serde_yaml::from_str::<Project>(&content) {
                        // Normalize: ensure livestock vec exists
                        if project.livestock.is_empty() {
                            // Already default empty vec from serde
                        }
                        projects.push(project);
                    }
                }
            }
        }
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
}

pub fn save_project(project: &Project) -> Result<()> {
    ensure_config_dirs();
    validate_name(&project.name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project.name));
    let content = serde_yaml::to_string(project).context("Failed to serialize project")?;
    fs::write(&path, content).context("Failed to write project file")?;
    Ok(())
}

pub fn delete_project(name: &str) -> Result<bool> {
    validate_name(name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).context("Failed to delete project")?;
    Ok(true)
}

pub fn add_livestock_to_project(project_name: &str, livestock: &Livestock) -> Result<()> {
    validate_name(project_name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if project.livestock.iter().any(|l| l.name == livestock.name) {
        anyhow::bail!("Livestock '{}' already exists in project '{}'", livestock.name, project_name);
    }

    project.livestock.push(livestock.clone());

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    fs::write(&path, new_content).context("Failed to write project file")?;
    Ok(())
}

pub fn update_livestock_in_project(project_name: &str, original_name: &str, updated: &Livestock) -> Result<()> {
    validate_name(project_name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == original_name) {
        *ls = updated.clone();
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", original_name, project_name);
    }

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    fs::write(&path, new_content).context("Failed to write project file")?;
    Ok(())
}

// ============================================================================
// Barns
// ============================================================================

pub const LOCAL_BARN_NAME: &str = "local";

pub fn local_barn() -> Barn {
    Barn {
        name: LOCAL_BARN_NAME.to_string(),
        host: None,
        user: None,
        port: None,
        identity_file: None,
        critters: vec![],
        source: None,
        connection_type: None,
        connection_config: None,
        connectable: None,
    }
}

pub fn is_local_barn(barn: &Barn) -> bool {
    barn.name == LOCAL_BARN_NAME
}

pub fn load_barns() -> Vec<Barn> {
    ensure_config_dirs();
    let mut barns = vec![local_barn()];
    let dir = barns_dir();
    if !dir.exists() {
        return barns;
    }

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(barn) = serde_yaml::from_str::<Barn>(&content) {
                        if barn.name != LOCAL_BARN_NAME {
                            barns.push(barn);
                        }
                    }
                }
            }
        }
    }
    barns
}

pub fn save_barn(barn: &Barn) -> Result<()> {
    ensure_config_dirs();
    validate_name(&barn.name, "barn")?;
    let path = barns_dir().join(format!("{}.yaml", barn.name));
    let content = serde_yaml::to_string(barn).context("Failed to serialize barn")?;
    fs::write(&path, content).context("Failed to write barn file")?;
    Ok(())
}

pub fn delete_barn(name: &str) -> Result<bool> {
    if name == LOCAL_BARN_NAME {
        return Ok(false);
    }
    validate_name(name, "barn")?;
    let path = barns_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).context("Failed to delete barn")?;
    Ok(true)
}

pub fn update_critter_in_barn(barn_name: &str, original_name: &str, updated: &Critter) -> Result<()> {
    if barn_name == LOCAL_BARN_NAME {
        anyhow::bail!("Cannot edit critters on the local barn");
    }
    validate_name(barn_name, "barn")?;
    let path = barns_dir().join(format!("{}.yaml", barn_name));
    let content = fs::read_to_string(&path).context("Failed to read barn file")?;
    let mut barn: Barn = serde_yaml::from_str(&content).context("Failed to parse barn")?;

    if let Some(cr) = barn.critters.iter_mut().find(|c| c.name == original_name) {
        *cr = updated.clone();
    } else {
        anyhow::bail!("Critter '{}' not found in barn '{}'", original_name, barn_name);
    }

    let new_content = serde_yaml::to_string(&barn).context("Failed to serialize barn")?;
    fs::write(&path, new_content).context("Failed to write barn file")?;
    Ok(())
}

pub fn get_livestock_for_barn(barn_name: &str) -> Vec<(Project, Livestock)> {
    let projects = load_projects();
    let mut result = Vec::new();
    for project in &projects {
        for livestock in &project.livestock {
            let matches = if barn_name == LOCAL_BARN_NAME {
                livestock.barn.is_none()
            } else {
                livestock.barn.as_deref() == Some(barn_name)
            };
            if matches {
                result.push((project.clone(), livestock.clone()));
            }
        }
    }
    result
}

// ============================================================================
// Worms
// ============================================================================

pub fn load_worms() -> Vec<Worm> {
    ensure_config_dirs();
    let dir = worms_dir();
    if !dir.exists() {
        return vec![];
    }

    let mut worms = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(worm) = serde_yaml::from_str::<Worm>(&content) {
                        worms.push(worm);
                    }
                }
            }
        }
    }
    worms.sort_by(|a, b| a.name.cmp(&b.name));
    worms
}

pub fn save_worm(worm: &Worm) -> Result<()> {
    ensure_config_dirs();
    validate_name(&worm.name, "worm")?;
    let path = worms_dir().join(format!("{}.yaml", worm.name));
    let content = serde_yaml::to_string(worm).context("Failed to serialize worm")?;
    fs::write(&path, content).context("Failed to write worm file")?;
    Ok(())
}

pub fn delete_worm(name: &str) -> Result<bool> {
    validate_name(name, "worm")?;
    let path = worms_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).context("Failed to delete worm")?;
    Ok(true)
}

pub fn load_worm_runs(worm_name: &str) -> Vec<WormRun> {
    let dir = worm_runs_for(worm_name);
    if !dir.exists() {
        return vec![];
    }

    let mut runs = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(run) = serde_yaml::from_str::<WormRun>(&content) {
                        runs.push(run);
                    }
                }
            }
        }
    }
    // Sort by started_at descending (most recent first)
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs
}

pub fn save_worm_run(worm_name: &str, run: &WormRun) -> Result<()> {
    let dir = worm_runs_for(worm_name);
    if !dir.exists() {
        fs::create_dir_all(&dir).context("Failed to create worm runs directory")?;
    }
    let filename = format!("{}.yaml", run.started_at.replace(':', "-"));
    let path = dir.join(filename);
    let content = serde_yaml::to_string(run).context("Failed to serialize worm run")?;
    fs::write(&path, content).context("Failed to write worm run file")?;
    Ok(())
}

// ============================================================================
// Trails
// ============================================================================

pub fn load_trail(name: &str) -> Option<crate::trails::Trail> {
    let path = trails_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_yaml::from_str(&content).ok()
}

pub fn load_all_trails() -> Vec<crate::trails::Trail> {
    ensure_config_dirs();
    let dir = trails_dir();
    if !dir.exists() {
        return vec![];
    }

    let mut trails = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(trail) = serde_yaml::from_str::<crate::trails::Trail>(&content) {
                        trails.push(trail);
                    }
                }
            }
        }
    }
    trails.sort_by(|a, b| a.name.cmp(&b.name));
    trails
}

pub fn load_trails_for_livestock(livestock: &Livestock) -> Vec<crate::trails::Trail> {
    livestock.trails.iter()
        .filter_map(|name| load_trail(name))
        .collect()
}

pub fn link_trail_to_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    validate_name(project_name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == livestock_name) {
        if !ls.trails.contains(&trail_name.to_string()) {
            ls.trails.push(trail_name.to_string());
        }
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", livestock_name, project_name);
    }

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    fs::write(&path, new_content).context("Failed to write project file")?;

    // Auto-create poll worm if trail has on:push trigger
    if let Some(trail) = load_trail(trail_name) {
        if trail.has_push_trigger() {
            let _ = create_poll_worm(
                livestock_name,
                trail_name,
                trail.poll_interval(),
                Some(project_name),
            );
        }
    }

    Ok(())
}

pub fn unlink_trail_from_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    validate_name(project_name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == livestock_name) {
        ls.trails.retain(|t| t != trail_name);
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", livestock_name, project_name);
    }

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    fs::write(&path, new_content).context("Failed to write project file")?;

    // Remove poll worm if it exists
    let _ = remove_poll_worm(livestock_name, trail_name);

    Ok(())
}

pub fn save_trail(trail: &crate::trails::Trail) -> Result<()> {
    ensure_config_dirs();
    validate_name(&trail.name, "trail")?;
    let path = trails_dir().join(format!("{}.yaml", trail.name));
    let content = serde_yaml::to_string(trail).context("Failed to serialize trail")?;
    fs::write(&path, content).context("Failed to write trail file")?;
    Ok(())
}

pub fn delete_trail(name: &str) -> Result<bool> {
    validate_name(name, "trail")?;
    let path = trails_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).context("Failed to delete trail")?;
    Ok(true)
}

// ============================================================================
// Trail Runs
// ============================================================================

pub fn save_trail_run(run: &crate::trails::TrailRun, run_dir: &std::path::Path) -> Result<()> {
    if !run_dir.exists() {
        fs::create_dir_all(run_dir).context("Failed to create trail run directory")?;
    }
    let path = run_dir.join("run.json");
    let content = serde_json::to_string_pretty(run).context("Failed to serialize trail run")?;
    fs::write(&path, content).context("Failed to write trail run")?;
    Ok(())
}

pub fn load_trail_runs(livestock_name: &str, trail_name: &str) -> Vec<crate::trails::TrailRun> {
    let dir = trail_runs_dir();
    if !dir.exists() {
        return vec![];
    }

    let prefix = format!("{}--{}--", livestock_name, trail_name);
    let mut runs = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && entry.path().is_dir() {
                let run_path = entry.path().join("run.json");
                if let Ok(content) = fs::read_to_string(&run_path) {
                    if let Ok(run) = serde_json::from_str::<crate::trails::TrailRun>(&content) {
                        runs.push(run);
                    }
                }
            }
        }
    }

    // Most recent first
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs
}

pub fn load_trail_step_log(run_dir: &std::path::Path, step_index: usize) -> Option<String> {
    let log_path = run_dir.join(format!("step-{}.log", step_index));
    fs::read_to_string(&log_path).ok()
}

/// Count existing runs for a livestock+trail pair (for $RUN_NUMBER).
pub fn count_trail_runs(livestock_name: &str, trail_name: &str) -> u64 {
    load_trail_runs(livestock_name, trail_name).len() as u64
}

// ============================================================================
// Poll State
// ============================================================================

/// Read the last known SHA for a livestock+branch polling pair.
pub fn read_poll_sha(livestock_name: &str, branch: &str) -> Option<String> {
    let path = poll_state_dir().join(format!("{}--{}.sha", livestock_name, branch));
    std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string())
}

/// Write the current SHA for a livestock+branch polling pair.
pub fn write_poll_sha(livestock_name: &str, branch: &str, sha: &str) -> Result<()> {
    let dir = poll_state_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}--{}.sha", livestock_name, branch));
    std::fs::write(&path, sha)?;
    Ok(())
}

/// Delete poll state for a livestock+branch pair.
pub fn delete_poll_sha(livestock_name: &str, branch: &str) -> Result<()> {
    let path = poll_state_dir().join(format!("{}--{}.sha", livestock_name, branch));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

// ============================================================================
// Poll Worm Helpers
// ============================================================================

/// Create a poll worm for a trail with on:push trigger.
pub fn create_poll_worm(
    livestock_name: &str,
    trail_name: &str,
    poll_interval_secs: u64,
    project_name: Option<&str>,
) -> Result<()> {
    let worm_name = format!("poll--{}--{}", livestock_name, trail_name);

    let schedule = if poll_interval_secs < 60 {
        "* * * * *".to_string()
    } else {
        let mins = poll_interval_secs / 60;
        format!("*/{} * * * *", mins)
    };

    let worm = crate::types::Worm {
        name: worm_name,
        command: format!("yeehaw trail poll {} {}", livestock_name, trail_name),
        schedule,
        worm_type: "shell".to_string(),
        enabled: true,
        project: project_name.map(|s| s.to_string()),
        working_dir: None,
    };

    save_worm(&worm)?;
    crate::crontab::sync_crontab()?;
    Ok(())
}

/// Remove a poll worm for a trail.
pub fn remove_poll_worm(livestock_name: &str, trail_name: &str) -> Result<()> {
    let worm_name = format!("poll--{}--{}", livestock_name, trail_name);
    delete_worm(&worm_name)?;
    crate::crontab::sync_crontab()?;
    Ok(())
}

// ============================================================================
// Ranch Hands
// ============================================================================

pub fn load_ranchhands() -> Vec<RanchHand> {
    ensure_config_dirs();
    let dir = ranchhands_dir();
    if !dir.exists() {
        return vec![];
    }

    let mut ranchhands = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(rh) = serde_yaml::from_str::<RanchHand>(&content) {
                        ranchhands.push(rh);
                    }
                }
            }
        }
    }
    ranchhands
}

pub fn load_ranchhands_for_project(project_name: &str) -> Vec<RanchHand> {
    load_ranchhands()
        .into_iter()
        .filter(|rh| rh.project == project_name)
        .collect()
}

pub fn save_ranchhand(rh: &RanchHand) -> Result<()> {
    ensure_config_dirs();
    let path = ranchhands_dir().join(format!("{}.yaml", rh.name));
    let content = serde_yaml::to_string(rh).context("Failed to serialize ranchhand")?;
    fs::write(&path, content).context("Failed to write ranchhand")?;
    Ok(())
}

pub fn delete_ranchhand(name: &str) -> Result<bool> {
    validate_name(name, "ranchhand")?;
    let path = ranchhands_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).context("Failed to delete ranchhand")?;
    Ok(true)
}

pub fn update_ranchhand_last_sync(name: &str) -> Result<()> {
    let mut ranchhands = load_ranchhands();
    let rh = ranchhands.iter_mut().find(|r| r.name == name)
        .ok_or_else(|| anyhow::anyhow!("Ranch hand not found: {}", name))?;
    rh.last_sync = Some(chrono::Utc::now().to_rfc3339());
    save_ranchhand(rh)?;
    Ok(())
}

pub fn add_ranchhand_resource_mapping(name: &str, resource_id: &str, herd_name: &str) -> Result<()> {
    let mut ranchhands = load_ranchhands();
    let rh = ranchhands.iter_mut().find(|r| r.name == name)
        .ok_or_else(|| anyhow::anyhow!("Ranch hand not found: {}", name))?;
    // Remove existing mapping for this resource if any
    rh.resource_mappings.retain(|m| m.resource_id != resource_id);
    // Add new mapping
    rh.resource_mappings.push(crate::types::ResourceMapping {
        resource_id: resource_id.to_string(),
        herd_name: herd_name.to_string(),
    });
    save_ranchhand(rh)?;
    Ok(())
}
