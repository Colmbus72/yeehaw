use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::types::*;

// ============================================================================
// Paths
// ============================================================================

pub fn yeehaw_dir() -> PathBuf {
    match yeehaw_home() {
        // An empty value is how a shell spells "unset" (`YEEHAW_HOME= yeehaw`),
        // and it is not harmless: `PathBuf::from("").join("projects")` is
        // `projects`, so the ranch would land in whatever directory the process
        // happened to start in.
        Some(value) if !value.is_empty() => {
            let dir = PathBuf::from(value);
            // A relative ranch moves with the process. Yeehaw spawns tmux panes
            // and ssh sessions from varying working directories, so the store
            // would fragment across the filesystem instead of failing. `~/ranch`
            // lands here too: nothing expands a tilde outside a shell, so it
            // would name a literal `~` directory.
            if !dir.is_absolute() {
                panic!(
                    "YEEHAW_HOME must be an absolute path, got {:?}. A relative \
                     path moves the ranch with the working directory, and `~` is \
                     not expanded outside a shell — write the path out in full.",
                    dir
                );
            }
            dir
        }
        _ => dirs::home_dir()
            .expect("Could not find home directory")
            .join(".yeehaw"),
    }
}

/// The `YEEHAW_HOME` value `yeehaw_dir()` honors.
///
/// `var_os`, not `var`: a path is arbitrary bytes on Unix, and `var` turns a
/// value that is not valid UTF-8 into `Err(NotUnicode)`, which reads here as
/// "unset" — silently relocating the ranch to `$HOME/.yeehaw`.
#[cfg(not(test))]
fn yeehaw_home() -> Option<OsString> {
    std::env::var_os("YEEHAW_HOME")
}

/// Under test, the thread-local ranch set by `crate::testing::with_temp_ranch`
/// is the *only* thing consulted. The process environment is deliberately not
/// read here.
///
/// A test with no ranch in scope is a bug: it would read and write the
/// developer's real `~/.yeehaw`. Fail loudly rather than lose their data.
///
/// There is no `YEEHAW_HOME` escape hatch on this path on purpose. Honoring it
/// disarmed the guard suite-wide for any developer who happened to have the
/// variable exported: every test that forgot `with_temp_ranch` then quietly
/// shared one directory instead of panicking, and since tests run in parallel
/// threads they trampled each other's state. The variable stays honored in the
/// `cfg(not(test))` build, where it is a real user-facing feature.
#[cfg(test)]
fn yeehaw_home() -> Option<OsString> {
    use crate::testing::RanchEnv;
    match crate::testing::ranch_env() {
        Some(RanchEnv::Set(value)) => Some(value),
        Some(RanchEnv::Unset) => None,
        None => panic!(
            "yeehaw_dir() called in a test without with_temp_ranch(); wrap \
             the test to avoid writing to the real ~/.yeehaw. The override \
             is thread-local, so a thread spawned inside a test does not \
             inherit it and must establish its own."
        ),
    }
}

pub fn config_file() -> PathBuf { yeehaw_dir().join("config.yaml") }
pub fn projects_dir() -> PathBuf { yeehaw_dir().join("projects") }
pub fn barns_dir() -> PathBuf { yeehaw_dir().join("barns") }
pub fn ranchhands_dir() -> PathBuf { yeehaw_dir().join("ranchhands") }
pub fn worms_dir() -> PathBuf { yeehaw_dir().join("worms") }
pub fn worm_runs_dir() -> PathBuf { yeehaw_dir().join("worm-runs") }
/// Drop-box for worm trigger files, watched live by `watcher.rs`.
///
/// # INVARIANT: never write into this directory with temp-and-rename
///
/// Everything else in the ranch is written through `store::write_atomic`. This
/// one directory is the documented exception, and the bare `fs::write` at each
/// of its five write sites is deliberate — not an oversight, and not something
/// to "fix":
///
/// - `watcher.rs` matches **any** path under this directory. There is no
///   extension filter, so a `.<name>.tmp-<pid>-<seq>` temp file is a trigger
///   as far as the watcher is concerned.
/// - `app.rs::handle_worm_trigger` reads the file the watcher named and then
///   **immediately deletes it**.
///
/// So a temp-and-rename write here is consumed before it is published: the
/// watcher fires on the temp file, the handler runs the worm off it and unlinks
/// it, and the `rename` that was supposed to publish the real file fails with
/// ENOENT. The trigger is lost *and* the writer reports an error.
///
/// Torn reads are not the trade they would be elsewhere. These payloads are a
/// few hundred bytes of JSON, and `handle_worm_trigger` already deletes and
/// ignores anything that fails to parse — so the worst case of a partial read
/// is one dropped trigger, which is exactly what temp-and-rename would cause
/// every time.
///
/// The five sites, all of which point back here:
/// `main.rs::run_worm_exec`, `app.rs::trigger_worm`, `mcp_server.rs`
/// (`run_worm_now` and the trail trigger) and `trails::polling::poll_and_trigger`.
pub fn worm_triggers_dir() -> PathBuf { yeehaw_dir().join("worm-triggers") }
pub fn sessions_dir() -> PathBuf { yeehaw_dir().join("sessions") }
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
    // A name is also a filename, so a blank one is a path, not a name: it writes
    // `barns/.yaml`, a dotfile that loads back as an entity called `""` — a blank
    // row in every listing that nothing can address to delete. Checked here
    // rather than at any constructor because `Barn { name: String::new(), .. }`
    // has always been legal and `Barn::default()` only made it shorter; this is
    // the one place every write passes through.
    if name.trim().is_empty() {
        anyhow::bail!("Invalid {} name: a name cannot be blank", entity_type);
    }
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
        signals_dir(),
        bin_dir(),
        trails_dir(),
        trail_runs_dir(),
        poll_state_dir(),
        skills_dir(),
        crate::tombstones::tombstones_dir(),
        // The three-way merge's ancestor snapshots. Created up front like every
        // other store directory, so the first sync is not also the first time
        // this path has ever been exercised.
        crate::ranch::base::base_dir(),
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
// Loading entity directories
// ============================================================================

/// A file that was supposed to be an entity and was not.
#[derive(Debug, Clone)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

/// What a directory of entities yielded.
#[derive(Debug, Clone)]
pub struct LoadResult<T> {
    pub items: Vec<T>,
    pub errors: Vec<LoadError>,
}

/// Loads every `*.yaml` in `dir`, reporting what failed instead of hiding it.
///
/// The old loaders did `if let Ok(x) = serde_yaml::from_str(..)`, so a file
/// that failed to parse simply vanished from the UI. During a migration that is
/// the difference between a visible bug and silently missing projects.
///
/// The `.yaml` filter is load-bearing for more than tidiness: `store` puts its
/// lock files in a `.locks` subdirectory (extension `None`) and writes through
/// `.<name>.tmp-<pid>-<seq>` temp files (extension `Some("tmp-123-4")`). Both
/// live inside these directories, and neither must surface — not as an item,
/// and not as a spurious parse error either, which would be worse than the
/// silence this function exists to remove.
fn load_dir<T: serde::de::DeserializeOwned>(dir: &Path) -> LoadResult<T> {
    let mut items = Vec::new();
    let mut errors = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "yaml") {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(content) => match serde_yaml::from_str::<T>(&content) {
                    Ok(item) => items.push(item),
                    Err(e) => errors.push(LoadError {
                        path: path.clone(),
                        message: e.to_string(),
                    }),
                },
                // Unreadable is reported, not skipped: a permissions problem on
                // one entity is exactly the kind of thing that should be loud.
                Err(e) => errors.push(LoadError {
                    path: path.clone(),
                    message: e.to_string(),
                }),
            }
        }
    }

    LoadResult { items, errors }
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
        let _ = crate::store::write_atomic(&path, &content);
        return config;
    }

    let content = fs::read_to_string(&path).unwrap_or_default();
    serde_yaml::from_str(&content).unwrap_or_default()
}

/// Persists `config` to `~/.yeehaw/config.yaml`.
///
/// The one supported way to write the config file. It is the most-read file in
/// the ranch and it is read-modify-written from several places while a TUI and
/// an mcp-server may both be live, so a bare `fs::write` here is a torn read
/// waiting to happen for every other reader.
pub fn save_config(config: &Config) -> Result<()> {
    ensure_config_dirs();
    let content = serde_yaml::to_string(config).context("Failed to serialize config")?;
    crate::store::write_atomic(&config_file(), &content).context("Failed to write config")?;
    Ok(())
}

// ============================================================================
// Projects
// ============================================================================

pub fn load_projects_checked() -> LoadResult<Project> {
    ensure_config_dirs();
    let mut result: LoadResult<Project> = load_dir(&projects_dir());
    result.items.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

pub fn load_projects() -> Vec<Project> {
    load_projects_checked().items
}

pub fn save_project(project: &mut Project) -> Result<()> {
    ensure_config_dirs();
    validate_name(&project.name, "project")?;
    // Stamped before serializing, so the id and timestamps that reach the file
    // are the same ones the caller keeps holding.
    project.stamp();
    let path = projects_dir().join(format!("{}.yaml", project.name));
    let content = serde_yaml::to_string(project).context("Failed to serialize project")?;
    crate::store::write_atomic(&path, &content).context("Failed to write project file")?;
    Ok(())
}

/// Saves a project that is not supposed to exist yet.
///
/// `save_project` writes over whatever is at `projects/<name>.yaml`, and a
/// freshly built `Project` carries empty `livestock`, `herds` and `wiki` and no
/// id — so a plain save on a name already in use destroys the entity's content
/// *and* mints a new uuid over its old one. To anything syncing on the uuid the
/// result is a different entity wearing the same name, with no trace of the one
/// that was there.
pub fn create_project(project: &mut Project) -> Result<()> {
    ensure_config_dirs();
    validate_name(&project.name, "project")?;
    // Held across the check and the write: without it two creators racing on
    // the same name both see an empty slot and the second still clobbers.
    let _guard = crate::store::lock_entity(&projects_dir(), &project.name)?;

    if projects_dir().join(format!("{}.yaml", project.name)).exists() {
        anyhow::bail!("A project named '{}' already exists", project.name);
    }
    save_project(project)
}

/// Saves `project` under its (possibly new) name and removes the file it used
/// to live in.
///
/// The filename is the human key and the uuid is the sync key, so a rename has
/// to move the file while keeping the uuid: that is exactly what tells a peer
/// this was a rename and not a delete plus a create. `save_project` alone
/// leaves the old file in place, and two files then carry one uuid.
///
/// **No tombstone.** The surviving uuid is the whole signal; a tombstone would
/// tell peers to delete the entity that was just renamed.
///
/// Write-then-delete, not delete-then-write. A crash between the two steps
/// leaves a duplicate, which a human can resolve, rather than nothing at all.
pub fn rename_project(old_name: &str, project: &mut Project) -> Result<()> {
    ensure_config_dirs();
    validate_name(old_name, "project")?;
    validate_name(&project.name, "project")?;

    // Load-bearing, not an optimization: this is what guarantees the two names
    // locked below are distinct. `fs2` locks are not re-entrant, so a rename of
    // a name to itself would reach for the same lock twice and park this thread
    // against itself with no timeout to recover from.
    if old_name == project.name {
        return save_project(project);
    }

    // Both names are held for the whole rename. The destination alone is not
    // enough: `add_livestock_to_project(old_name, …)` locks the *source*, so
    // with only the destination guarded the two run unserialized. That writer
    // can read `old.yaml`, watch this rename publish `new.yaml` and unlink
    // `old.yaml`, then write `old.yaml` back out of the copy it is holding —
    // resurrecting a second file carrying the same uuid, with its livestock
    // addition stranded on the orphan. The mirrored interleaving drops the
    // addition into a file this rename is about to delete.
    //
    // LOCK ORDERING RULE. This is the first path in the codebase to hold two
    // entity locks at once, so deadlock becomes possible here for the first
    // time: two crossing renames (`a`→`b` racing `b`→`a`) acquiring in argument
    // order would each hold the lock the other waits on, and
    // `fs2::lock_exclusive()` has no timeout to break the cycle. So the locks
    // are taken in one total order — sorted, smallest first. **Any future
    // multi-lock path must acquire in this same order.**
    //
    // The order is over `store::lock_key`, not over the name, because the
    // name→lock mapping is many-to-one; see that function for why sorting by
    // name is not a total order over the locks themselves.
    let mut names = [old_name, project.name.as_str()];
    names.sort_by_key(|name| crate::store::lock_key(name));

    let _first = crate::store::lock_entity(&projects_dir(), names[0])?;
    // Distinct names can still share one lock file (`"my api"` and `"my_api"`
    // both key to `my_api`). One lock already covers both of them, and asking
    // for it again would self-deadlock.
    let _second = if crate::store::lock_key(names[0]) == crate::store::lock_key(names[1]) {
        None
    } else {
        Some(crate::store::lock_entity(&projects_dir(), names[1])?)
    };

    // Refuse rather than clobber: the project sitting on the destination name
    // is a different entity with its own uuid and its own livestock.
    if projects_dir().join(format!("{}.yaml", project.name)).exists() {
        anyhow::bail!(
            "Cannot rename project '{}' to '{}': a project named '{}' already exists",
            old_name,
            project.name,
            project.name
        );
    }

    save_project(project)?;

    let old_path = projects_dir().join(format!("{}.yaml", old_name));
    if old_path.exists() {
        fs::remove_file(&old_path).with_context(|| {
            format!(
                "Renamed project to '{}' but failed to remove the old file {} — \
                 two files now carry the same id; delete the old one by hand",
                project.name,
                old_path.display()
            )
        })?;
    }
    Ok(())
}

/// Deletes a project and entombs it.
///
/// The tombstone is what stops the deletion from being undone. Absence and
/// deletion look identical on disk, so a peer that still has the project reads
/// "I have something you lack" and pushes it back. The tombstone says "I had
/// this and I deleted it", keyed by the uuid the peer is matching on.
///
/// **Read the id before the unlink.** After `remove_file` there is nothing left
/// to read it from, and the tombstone degrades to the `{kind}--{name}` fallback
/// key — which no peer is matching on, so the deletion never lands.
///
/// Contrast [`rename_project`], which deliberately records nothing.
pub fn delete_project(name: &str) -> Result<bool> {
    validate_name(name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }

    let id = entity_id::<Project>(&path);

    fs::remove_file(&path).context("Failed to delete project")?;
    crate::tombstones::record("project", name, id.as_deref())?;
    Ok(true)
}

/// The stable id recorded in the entity file at `path`, if it has one.
///
/// Everything is best-effort on purpose: this runs on the deletion path, where
/// the file is about to stop existing. An unreadable or unparseable file, or one
/// written before Task 6 gave entities ids, yields `None` and the caller falls
/// back to the deterministic `{kind}--{name}` key. Recording a deletion under a
/// weaker key is strictly better than refusing to record it.
fn entity_id<T: serde::de::DeserializeOwned + crate::types::Identified>(
    path: &Path,
) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_yaml::from_str::<T>(&c).ok())
        .and_then(|e| e.id().map(|s| s.to_string()))
}

pub fn add_livestock_to_project(project_name: &str, livestock: &Livestock) -> Result<()> {
    validate_name(project_name, "project")?;
    // Held across the read *and* the write. Atomic writes alone do not make a
    // read-modify-write safe: two writers can each read version N, apply a
    // different change, and the second write silently discards the first.
    let _guard = crate::store::lock_entity(&projects_dir(), project_name)?;

    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if project.livestock.iter().any(|l| l.name == livestock.name) {
        anyhow::bail!("Livestock '{}' already exists in project '{}'", livestock.name, project_name);
    }

    project.livestock.push(livestock.clone());
    // The parent is what changed: a livestock addition is an edit of the
    // project, so the project's `updated_at` has to move with it.
    project.stamp();

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    crate::store::write_atomic(&path, &new_content).context("Failed to write project file")?;
    Ok(())
}

pub fn update_livestock_in_project(project_name: &str, original_name: &str, updated: &Livestock) -> Result<()> {
    validate_name(project_name, "project")?;
    // Held across the read *and* the write. Atomic writes alone do not make a
    // read-modify-write safe: two writers can each read version N, apply a
    // different change, and the second write silently discards the first.
    let _guard = crate::store::lock_entity(&projects_dir(), project_name)?;

    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == original_name) {
        *ls = updated.clone();
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", original_name, project_name);
    }
    project.stamp();

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    crate::store::write_atomic(&path, &new_content).context("Failed to write project file")?;
    Ok(())
}

// ============================================================================
// Barns
// ============================================================================

pub const LOCAL_BARN_NAME: &str = "local";

pub fn local_barn() -> Barn {
    Barn {
        name: LOCAL_BARN_NAME.to_string(),
        // Synthetic and never persisted: `local` is injected into every
        // `load_barns()` and means "whichever machine is reading this", so it
        // has no stable identity to carry, no host to dial, and no ranch
        // membership of its own — whatever the real self-barn has is on the real
        // self-barn's record. Every one of those is `Default`'s answer.
        ..Default::default()
    }
}

pub fn is_local_barn(barn: &Barn) -> bool {
    barn.name == LOCAL_BARN_NAME
}

pub fn load_barns_checked() -> LoadResult<Barn> {
    ensure_config_dirs();
    let mut result: LoadResult<Barn> = load_dir(&barns_dir());
    // `local` is synthetic and belongs at the head of every listing. A file
    // that claims the name is dropped rather than shown: two barns spelled
    // `local` would be indistinguishable in the UI, and only one of them is
    // the machine the user is sitting at.
    result.items.retain(|barn| barn.name != LOCAL_BARN_NAME);
    result.items.insert(0, local_barn());
    result
}

pub fn load_barns() -> Vec<Barn> {
    load_barns_checked().items
}

pub fn save_barn(barn: &mut Barn) -> Result<()> {
    ensure_config_dirs();
    validate_name(&barn.name, "barn")?;
    barn.stamp();
    let path = barns_dir().join(format!("{}.yaml", barn.name));
    let content = serde_yaml::to_string(barn).context("Failed to serialize barn")?;
    crate::store::write_atomic(&path, &content).context("Failed to write barn file")?;
    Ok(())
}

/// Saves a barn that is not supposed to exist yet. See [`create_project`] for
/// why a plain save on a name already in use is destructive.
pub fn create_barn(barn: &mut Barn) -> Result<()> {
    ensure_config_dirs();
    validate_name(&barn.name, "barn")?;
    let _guard = crate::store::lock_entity(&barns_dir(), &barn.name)?;

    if barns_dir().join(format!("{}.yaml", barn.name)).exists() {
        anyhow::bail!("A barn named '{}' already exists", barn.name);
    }
    save_barn(barn)
}

pub fn delete_barn(name: &str) -> Result<bool> {
    // Above the tombstone logic, and it has to stay there. `local` is synthetic
    // — injected into every listing, never written to disk — so there is no file
    // to read an id from and no entity any peer could be asked to delete.
    if name == LOCAL_BARN_NAME {
        return Ok(false);
    }
    validate_name(name, "barn")?;
    let path = barns_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }

    let id = entity_id::<Barn>(&path);

    fs::remove_file(&path).context("Failed to delete barn")?;
    crate::tombstones::record("barn", name, id.as_deref())?;
    Ok(true)
}

pub fn update_critter_in_barn(barn_name: &str, original_name: &str, updated: &Critter) -> Result<()> {
    if barn_name == LOCAL_BARN_NAME {
        anyhow::bail!("Cannot edit critters on the local barn");
    }
    validate_name(barn_name, "barn")?;
    // Held across the read *and* the write. Atomic writes alone do not make a
    // read-modify-write safe: two writers can each read version N, apply a
    // different change, and the second write silently discards the first.
    let _guard = crate::store::lock_entity(&barns_dir(), barn_name)?;

    let path = barns_dir().join(format!("{}.yaml", barn_name));
    let content = fs::read_to_string(&path).context("Failed to read barn file")?;
    let mut barn: Barn = serde_yaml::from_str(&content).context("Failed to parse barn")?;

    if let Some(cr) = barn.critters.iter_mut().find(|c| c.name == original_name) {
        *cr = updated.clone();
    } else {
        anyhow::bail!("Critter '{}' not found in barn '{}'", original_name, barn_name);
    }
    barn.stamp();

    let new_content = serde_yaml::to_string(&barn).context("Failed to serialize barn")?;
    crate::store::write_atomic(&path, &new_content).context("Failed to write barn file")?;
    Ok(())
}

/// The barn name this machine answers to, or `None` if unmigrated.
///
/// Deliberately not `load_config()`. This is now asked from render paths and
/// once per row of a livestock list, and `load_config` runs
/// `ensure_config_dirs` — seventeen `create_dir_all`s — and writes a default
/// config file when none exists. Reading the one field keeps a redraw to a
/// single small read, and a missing or unparseable file answers `None`, which
/// is what `load_config().this_barn` answered too.
pub fn this_barn_name() -> Option<String> {
    let content = fs::read_to_string(config_file()).ok()?;
    serde_yaml::from_str::<Config>(&content).ok()?.this_barn
}

/// Resolves a possibly-aliased barn name to the name stored in livestock records.
///
/// `local` is the only alias, and it is a display name rather than stored data:
/// once this machine has been adopted, the livestock that used to say `barn:
/// None` say its real name, so a `local` lookup has to follow config to find
/// them.
pub fn resolve_barn_name(name: &str) -> String {
    if name == LOCAL_BARN_NAME {
        if let Some(this) = this_barn_name() {
            return this;
        }
    }
    name.to_string()
}

/// True when `name` denotes the machine Yeehaw is running on.
///
/// Two spellings mean that, and both have to be recognised: the `local` alias,
/// which is what an unadopted ranch and every peer's unmigrated file say, and
/// this machine's real barn name once [`crate::migrate::adopt_this_machine`]
/// has run. A caller that knows only one of them treats half of this machine's
/// own livestock as remote.
pub fn barn_name_is_this_machine(name: &str) -> bool {
    name == LOCAL_BARN_NAME || this_barn_name().as_deref() == Some(name)
}

/// True when `barn` is the record for the machine Yeehaw is running on.
///
/// The `Barn` form of [`barn_name_is_this_machine`], and the reason
/// [`is_local_barn`] is not enough on its own: adoption persists a *real* barn
/// for this machine, so from that point on the user can navigate into a barn
/// record that is the machine they are sitting at while `is_local_barn` says
/// `false` about it.
pub fn barn_is_this_machine(barn: &Barn) -> bool {
    barn_name_is_this_machine(&barn.name)
}

/// True when this livestock lives on the machine Yeehaw is running on.
///
/// Three spellings say so and all three are here: `None` ("whichever machine
/// reads this file"), the literal `local` the UI used to write, and — after
/// adoption — this machine's real barn name.
pub fn livestock_is_on_this_machine(livestock: &Livestock) -> bool {
    match livestock.barn.as_deref() {
        None => true,
        Some(name) => barn_name_is_this_machine(name),
    }
}

/// The barn a livestock lives on, or `None` when that is this machine.
///
/// `None` is deliberately the machine-relative answer rather than the barn's
/// real name. It is exactly what the livestock said *before* adoption, and it
/// is what every call site already branches on, so swapping
/// `livestock.barn.as_deref()` for this makes a site behave identically on
/// both sides of the migration rather than newly SSHing to itself.
pub fn resolve_livestock_barn(livestock: &Livestock) -> Option<&str> {
    match livestock.barn.as_deref() {
        Some(name) if !barn_name_is_this_machine(name) => Some(name),
        _ => None,
    }
}

/// Canonical spelling of a barn name, for comparing two of them.
///
/// Both names for this machine collapse to `local`, so a filter written with
/// one spelling still matches data tagged with the other. `local` is the
/// canonical form rather than the machine's real name because it is the
/// spelling that is stable across adoption: tmux windows tagged before the
/// migration keep matching afterwards without being retagged.
pub fn canonical_barn_name(name: &str) -> String {
    if barn_name_is_this_machine(name) {
        LOCAL_BARN_NAME.to_string()
    } else {
        name.to_string()
    }
}

/// How a livestock's barn is labelled in the UI.
///
/// Everything on this machine reads `local`, on both sides of adoption. The
/// column answers "is this here, or somewhere else?", and `local` is the
/// spelling the design keeps for "the barn I am running on". Adoption gives the
/// machine a name so that livestock records are portable between machines; it
/// is not news to the user sitting at it, and showing their own hostname where
/// they read `local` yesterday would be a change with no reader it helps.
pub fn barn_label(livestock: &Livestock) -> &str {
    resolve_livestock_barn(livestock).unwrap_or(LOCAL_BARN_NAME)
}

/// The value to store in `livestock.barn` for a barn the user picked by name.
///
/// The literal `local` is the one answer that must never reach disk. It is
/// machine-relative with a different spelling than `None`, so it carries the
/// identical sync defect while looking like a real pin — two livestock in the
/// real ranch acquired it that way. An unadopted machine stores `None`; an
/// adopted one stores its real name, which is the entire point of adoption.
pub fn stored_barn_name(picked: &str) -> Option<String> {
    let picked = picked.trim();
    if picked.is_empty() || picked.eq_ignore_ascii_case(LOCAL_BARN_NAME) {
        return this_barn_name();
    }
    Some(picked.to_string())
}

pub fn get_livestock_for_barn(barn_name: &str) -> Vec<(Project, Livestock)> {
    let target = resolve_barn_name(barn_name);
    // True for both spellings of this machine: the alias on an unmigrated
    // ranch, and the real name on an adopted one. A ranch is routinely
    // half-migrated — a project edited on another machine still carries
    // `barn: None` — so both spellings have to keep finding unpinned livestock.
    let is_this_machine = target == LOCAL_BARN_NAME
        || this_barn_name().as_deref() == Some(target.as_str());

    let projects = load_projects();
    let mut result = Vec::new();
    for project in &projects {
        for livestock in &project.livestock {
            let matches = match livestock.barn.as_deref() {
                // Unmigrated livestock belongs to whichever machine reads it.
                None => is_this_machine,
                // So does a livestock spelled `barn: local`. The UI's barn
                // picker writes that, and a peer can sync one in after this
                // machine was adopted, so it survives the migration — but
                // `local` is synthetic and matches no stored barn name, so
                // resolving it literally would leave the livestock visible
                // under no barn at all.
                Some(b) if b == LOCAL_BARN_NAME => is_this_machine,
                Some(b) => b == target,
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

pub fn load_worms_checked() -> LoadResult<Worm> {
    ensure_config_dirs();
    let mut result: LoadResult<Worm> = load_dir(&worms_dir());
    result.items.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

pub fn load_worms() -> Vec<Worm> {
    load_worms_checked().items
}

pub fn save_worm(worm: &mut Worm) -> Result<()> {
    ensure_config_dirs();
    validate_name(&worm.name, "worm")?;
    worm.stamp();
    let path = worms_dir().join(format!("{}.yaml", worm.name));
    let content = serde_yaml::to_string(worm).context("Failed to serialize worm")?;
    crate::store::write_atomic(&path, &content).context("Failed to write worm file")?;
    Ok(())
}

/// Saves a worm that is not supposed to exist yet. See [`create_project`] for
/// why a plain save on a name already in use is destructive.
pub fn create_worm(worm: &mut Worm) -> Result<()> {
    ensure_config_dirs();
    validate_name(&worm.name, "worm")?;
    let _guard = crate::store::lock_entity(&worms_dir(), &worm.name)?;

    if worms_dir().join(format!("{}.yaml", worm.name)).exists() {
        anyhow::bail!("A worm named '{}' already exists", worm.name);
    }
    save_worm(worm)
}

pub fn delete_worm(name: &str) -> Result<bool> {
    validate_name(name, "worm")?;
    let path = worms_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }

    let id = entity_id::<Worm>(&path);

    fs::remove_file(&path).context("Failed to delete worm")?;
    crate::tombstones::record("worm", name, id.as_deref())?;
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
    crate::store::write_atomic(&path, &content).context("Failed to write worm run file")?;
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

pub fn load_all_trails_checked() -> LoadResult<crate::trails::Trail> {
    ensure_config_dirs();
    let mut result: LoadResult<crate::trails::Trail> = load_dir(&trails_dir());
    result.items.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

pub fn load_all_trails() -> Vec<crate::trails::Trail> {
    load_all_trails_checked().items
}

pub fn load_trails_for_livestock(livestock: &Livestock) -> Vec<crate::trails::Trail> {
    livestock.trails.iter()
        .filter_map(|name| load_trail(name))
        .collect()
}

pub fn link_trail_to_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    validate_name(project_name, "project")?;
    // Held across the read *and* the write. Atomic writes alone do not make a
    // read-modify-write safe: two writers can each read version N, apply a
    // different change, and the second write silently discards the first.
    let _guard = crate::store::lock_entity(&projects_dir(), project_name)?;

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
    project.stamp();

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    crate::store::write_atomic(&path, &new_content).context("Failed to write project file")?;

    // The project read-modify-write is complete; everything below touches only
    // the trails and worms directories and the system crontab. Release the lock
    // here rather than at end of scope: `create_poll_worm` shells out through
    // `sync_crontab` to `crontab -l` and `crontab <file>`, which on macOS is a
    // Full-Disk-Access-gated binary and can be slow or hang. `lock_exclusive()`
    // has no timeout, so any other writer of this project would otherwise block
    // on external-process latency for no reason.
    drop(_guard);

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
    // Held across the read *and* the write. Atomic writes alone do not make a
    // read-modify-write safe: two writers can each read version N, apply a
    // different change, and the second write silently discards the first.
    let _guard = crate::store::lock_entity(&projects_dir(), project_name)?;

    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == livestock_name) {
        ls.trails.retain(|t| t != trail_name);
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", livestock_name, project_name);
    }
    project.stamp();

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    crate::store::write_atomic(&path, &new_content).context("Failed to write project file")?;

    // Released here, not at end of scope: the project write is done, and
    // `remove_poll_worm` only touches the worms directory before shelling out
    // through `sync_crontab` to `crontab -l` / `crontab <file>`. Holding an
    // untimed exclusive lock across an external process would stall every other
    // writer of this project for the duration.
    drop(_guard);

    // Remove poll worm if it exists
    let _ = remove_poll_worm(livestock_name, trail_name);

    Ok(())
}

pub fn save_trail(trail: &mut crate::trails::Trail) -> Result<()> {
    ensure_config_dirs();
    validate_name(&trail.name, "trail")?;
    trail.stamp();
    let path = trails_dir().join(format!("{}.yaml", trail.name));
    let content = serde_yaml::to_string(trail).context("Failed to serialize trail")?;
    crate::store::write_atomic(&path, &content).context("Failed to write trail file")?;
    Ok(())
}

/// Saves a trail that is not supposed to exist yet. See [`create_project`] for
/// why a plain save on a name already in use is destructive.
pub fn create_trail(trail: &mut crate::trails::Trail) -> Result<()> {
    ensure_config_dirs();
    validate_name(&trail.name, "trail")?;
    let _guard = crate::store::lock_entity(&trails_dir(), &trail.name)?;

    if trails_dir().join(format!("{}.yaml", trail.name)).exists() {
        anyhow::bail!("A trail named '{}' already exists", trail.name);
    }
    save_trail(trail)
}

pub fn delete_trail(name: &str) -> Result<bool> {
    validate_name(name, "trail")?;
    let path = trails_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }

    let id = entity_id::<crate::trails::Trail>(&path);

    fs::remove_file(&path).context("Failed to delete trail")?;
    crate::tombstones::record("trail", name, id.as_deref())?;
    Ok(true)
}

// ============================================================================
// Trail Runs
// ============================================================================

pub fn save_trail_run(run: &crate::trails::TrailRun, run_dir: &std::path::Path) -> Result<()> {
    // No `create_dir_all` here: `write_atomic` creates the target's parent
    // directory itself, and it has to — its temp file lives in that directory.
    let path = run_dir.join("run.json");
    let content = serde_json::to_string_pretty(run).context("Failed to serialize trail run")?;
    crate::store::write_atomic(&path, &content).context("Failed to write trail run")?;
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
    // `write_atomic` creates the parent directory itself.
    let path = poll_state_dir().join(format!("{}--{}.sha", livestock_name, branch));
    crate::store::write_atomic(&path, sha)?;
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

    let mut worm = crate::types::Worm {
        name: worm_name,
        command: format!("yeehaw trail poll {} {}", livestock_name, trail_name),
        schedule,
        worm_type: "shell".to_string(),
        enabled: true,
        project: project_name.map(|s| s.to_string()),
        working_dir: None,
        id: None,
        created_at: None,
        updated_at: None,
    };

    save_worm(&mut worm)?;
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

pub fn load_ranchhands_checked() -> LoadResult<RanchHand> {
    ensure_config_dirs();
    load_dir(&ranchhands_dir())
}

pub fn load_ranchhands() -> Vec<RanchHand> {
    load_ranchhands_checked().items
}

pub fn load_ranchhands_for_project(project_name: &str) -> Vec<RanchHand> {
    load_ranchhands()
        .into_iter()
        .filter(|rh| rh.project == project_name)
        .collect()
}

pub fn save_ranchhand(rh: &mut RanchHand) -> Result<()> {
    ensure_config_dirs();
    rh.stamp();
    let path = ranchhands_dir().join(format!("{}.yaml", rh.name));
    let content = serde_yaml::to_string(rh).context("Failed to serialize ranchhand")?;
    crate::store::write_atomic(&path, &content).context("Failed to write ranchhand")?;
    Ok(())
}

/// Saves a ranch hand that is not supposed to exist yet. See [`create_project`]
/// for why a plain save on a name already in use is destructive.
pub fn create_ranchhand(rh: &mut RanchHand) -> Result<()> {
    ensure_config_dirs();
    validate_name(&rh.name, "ranchhand")?;
    let _guard = crate::store::lock_entity(&ranchhands_dir(), &rh.name)?;

    if ranchhands_dir().join(format!("{}.yaml", rh.name)).exists() {
        anyhow::bail!("A ranch hand named '{}' already exists", rh.name);
    }
    save_ranchhand(rh)
}

pub fn delete_ranchhand(name: &str) -> Result<bool> {
    validate_name(name, "ranchhand")?;
    let path = ranchhands_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }

    let id = entity_id::<RanchHand>(&path);

    fs::remove_file(&path).context("Failed to delete ranchhand")?;
    crate::tombstones::record("ranchhand", name, id.as_deref())?;
    Ok(true)
}

pub fn update_ranchhand_last_sync(name: &str) -> Result<()> {
    // Same read-modify-write shape as `add_ranchhand_resource_mapping`, and it
    // rewrites the whole file for the sake of one timestamp — so without this
    // guard a sync stamp can discard a resource mapping that landed while it
    // was holding a stale copy.
    let _guard = crate::store::lock_entity(&ranchhands_dir(), name)?;

    let mut ranchhands = load_ranchhands();
    let rh = ranchhands.iter_mut().find(|r| r.name == name)
        .ok_or_else(|| anyhow::anyhow!("Ranch hand not found: {}", name))?;
    rh.last_sync = Some(chrono::Utc::now().to_rfc3339());
    save_ranchhand(rh)?;
    Ok(())
}

pub fn add_ranchhand_resource_mapping(name: &str, resource_id: &str, herd_name: &str) -> Result<()> {
    // Held across the read *and* the write: this rewrites the whole ranch hand
    // file from an in-memory copy, so an unlocked concurrent writer's change is
    // silently discarded rather than merged. `save_ranchhand` takes no lock of
    // its own, so calling it under this guard cannot deadlock.
    let _guard = crate::store::lock_entity(&ranchhands_dir(), name)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// The text of a caught panic, whichever way it was formatted.
    fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
        panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    }

    #[test]
    fn yeehaw_dir_respects_env_override() {
        testing::with_temp_ranch(|ranch| {
            assert_eq!(yeehaw_dir(), ranch.dir.path());
        });
    }

    #[test]
    fn yeehaw_dir_falls_back_to_home() {
        testing::without_yeehaw_home(|| {
            let expected = dirs::home_dir().unwrap().join(".yeehaw");
            assert_eq!(yeehaw_dir(), expected);
        });
    }

    #[test]
    fn temp_ranch_isolates_writes() {
        testing::with_temp_ranch(|ranch| {
            ensure_config_dirs();
            assert!(ranch.dir.path().join("projects").is_dir());
            assert!(ranch.dir.path().join("barns").is_dir());
        });
    }

    /// A relative `YEEHAW_HOME` does not fail — it *moves*. The ranch would be
    /// resolved against the process's working directory, and this TUI spawns
    /// tmux panes and ssh sessions from varying directories, so the store would
    /// silently fragment across the filesystem. `~/ranch` is the same bug wearing
    /// a friendlier face: nothing expands the tilde outside a shell, so it names
    /// a literal directory called `~` under wherever the process started.
    #[test]
    fn a_yeehaw_home_that_is_not_an_absolute_path_is_refused() {
        for value in ["relative/ranch", "~/ranch", "."] {
            let panic = std::panic::catch_unwind(|| {
                testing::with_ranch_env(value, yeehaw_dir)
            })
            .expect_err(&format!("{value:?} must be refused, not silently resolved"));

            assert!(
                panic_message(&panic).contains("absolute"),
                "the panic must say what is wrong with {value:?}"
            );
        }
    }

    /// The one value that is *not* an error. An empty `YEEHAW_HOME` is how a
    /// shell spells "unset" (`YEEHAW_HOME= yeehaw`), and it has to keep meaning
    /// that: `PathBuf::from("")` joins to a bare relative `projects`, so
    /// treating it as a path would scatter the ranch through `$CWD`.
    #[test]
    fn an_empty_yeehaw_home_falls_back_to_home_rather_than_the_working_directory() {
        let dir = testing::with_ranch_env("", yeehaw_dir);

        assert_eq!(dir, dirs::home_dir().unwrap().join(".yeehaw"));
    }

    /// A path is arbitrary bytes on Unix, so `YEEHAW_HOME` must survive as
    /// `OsString` end to end. Reading it as a `String` turns a name like this
    /// into `Err(NotUnicode)`, which reads as "unset" — the ranch would quietly
    /// relocate to `$HOME/.yeehaw` and the real one would look empty.
    #[cfg(unix)]
    #[test]
    fn a_yeehaw_home_that_is_not_utf8_is_still_honored() {
        use std::os::unix::ffi::OsStringExt;

        let value = std::ffi::OsString::from_vec(b"/tmp/ranch-\xff".to_vec());

        let dir = testing::with_ranch_env(value.clone(), yeehaw_dir);

        assert_eq!(dir, PathBuf::from(value));
    }

    /// `add_livestock_to_project` is a read-modify-write: it reads the whole
    /// project file, appends one livestock, and writes the whole file back.
    /// Two writers that read the same version each write back a file holding
    /// only their own addition, so the loser's livestock vanishes with no error
    /// reported anywhere.
    ///
    /// Atomic writes do not fix this. Every individual write is intact and
    /// well-formed; it is the *update* that is lost. Only holding the entity
    /// lock across the read and the write makes the sequence safe.
    ///
    /// The TUI and `yeehaw mcp-server` are separate processes writing these
    /// same files, so this is the real concurrency shape, not a synthetic one.
    #[test]
    fn concurrent_livestock_additions_do_not_lose_each_other() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 10;

        let ranch = testing::temp_ranch();
        // The ranch override is thread-local and a spawned thread does not
        // inherit it, so each worker has to be handed the path and point
        // itself at the same directory.
        let ranch_path = ranch.dir.path().as_os_str().to_os_string();

        let mut project = Project {
            name: "api".into(),
            path: "/tmp/api".into(),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        };
        save_project(&mut project).unwrap();

        let mut handles = Vec::new();
        for thread in 0..THREADS {
            let ranch_path = ranch_path.clone();
            handles.push(std::thread::spawn(move || {
                testing::with_ranch_env(ranch_path, || {
                    for i in 0..PER_THREAD {
                        let livestock = Livestock {
                            name: format!("web-{}-{}", thread, i),
                            path: "/tmp/web".into(),
                            barn: None,
                            repo: None,
                            branch: None,
                            log_path: None,
                            env_path: None,
                            source: None,
                            k8s_metadata: None,
                            trails: vec![],
                        };
                        add_livestock_to_project("api", &livestock)
                            .unwrap_or_else(|e| panic!("thread {thread} add {i} failed: {e:?}"));
                    }
                });
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let loaded = load_projects();
        assert_eq!(loaded.len(), 1);
        let names: std::collections::HashSet<&str> =
            loaded[0].livestock.iter().map(|l| l.name.as_str()).collect();

        let missing: Vec<String> = (0..THREADS)
            .flat_map(|t| (0..PER_THREAD).map(move |i| format!("web-{}-{}", t, i)))
            .filter(|expected| !names.contains(expected.as_str()))
            .collect();

        assert!(
            missing.is_empty(),
            "lost {} of {} livestock to a read-modify-write clobber: {:?}",
            missing.len(),
            THREADS * PER_THREAD,
            missing
        );

        // `missing` is derived from a set, so it stays empty even if an entry
        // was written twice. Count the entries too: a writer that serialized
        // correctly but appended a duplicate is just as wrong as one that lost
        // an update, and only this assertion sees it.
        assert_eq!(
            loaded[0].livestock.len(),
            THREADS * PER_THREAD,
            "expected {} livestock, got {}",
            THREADS * PER_THREAD,
            loaded[0].livestock.len()
        );
    }

    fn bare_project(name: &str) -> Project {
        Project {
            name: name.into(),
            path: format!("/tmp/{name}"),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock: vec![],
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn save_project_assigns_and_preserves_identity() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();

            let first_id = p.id.clone().unwrap();
            assert!(p.created_at.is_some());

            let loaded = load_projects();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].id.as_ref().unwrap(), &first_id);

            // Saving an already-identified entity must not mint a new id.
            let mut again = loaded[0].clone();
            again.summary = Some("changed".into());
            save_project(&mut again).unwrap();
            assert_eq!(again.id.as_ref().unwrap(), &first_id);
            assert_eq!(load_projects()[0].id.as_ref().unwrap(), &first_id);
        });
    }

    /// The same contract for the other four savers. `save_project` having it
    /// proves nothing about `save_barn`; each one stamps its own argument.
    #[test]
    fn every_saver_assigns_an_id_once_and_keeps_it_across_a_reload() {
        testing::with_temp_ranch(|_| {
            let mut barn = Barn {
                name: "pi".into(),
                host: Some("10.0.0.2".into()),
                user: Some("forge".into()),
                port: Some(22),
                identity_file: None,
                critters: vec![],
                ..Default::default()
            };
            save_barn(&mut barn).unwrap();
            let barn_id = barn.id.clone().expect("save_barn must stamp its argument");
            let mut reloaded = load_barns().into_iter().find(|b| b.name == "pi").unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&barn_id));
            save_barn(&mut reloaded).unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&barn_id), "barn id must survive a re-save");

            let mut worm = Worm {
                name: "nightly".into(),
                command: "echo hi".into(),
                schedule: "* * * * *".into(),
                worm_type: "shell".into(),
                enabled: true,
                project: None,
                working_dir: None,
                id: None,
                created_at: None,
                updated_at: None,
            };
            save_worm(&mut worm).unwrap();
            let worm_id = worm.id.clone().expect("save_worm must stamp its argument");
            let mut reloaded = load_worms().into_iter().find(|w| w.name == "nightly").unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&worm_id));
            save_worm(&mut reloaded).unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&worm_id), "worm id must survive a re-save");

            let mut trail = crate::trails::Trail {
                name: "deploy".into(),
                on: None,
                env: None,
                jobs: Default::default(),
                id: None,
                created_at: None,
                updated_at: None,
            };
            save_trail(&mut trail).unwrap();
            let trail_id = trail.id.clone().expect("save_trail must stamp its argument");
            let mut reloaded = load_trail("deploy").unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&trail_id));
            save_trail(&mut reloaded).unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&trail_id), "trail id must survive a re-save");

            let mut rh = RanchHand {
                name: "cluster".into(),
                project: "api".into(),
                rh_type: "k8s".into(),
                config: serde_yaml::Value::Null,
                sync_settings: RanchHandSyncSettings { auto_sync: false, interval_minutes: None },
                herd: "infra".into(),
                resource_mappings: vec![],
                last_sync: None,
                id: None,
                created_at: None,
                updated_at: None,
            };
            save_ranchhand(&mut rh).unwrap();
            let rh_id = rh.id.clone().expect("save_ranchhand must stamp its argument");
            let mut reloaded = load_ranchhands().into_iter().find(|r| r.name == "cluster").unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&rh_id));
            save_ranchhand(&mut reloaded).unwrap();
            assert_eq!(reloaded.id.as_ref(), Some(&rh_id), "ranchhand id must survive a re-save");
        });
    }

    /// A read-modify-write changes the *parent*. Adding a livestock is an edit
    /// of the project, so the project's `updated_at` has to move — otherwise a
    /// sync comparing timestamps would decide the project was untouched and
    /// keep the other side's older copy, silently discarding the addition.
    #[test]
    fn read_modify_write_advances_the_parents_updated_at() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();
            let id = p.id.clone().unwrap();
            let created = p.created_at.clone().unwrap();
            let after_save = p.updated_at.clone().unwrap();

            std::thread::sleep(std::time::Duration::from_millis(5));
            add_livestock_to_project(
                "api",
                &Livestock {
                    name: "web".into(),
                    path: "/tmp/web".into(),
                    barn: None,
                    repo: None,
                    branch: None,
                    log_path: None,
                    env_path: None,
                    source: None,
                    k8s_metadata: None,
                    trails: vec![],
                },
            )
            .unwrap();

            let loaded = load_projects().remove(0);
            assert_eq!(loaded.livestock.len(), 1);
            assert_eq!(loaded.id.as_ref(), Some(&id), "the rmw must not re-mint the id");
            assert_eq!(loaded.created_at.as_ref(), Some(&created), "created_at must not move");
            assert!(
                loaded.updated_at.as_ref().unwrap() > &after_save,
                "adding a livestock is a change to the project: updated_at must advance \
                 (was {after_save}, now {:?})",
                loaded.updated_at
            );

            // Same for the other project-level read-modify-writes.
            let before = loaded.updated_at.clone().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
            link_trail_to_livestock("api", "web", "deploy").unwrap();
            let after_link = load_projects().remove(0).updated_at.unwrap();
            assert!(after_link > before, "link_trail_to_livestock must advance updated_at");

            std::thread::sleep(std::time::Duration::from_millis(5));
            unlink_trail_from_livestock("api", "web", "deploy").unwrap();
            let after_unlink = load_projects().remove(0).updated_at.unwrap();
            assert!(after_unlink > after_link, "unlink_trail_from_livestock must advance updated_at");

            std::thread::sleep(std::time::Duration::from_millis(5));
            update_livestock_in_project(
                "api",
                "web",
                &Livestock {
                    name: "web".into(),
                    path: "/tmp/web2".into(),
                    barn: None,
                    repo: None,
                    branch: None,
                    log_path: None,
                    env_path: None,
                    source: None,
                    k8s_metadata: None,
                    trails: vec![],
                },
            )
            .unwrap();
            let after_update = load_projects().remove(0).updated_at.unwrap();
            assert!(after_update > after_unlink, "update_livestock_in_project must advance updated_at");
        });
    }

    /// `update_critter_in_barn` is the barn-side read-modify-write.
    #[test]
    fn updating_a_critter_advances_the_barns_updated_at() {
        testing::with_temp_ranch(|_| {
            let critter = |path: Option<&str>| Critter {
                name: "mysql".into(),
                service: "mysql.service".into(),
                service_path: path.map(|s| s.to_string()),
                config_path: None,
                log_path: None,
                use_journald: Some(true),
                source: None,
                endpoint: None,
                port: None,
                k8s_metadata: None,
                tf_metadata: None,
            };
            let mut barn = Barn {
                name: "pi".into(),
                host: Some("10.0.0.2".into()),
                user: Some("forge".into()),
                port: Some(22),
                identity_file: None,
                critters: vec![critter(None)],
                ..Default::default()
            };
            save_barn(&mut barn).unwrap();
            let id = barn.id.clone().unwrap();
            let before = barn.updated_at.clone().unwrap();

            std::thread::sleep(std::time::Duration::from_millis(5));
            update_critter_in_barn("pi", "mysql", &critter(Some("/etc/mysql"))).unwrap();

            let loaded = load_barns().into_iter().find(|b| b.name == "pi").unwrap();
            assert_eq!(loaded.critters[0].service_path.as_deref(), Some("/etc/mysql"));
            assert_eq!(loaded.id.as_ref(), Some(&id), "the rmw must not re-mint the id");
            assert!(
                loaded.updated_at.as_ref().unwrap() > &before,
                "editing a critter is a change to the barn: updated_at must advance"
            );
        });
    }

    /// The 17 projects, 9 barns and 11 trails already on this machine carry
    /// none of the new fields. Loading the ranch must not touch a single byte
    /// of them — an id appears only on an entity that is actually saved.
    #[test]
    fn loading_a_ranch_of_pre_identity_files_rewrites_nothing() {
        testing::with_temp_ranch(|ranch| {
            ensure_config_dirs();

            let project_yaml = r#"name: Agent Desk
path: /Users/cam/Sites/MPP/desk/
summary: Autonomous financial newsroom.
color: '#000000'
gradientSpread: null
gradientInverted: null
livestock:
- name: production
  path: /home/forge/desk/
  barn: guided
  repo: null
  branch: main
  log_path: null
  env_path: null
  source: null
  k8s_metadata: null
  trails: []
herds: []
wiki: []
"#;
            let barn_yaml = r#"name: ascend
host: 172.233.129.224
user: forge
port: 22
identity_file: ~/.ssh/id_big_ups
critters:
  - name: mysql
    service: mysql.service
    use_journald: true
"#;
            let trail_yaml = r#"name: deploy
on:
  push: null
env: null
jobs:
  deploy:
    runs-on: self-hosted
    env: null
    steps:
    - name: Pull
      run: git pull
      env: null
      timeout-minutes: null
"#;

            let files = [
                (ranch.dir.path().join("projects").join("Agent Desk.yaml"), project_yaml),
                (ranch.dir.path().join("barns").join("ascend.yaml"), barn_yaml),
                (ranch.dir.path().join("trails").join("deploy.yaml"), trail_yaml),
            ];
            for (path, content) in &files {
                fs::write(path, content).unwrap();
            }

            assert_eq!(load_projects().len(), 1);
            assert_eq!(load_barns().len(), 2, "the synthetic local barn plus ascend");
            assert_eq!(load_all_trails().len(), 1);
            assert!(load_projects()[0].id.is_none(), "loading must not mint an id");

            for (path, content) in &files {
                assert_eq!(
                    &fs::read_to_string(path).unwrap(),
                    content,
                    "{} was rewritten by a read",
                    path.display()
                );
            }
        });
    }

    /// A test that forgets `with_temp_ranch` must fail loudly instead of
    /// quietly reading and writing the developer's real `~/.yeehaw`.
    ///
    /// Unconditional on purpose. This assertion used to be skipped whenever the
    /// developer had `YEEHAW_HOME` set, which is exactly the case where the
    /// guard was silently disarmed suite-wide — the test reported `ok` in 0.00s
    /// having checked nothing.
    #[test]
    fn yeehaw_dir_without_a_ranch_in_scope_refuses_to_use_the_real_one() {
        let panic = std::panic::catch_unwind(|| testing::without_ranch_override(yeehaw_dir))
            .expect_err("yeehaw_dir() with no ranch in scope must panic");

        assert!(
            panic_message(&panic).contains("with_temp_ranch"),
            "the panic must name the fix, got: {}",
            panic_message(&panic)
        );
    }

    // === renaming =========================================================

    fn project_files() -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(projects_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".yaml"))
            .collect();
        names.sort();
        names
    }

    /// `save_project` writes to `projects/<name>.yaml`, so saving a renamed
    /// project leaves the old file sitting beside the new one carrying the very
    /// same uuid. Two files, one identity: a uuid-keyed merge cannot tell which
    /// is the entity and which is the ghost, and whichever it picks the other
    /// one comes back.
    #[test]
    fn renaming_a_project_leaves_exactly_one_file_behind() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();
            let id = p.id.clone().unwrap();

            let mut renamed = p.clone();
            renamed.name = "gateway".into();
            rename_project("api", &mut renamed).unwrap();

            assert_eq!(
                project_files(),
                vec!["gateway.yaml".to_string()],
                "the old file must be gone"
            );

            let loaded = load_projects();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].name, "gateway");
            assert_eq!(
                loaded[0].id.as_ref(),
                Some(&id),
                "a rename keeps the uuid — that is what makes it a rename"
            );
        });
    }

    /// The rename must not be able to eat an unrelated project that happens to
    /// hold the destination name. Refusing leaves both entities intact; the
    /// alternative silently destroys one of them.
    #[test]
    fn renaming_onto_an_existing_project_is_refused_and_changes_nothing() {
        testing::with_temp_ranch(|_| {
            let mut api = bare_project("api");
            save_project(&mut api).unwrap();
            let mut gateway = bare_project("gateway");
            gateway.summary = Some("the real gateway".into());
            save_project(&mut gateway).unwrap();
            let gateway_id = gateway.id.clone().unwrap();

            let mut renamed = api.clone();
            renamed.name = "gateway".into();
            let err = rename_project("api", &mut renamed)
                .expect_err("renaming onto an existing project must be refused");
            assert!(
                err.to_string().contains("gateway"),
                "the error must name the collision, got: {err}"
            );

            assert_eq!(
                project_files(),
                vec!["api.yaml".to_string(), "gateway.yaml".to_string()],
                "both projects must survive a refused rename"
            );
            let survivor = load_projects().into_iter().find(|p| p.name == "gateway").unwrap();
            assert_eq!(survivor.id.as_ref(), Some(&gateway_id), "the victim keeps its identity");
            assert_eq!(survivor.summary.as_deref(), Some("the real gateway"));
        });
    }

    /// The no-op case has to stay a plain save: an edit that does not touch the
    /// name must not delete the file it just wrote.
    #[test]
    fn renaming_a_project_to_its_own_name_is_an_ordinary_save() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();
            let id = p.id.clone().unwrap();

            p.summary = Some("edited".into());
            rename_project("api", &mut p).unwrap();

            assert_eq!(project_files(), vec!["api.yaml".to_string()]);
            let loaded = load_projects();
            assert_eq!(loaded[0].summary.as_deref(), Some("edited"));
            assert_eq!(loaded[0].id.as_ref(), Some(&id));
        });
    }

    // === renaming under concurrency =======================================

    fn bare_livestock(name: &str) -> Livestock {
        Livestock {
            name: name.into(),
            path: "/tmp/web".into(),
            barn: None,
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    /// The uuid on every project file currently in the ranch, in file order.
    fn project_file_ids() -> Vec<String> {
        fs::read_dir(projects_dir())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
            .filter_map(|p| fs::read_to_string(&p).ok())
            .filter_map(|c| serde_yaml::from_str::<Project>(&c).ok())
            .filter_map(|p| p.id)
            .collect()
    }

    /// A rename publishes the new file and unlinks the old one. Guarding only
    /// the *destination* name leaves the source unguarded, and
    /// `add_livestock_to_project(old_name, …)` locks exactly the source — so
    /// the two run unserialized against each other.
    ///
    /// The losing interleaving resurrects the file the rename just deleted: the
    /// writer reads `api.yaml`, this rename writes `gateway.yaml` and unlinks
    /// `api.yaml`, and the writer then writes `api.yaml` back out of the copy
    /// it is still holding. The ranch is left with two files carrying one uuid
    /// — the exact state a rename exists to avoid — and the livestock that was
    /// just added lives only on the orphan.
    ///
    /// The project is loaded up with livestock on purpose: the wider the
    /// writer's parse/serialize window, the more reliably the unlink lands
    /// inside it.
    ///
    /// Not asserted: that the concurrent addition survives. `rename_project`
    /// writes the caller's in-memory snapshot, so an addition that lands before
    /// it is overwritten by a stale copy either way. That is a separate
    /// last-writer-wins question, not this lock race.
    #[test]
    fn a_rename_racing_a_livestock_addition_cannot_resurrect_the_old_file() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        const ATTEMPTS: usize = 40;

        for attempt in 0..ATTEMPTS {
            let ranch = testing::temp_ranch();
            // Thread-local and not inherited: the worker has to point itself at
            // the same ranch.
            let ranch_path = ranch.dir.path().as_os_str().to_os_string();

            let mut project = bare_project("api");
            project.livestock = (0..150).map(|i| bare_livestock(&format!("seed-{i}"))).collect();
            save_project(&mut project).unwrap();

            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicUsize::new(0));

            let writer = {
                let ranch_path = ranch_path.clone();
                let stop = Arc::clone(&stop);
                let done = Arc::clone(&done);
                std::thread::spawn(move || {
                    testing::with_ranch_env(ranch_path, || {
                        let mut i = 0usize;
                        while !stop.load(Ordering::Relaxed) {
                            // Errors are expected once the rename has taken the
                            // old file away; the corruption, not the error, is
                            // what this test is looking for.
                            let _ = add_livestock_to_project(
                                "api",
                                &bare_livestock(&format!("racer-{i}")),
                            );
                            i += 1;
                            done.store(i, Ordering::Relaxed);
                        }
                    });
                })
            };

            // Let the writer get into its loop, so the rename lands mid-flight
            // rather than before the first read.
            while done.load(Ordering::Relaxed) < 2 {
                std::thread::yield_now();
            }

            let mut renamed = project.clone();
            renamed.name = "gateway".into();
            rename_project("api", &mut renamed).unwrap();

            stop.store(true, Ordering::Relaxed);
            writer.join().unwrap();

            let files = project_files();
            assert_eq!(
                files,
                vec!["gateway.yaml".to_string()],
                "attempt {attempt}: a concurrent livestock addition resurrected the \
                 renamed-away file — {files:?}"
            );

            let ids = project_file_ids();
            let unique: std::collections::HashSet<&String> = ids.iter().collect();
            assert_eq!(
                ids.len(),
                unique.len(),
                "attempt {attempt}: two project files carry the same uuid: {ids:?}"
            );
        }
    }

    /// Locking two names is what makes deadlock possible here for the first
    /// time, and `fs2::lock_exclusive()` has no timeout — a cycle hangs the
    /// process forever, with no error to fail a test on. `a`→`b` racing `b`→`a`
    /// is the cycle: acquiring in argument order has each thread holding the
    /// lock the other is waiting for.
    ///
    /// Run on threads with a bounded wait on purpose: `cargo test` has no
    /// per-test timeout, so a regression asserted any other way would hang CI
    /// instead of failing it.
    #[test]
    fn crossing_renames_do_not_deadlock() {
        use std::sync::mpsc::RecvTimeoutError;
        use std::sync::{Arc, Barrier};
        use std::time::{Duration, Instant};

        const ROUNDS: usize = 50;
        const BUDGET: Duration = Duration::from_secs(20);

        for round in 0..ROUNDS {
            let ranch = testing::temp_ranch();
            let ranch_path = ranch.dir.path().as_os_str().to_os_string();

            let mut alpha = bare_project("alpha");
            save_project(&mut alpha).unwrap();
            let mut beta = bare_project("beta");
            save_project(&mut beta).unwrap();

            let barrier = Arc::new(Barrier::new(2));
            let (tx, rx) = std::sync::mpsc::channel();

            let mut handles = Vec::new();
            for (old, mut moving) in [("alpha", beta.clone()), ("beta", alpha.clone())] {
                let ranch_path = ranch_path.clone();
                let barrier = Arc::clone(&barrier);
                let tx = tx.clone();
                handles.push(std::thread::spawn(move || {
                    testing::with_ranch_env(ranch_path, || {
                        barrier.wait();
                        // Both are refused — each destination is occupied — but
                        // the refusal happens *under* the locks, which is the
                        // part that can deadlock.
                        let refused = rename_project(old, &mut moving).is_err();
                        let _ = tx.send(refused);
                    });
                }));
            }
            // The test's own sender would otherwise keep the channel alive and
            // turn a panicking worker into a timeout.
            drop(tx);

            let deadline = Instant::now() + BUDGET;
            for _ in 0..2 {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(refused) => assert!(refused, "round {round}: a crossing rename was allowed"),
                    Err(RecvTimeoutError::Timeout) => panic!(
                        "round {round}: crossing renames deadlocked — 'alpha'→'beta' and \
                         'beta'→'alpha' did not both finish within {BUDGET:?}. Entity locks \
                         must be acquired in one total order, not in argument order."
                    ),
                    Err(RecvTimeoutError::Disconnected) => {
                        panic!("round {round}: a rename thread panicked")
                    }
                }
            }
            // Safe only here: both threads have already reported, so neither
            // can be parked on a lock.
            for handle in handles {
                handle.join().unwrap();
            }

            assert_eq!(
                project_files(),
                vec!["alpha.yaml".to_string(), "beta.yaml".to_string()],
                "round {round}: both projects must survive two refused renames"
            );
        }
    }

    // === create must not clobber ==========================================

    /// `create_project` builds a project with empty `livestock`, `herds` and
    /// `wiki`. Letting it land on an existing file destroys all three *and*
    /// mints a new uuid over the old one — the entity is gone and its
    /// replacement is, to anything syncing, a different entity wearing its
    /// name. There is no trace either way.
    #[test]
    fn create_project_refuses_to_overwrite_an_existing_project() {
        testing::with_temp_ranch(|_| {
            let mut existing = bare_project("api");
            existing.livestock.push(Livestock {
                name: "web".into(),
                path: "/tmp/web".into(),
                barn: None,
                repo: None,
                branch: None,
                log_path: None,
                env_path: None,
                source: None,
                k8s_metadata: None,
                trails: vec![],
            });
            save_project(&mut existing).unwrap();
            let id = existing.id.clone().unwrap();

            let mut intruder = bare_project("api");
            let err = create_project(&mut intruder)
                .expect_err("creating over an existing project must be refused");
            assert!(
                err.to_string().contains("api"),
                "the error must name the entity, got: {err}"
            );

            let loaded = load_projects();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].id.as_ref(), Some(&id), "the uuid must not be re-minted");
            assert_eq!(loaded[0].livestock.len(), 1, "the content must survive");
        });
    }

    /// The same contract for the other four creators. Each writes its own file,
    /// so `create_project` having the guard proves nothing about them.
    #[test]
    fn every_creator_refuses_to_overwrite_an_existing_entity() {
        testing::with_temp_ranch(|_| {
            let mut barn = bare_barn("pi");
            save_barn(&mut barn).unwrap();
            let barn_id = barn.id.clone().unwrap();
            assert!(create_barn(&mut bare_barn("pi")).is_err(), "create_barn must refuse");
            assert_eq!(
                load_barns().into_iter().find(|b| b.name == "pi").unwrap().id.as_ref(),
                Some(&barn_id),
                "barn uuid must not be re-minted"
            );

            let mut worm = bare_worm("nightly");
            save_worm(&mut worm).unwrap();
            let worm_id = worm.id.clone().unwrap();
            assert!(create_worm(&mut bare_worm("nightly")).is_err(), "create_worm must refuse");
            assert_eq!(
                load_worms().into_iter().find(|w| w.name == "nightly").unwrap().id.as_ref(),
                Some(&worm_id),
                "worm uuid must not be re-minted"
            );

            let mut trail = bare_trail("deploy");
            save_trail(&mut trail).unwrap();
            let trail_id = trail.id.clone().unwrap();
            assert!(create_trail(&mut bare_trail("deploy")).is_err(), "create_trail must refuse");
            assert_eq!(
                load_trail("deploy").unwrap().id.as_ref(),
                Some(&trail_id),
                "trail uuid must not be re-minted"
            );

            let mut rh = bare_ranchhand("cluster");
            save_ranchhand(&mut rh).unwrap();
            let rh_id = rh.id.clone().unwrap();
            assert!(
                create_ranchhand(&mut bare_ranchhand("cluster")).is_err(),
                "create_ranchhand must refuse"
            );
            assert_eq!(
                load_ranchhands().into_iter().find(|r| r.name == "cluster").unwrap().id.as_ref(),
                Some(&rh_id),
                "ranchhand uuid must not be re-minted"
            );
        });
    }

    /// The guard must not make creation itself impossible.
    #[test]
    fn create_still_creates_when_nothing_is_in_the_way() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            create_project(&mut p).unwrap();
            assert!(p.id.is_some(), "a created project is stamped like any other save");
            assert_eq!(load_projects().len(), 1);

            let mut barn = bare_barn("pi");
            create_barn(&mut barn).unwrap();
            let mut worm = bare_worm("nightly");
            create_worm(&mut worm).unwrap();
            let mut trail = bare_trail("deploy");
            create_trail(&mut trail).unwrap();
            let mut rh = bare_ranchhand("cluster");
            create_ranchhand(&mut rh).unwrap();

            assert!(load_barns().iter().any(|b| b.name == "pi"));
            assert!(load_worms().iter().any(|w| w.name == "nightly"));
            assert!(load_trail("deploy").is_some());
            assert!(load_ranchhands().iter().any(|r| r.name == "cluster"));
        });
    }

    // === ranch hand read-modify-write =====================================

    /// `add_ranchhand_resource_mapping` loads every ranch hand, mutates one in
    /// memory, and writes the whole file back. Two writers that read the same
    /// version each write back a file holding only their own mapping, and the
    /// loser's assignment vanishes with no error anywhere. The MCP server and
    /// the TUI are separate processes calling this, so the race is real.
    #[test]
    fn concurrent_resource_mappings_do_not_lose_each_other() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 10;

        let ranch = testing::temp_ranch();
        // The override is thread-local; a spawned thread has to point itself
        // at the same directory.
        let ranch_path = ranch.dir.path().as_os_str().to_os_string();

        let mut rh = bare_ranchhand("cluster");
        save_ranchhand(&mut rh).unwrap();

        let mut handles = Vec::new();
        for thread in 0..THREADS {
            let ranch_path = ranch_path.clone();
            handles.push(std::thread::spawn(move || {
                testing::with_ranch_env(ranch_path, || {
                    for i in 0..PER_THREAD {
                        add_ranchhand_resource_mapping(
                            "cluster",
                            &format!("res-{}-{}", thread, i),
                            "infra",
                        )
                        .unwrap_or_else(|e| panic!("thread {thread} mapping {i} failed: {e:?}"));
                    }
                });
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let loaded = load_ranchhands().into_iter().find(|r| r.name == "cluster").unwrap();
        let ids: std::collections::HashSet<&str> = loaded
            .resource_mappings
            .iter()
            .map(|m| m.resource_id.as_str())
            .collect();
        let missing: Vec<String> = (0..THREADS)
            .flat_map(|t| (0..PER_THREAD).map(move |i| format!("res-{}-{}", t, i)))
            .filter(|expected| !ids.contains(expected.as_str()))
            .collect();

        assert!(
            missing.is_empty(),
            "lost {} of {} mappings, e.g. {:?}",
            missing.len(),
            THREADS * PER_THREAD,
            &missing[..missing.len().min(5)]
        );
    }

    /// `update_ranchhand_last_sync` writes the *whole* ranch hand back too, so
    /// it can discard a mapping that landed while it was holding a stale copy.
    /// Locking one of the two read-modify-writes is not enough; the other one
    /// still clobbers.
    #[test]
    fn a_last_sync_update_does_not_discard_a_concurrent_resource_mapping() {
        const MAPPERS: usize = 4;
        const SYNCERS: usize = 4;
        const PER_THREAD: usize = 15;

        let ranch = testing::temp_ranch();
        let ranch_path = ranch.dir.path().as_os_str().to_os_string();

        let mut rh = bare_ranchhand("cluster");
        save_ranchhand(&mut rh).unwrap();

        let mut handles = Vec::new();
        for thread in 0..MAPPERS {
            let ranch_path = ranch_path.clone();
            handles.push(std::thread::spawn(move || {
                testing::with_ranch_env(ranch_path, || {
                    for i in 0..PER_THREAD {
                        add_ranchhand_resource_mapping(
                            "cluster",
                            &format!("res-{}-{}", thread, i),
                            "infra",
                        )
                        .unwrap_or_else(|e| panic!("mapper {thread} failed at {i}: {e:?}"));
                    }
                });
            }));
        }
        for _ in 0..SYNCERS {
            let ranch_path = ranch_path.clone();
            handles.push(std::thread::spawn(move || {
                testing::with_ranch_env(ranch_path, || {
                    for _ in 0..PER_THREAD {
                        update_ranchhand_last_sync("cluster").expect("last-sync update failed");
                    }
                });
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let loaded = load_ranchhands().into_iter().find(|r| r.name == "cluster").unwrap();
        let ids: std::collections::HashSet<&str> = loaded
            .resource_mappings
            .iter()
            .map(|m| m.resource_id.as_str())
            .collect();
        let missing: Vec<String> = (0..MAPPERS)
            .flat_map(|t| (0..PER_THREAD).map(move |i| format!("res-{}-{}", t, i)))
            .filter(|expected| !ids.contains(expected.as_str()))
            .collect();

        assert!(
            missing.is_empty(),
            "a last-sync write discarded {} of {} mappings, e.g. {:?}",
            missing.len(),
            MAPPERS * PER_THREAD,
            &missing[..missing.len().min(5)]
        );
        assert!(loaded.last_sync.is_some(), "last_sync must actually have been written");
    }

    /// Slice D gave `Barn` a `Default`, which makes
    /// `Barn { host: .., ..Default::default() }` a one-line way to ask for a barn
    /// with no name. The name becomes the filename, so an unnamed barn lands at
    /// `barns/.yaml` — a dotfile that then loads back as a barn called `""`,
    /// shows in every listing as a blank row, and cannot be addressed to delete.
    ///
    /// The guard belongs here rather than in a constructor: `Barn { name:
    /// String::new(), .. }` was already legal before `Default` existed, so no
    /// constructor could ever have been the defense, and this is where every
    /// other malformed name — `/`, `..`, `\0` — is already refused.
    #[test]
    fn an_entity_with_a_blank_name_is_refused_rather_than_written_to_a_dotfile() {
        testing::with_temp_ranch(|ranch| {
            let mut unnamed = Barn { host: Some("pi.local".into()), ..Default::default() };
            let err = save_barn(&mut unnamed)
                .expect_err("a barn with no name has no file it could legitimately be");
            assert!(
                err.to_string().to_lowercase().contains("name"),
                "the refusal must say what was wrong: {}",
                err
            );
            assert!(
                !barns_dir().join(".yaml").exists(),
                "a nameless barn must not land as a dotfile in barns/"
            );

            // Whitespace is not a name either: it would make a file called
            // `   .yaml` and a listing row that looks like a rendering bug.
            let mut blank = Barn { name: "   ".into(), ..Default::default() };
            assert!(save_barn(&mut blank).is_err(), "whitespace is not a name");

            // And the guard must not have become a guard against everything.
            let mut real = Barn { name: "pi".into(), ..Default::default() };
            save_barn(&mut real).expect("an ordinary barn still saves");
            assert!(ranch.dir.path().join("barns").join("pi.yaml").exists());
        })
    }

    fn bare_barn(name: &str) -> Barn {
        Barn {
            name: name.into(),
            host: Some("10.0.0.2".into()),
            user: Some("forge".into()),
            port: Some(22),
            identity_file: None,
            critters: vec![],
            ..Default::default()
        }
    }

    fn bare_worm(name: &str) -> Worm {
        Worm {
            name: name.into(),
            command: "echo hi".into(),
            schedule: "* * * * *".into(),
            worm_type: "shell".into(),
            enabled: true,
            project: None,
            working_dir: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    fn bare_trail(name: &str) -> crate::trails::Trail {
        crate::trails::Trail {
            name: name.into(),
            on: None,
            env: None,
            jobs: Default::default(),
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    fn bare_ranchhand(name: &str) -> RanchHand {
        RanchHand {
            name: name.into(),
            project: "api".into(),
            rh_type: "kubernetes".into(),
            config: serde_yaml::Value::Null,
            sync_settings: RanchHandSyncSettings { auto_sync: false, interval_minutes: None },
            herd: "infra".into(),
            resource_mappings: vec![],
            last_sync: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    // ========================================================================
    // Loader hardening (Task 8)
    // ========================================================================

    /// The whole point of the change: a file that fails to parse used to
    /// disappear from the UI with no error at all.
    #[test]
    fn a_broken_project_file_is_reported_rather_than_hidden() {
        crate::testing::with_temp_ranch(|ranch| {
            let mut good = bare_project("good");
            save_project(&mut good).unwrap();

            std::fs::write(
                ranch.dir.path().join("projects").join("broken.yaml"),
                "name: [this is not\n  valid yaml",
            )
            .unwrap();

            let result = load_projects_checked();
            assert_eq!(result.items.len(), 1, "the good project must still load");
            assert_eq!(result.errors.len(), 1, "the broken file must be reported");
            assert!(
                result.errors[0].path.ends_with("broken.yaml"),
                "wrong file blamed: {:?}",
                result.errors[0].path
            );
            assert!(
                !result.errors[0].message.is_empty(),
                "an error with no message is no better than silence"
            );

            // The lenient wrapper keeps its old contract for existing callers.
            assert_eq!(load_projects().len(), 1);
        });
    }

    /// `store` keeps lock files and in-flight temp files inside these very
    /// directories. Surfacing either as a parse error would be worse than the
    /// silence this change removes.
    #[test]
    fn lock_and_temp_files_are_neither_items_nor_errors() {
        crate::testing::with_temp_ranch(|ranch| {
            let mut good = bare_project("good");
            save_project(&mut good).unwrap();

            let projects = ranch.dir.path().join("projects");
            std::fs::create_dir_all(projects.join(".locks")).unwrap();
            std::fs::write(projects.join(".locks").join("good.lock"), "").unwrap();
            std::fs::write(projects.join(".good.yaml.tmp-123-4"), "garbage{{{").unwrap();

            let result = load_projects_checked();
            assert_eq!(result.items.len(), 1, "only the real project counts");
            assert!(
                result.errors.is_empty(),
                "internal files must stay invisible, got {:?}",
                result.errors.iter().map(|e| &e.path).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn an_unreadable_entity_file_is_reported_not_skipped() {
        crate::testing::with_temp_ranch(|ranch| {
            use std::os::unix::fs::PermissionsExt;

            let path = ranch.dir.path().join("projects").join("locked-out.yaml");
            ensure_config_dirs();
            std::fs::write(&path, "name: x\npath: /tmp\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

            let result = load_projects_checked();

            // Restore before asserting so a failure cannot leave the tempdir
            // undeletable.
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));

            assert!(result.items.is_empty());
            assert_eq!(result.errors.len(), 1, "an unreadable file must be reported");
        });
    }

    #[test]
    fn load_barns_checked_keeps_local_at_the_head_and_drops_an_impostor() {
        crate::testing::with_temp_ranch(|ranch| {
            let mut barn = Barn {
                name: "pi".into(),
                host: Some("pi.local".into()),
                user: None,
                port: None,
                identity_file: None,
                critters: vec![],
                ..Default::default()
            };
            save_barn(&mut barn).unwrap();

            // A persisted barn claiming the reserved name must not appear.
            std::fs::write(
                ranch.dir.path().join("barns").join("local.yaml"),
                "name: local\nhost: impostor\n",
            )
            .unwrap();

            let result = load_barns_checked();
            assert_eq!(result.items[0].name, LOCAL_BARN_NAME, "local belongs first");
            assert!(result.items[0].host.is_none(), "the synthetic local won, not the file");
            assert_eq!(
                result.items.iter().filter(|b| b.name == LOCAL_BARN_NAME).count(),
                1,
                "exactly one barn may be spelled 'local'"
            );
            assert!(result.items.iter().any(|b| b.name == "pi"));
            assert!(result.errors.is_empty());
        });
    }

    #[test]
    fn a_broken_ranchhand_file_is_reported_rather_than_hidden() {
        crate::testing::with_temp_ranch(|ranch| {
            let dir = ranchhands_dir();
            ensure_config_dirs();
            std::fs::write(
                dir.join("k8s.yaml"),
                "name: k8s\nproject: api\ntype: kubernetes\nconfig:\n  context: foo\nsync_settings:\n  auto_sync: false\n  interval_minutes: null\nherd: web\n",
            )
            .unwrap();
            std::fs::write(dir.join("broken.yaml"), "name: [nope\n  bad").unwrap();
            let _ = ranch;

            let result = load_ranchhands_checked();
            assert_eq!(result.items.len(), 1, "the valid ranch hand must load");
            assert_eq!(result.items[0].name, "k8s");
            assert_eq!(result.errors.len(), 1, "the broken one must be reported");
            assert_eq!(load_ranchhands().len(), 1, "lenient wrapper unchanged");
        });
    }

    // ========================================================================
    // Tombstones on deletion (Task 10)
    // ========================================================================

    #[test]
    fn delete_project_records_a_tombstone_with_the_entity_id() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();
            let id = p.id.clone().unwrap();

            assert!(delete_project("api").unwrap());

            let stones = crate::tombstones::load_all();
            assert_eq!(stones.len(), 1);
            assert_eq!(stones[0].id, id, "tombstone must carry the entity's uuid");
            assert_eq!(stones[0].kind, "project");
            assert_eq!(stones[0].name, "api");
        });
    }

    /// A deletion of something that was never there is not a deletion. Recording
    /// one would hand every peer an instruction to delete an entity this machine
    /// never had — and `delete_*` returning `false` is exactly how the callers
    /// spell "nothing to do".
    #[test]
    fn deleting_a_missing_project_records_nothing() {
        testing::with_temp_ranch(|_| {
            assert!(!delete_project("nope").unwrap());
            assert!(crate::tombstones::load_all().is_empty());
        });
    }

    /// Every top-level entity, not just projects. Each is entombed under its own
    /// kind, because the kind plus the uuid is what a peer matches against.
    #[test]
    fn every_deleter_records_a_tombstone_carrying_the_entitys_id() {
        testing::with_temp_ranch(|_| {
            let mut barn = bare_barn("pi");
            save_barn(&mut barn).unwrap();
            let mut worm = bare_worm("nightly");
            save_worm(&mut worm).unwrap();
            let mut trail = bare_trail("deploy");
            save_trail(&mut trail).unwrap();
            let mut rh = bare_ranchhand("k8s");
            save_ranchhand(&mut rh).unwrap();

            assert!(delete_barn("pi").unwrap());
            assert!(delete_worm("nightly").unwrap());
            assert!(delete_trail("deploy").unwrap());
            assert!(delete_ranchhand("k8s").unwrap());

            let mut got: Vec<(String, String, String)> = crate::tombstones::load_all()
                .into_iter()
                .map(|t| (t.kind, t.name, t.id))
                .collect();
            got.sort();

            let mut want = vec![
                ("barn".to_string(), "pi".to_string(), barn.id.clone().unwrap()),
                ("ranchhand".to_string(), "k8s".to_string(), rh.id.clone().unwrap()),
                ("trail".to_string(), "deploy".to_string(), trail.id.clone().unwrap()),
                ("worm".to_string(), "nightly".to_string(), worm.id.clone().unwrap()),
            ];
            want.sort();

            assert_eq!(got, want);
        });
    }

    /// `local` is injected into every listing and never written to disk, so
    /// there is no file to read an id from and no entity for a peer to delete.
    /// `delete_barn` returns `false` for it before anything else happens.
    #[test]
    fn deleting_the_local_barn_entombs_nothing() {
        testing::with_temp_ranch(|_| {
            ensure_config_dirs();
            assert!(!delete_barn(LOCAL_BARN_NAME).unwrap());
            assert!(
                crate::tombstones::load_all().is_empty(),
                "the synthetic local barn has nothing to entomb"
            );
        });
    }

    /// The critical asymmetry between a rename and a delete.
    ///
    /// A rename keeps the uuid and moves the file; the surviving uuid *is* the
    /// signal that says "renamed, not replaced". A tombstone says the opposite —
    /// it tells every peer to delete that uuid. Emit both and the sync deletes
    /// the project the user just renamed.
    ///
    /// This is a live hazard rather than a hypothetical one: `rename_project`
    /// removes the old file itself instead of calling `delete_project`, and the
    /// obvious "simplification" is to route it through `delete_project` — which
    /// would silently entomb the survivor.
    #[test]
    fn renaming_a_project_records_no_tombstone() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();
            let id = p.id.clone().unwrap();

            let mut renamed = p.clone();
            renamed.name = "gateway".into();
            rename_project("api", &mut renamed).unwrap();

            assert!(
                crate::tombstones::load_all().is_empty(),
                "a rename must leave no tombstone — the surviving uuid is the \
                 rename signal, and a tombstone would tell peers to delete it"
            );

            let loaded = load_projects();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].name, "gateway");
            assert_eq!(loaded[0].id.as_deref(), Some(id.as_str()));
        });
    }

    /// Renaming a project to the name it already has short-circuits into
    /// `save_project`. That path must stay tombstone-free too.
    #[test]
    fn renaming_a_project_to_its_own_name_records_no_tombstone() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();

            let mut same = p.clone();
            rename_project("api", &mut same).unwrap();

            assert!(crate::tombstones::load_all().is_empty());
        });
    }

    /// Every file already on disk predates Task 6 and carries no uuid, and
    /// nothing restamps one until it is next saved. Deleting such a project
    /// still has to be recorded — under the deterministic fallback key, because
    /// a deletion nobody wrote down is a resurrection on the next sync.
    #[test]
    fn deleting_a_project_that_was_never_stamped_records_the_fallback_key() {
        testing::with_temp_ranch(|_| {
            ensure_config_dirs();
            fs::write(
                projects_dir().join("legacy.yaml"),
                "name: legacy\npath: /tmp/legacy\n",
            )
            .unwrap();

            assert!(delete_project("legacy").unwrap());

            let stones = crate::tombstones::load_all();
            assert_eq!(stones.len(), 1);
            assert_eq!(stones[0].id, "project--legacy");
            assert_eq!(stones[0].kind, "project");
        });
    }

    /// The id has to be read out of the file while the file still exists.
    /// Reading it after the `remove_file` yields `None` every time, and the
    /// tombstone silently degrades to the fallback key — which is not the uuid
    /// the peer holding this project is matching on, so the deletion never
    /// cancels their copy and the project comes back.
    #[test]
    fn a_deletion_tombstone_is_never_the_fallback_key_when_the_entity_had_an_id() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            save_project(&mut p).unwrap();

            delete_project("api").unwrap();

            let stones = crate::tombstones::load_all();
            assert_eq!(stones.len(), 1);
            assert_ne!(
                stones[0].id, "project--api",
                "the id was read after the file was removed, so it was lost"
            );
        });
    }

    // ------------------------------------------------------------------
    // this_barn / save_config
    // ------------------------------------------------------------------

    /// A `config.yaml` written before the Slack integration was removed still
    /// has a `slack:` block in it, and `Config` no longer has a field for it.
    ///
    /// This is the one test standing between that file and silent data loss.
    /// `load_config` ends in `serde_yaml::from_str(..).unwrap_or_default()`, so a
    /// deserialize error is not an error the user ever sees — it is a `Config`
    /// with every field back at its default. The theory that carries the removal
    /// is that serde ignores a key it has no field for unless the struct asks for
    /// `deny_unknown_fields`, which `Config` does not; if that theory were wrong,
    /// the failure mode would be the whole config quietly reverting on first
    /// read. So the assertions are about the *other* fields, not about the
    /// absence of a panic: `editor`, `theme` and `default_project` are the proof
    /// that the parse succeeded rather than fell back.
    #[test]
    fn a_config_with_a_leftover_slack_block_still_loads_every_other_field() {
        testing::with_temp_ranch(|_| {
            ensure_config_dirs();
            let yaml = "\
version: 1
default_project: api
editor: nvim
theme: light
show_activity: false
claude:
  model: claude-opus-4
  auto_attach: false
tmux:
  session_prefix: 'yh-'
  default_shell: /bin/bash
slack:
  enabled: true
  allowed_users:
    - U12345
  default_project: api
  channel_projects:
    C0001: api
  system_prompt: be brief
this_barn: imac
";
            crate::store::write_atomic(&config_file(), yaml).unwrap();

            let cfg = load_config();
            assert_eq!(cfg.default_project.as_deref(), Some("api"));
            assert_eq!(cfg.editor, "nvim");
            assert_eq!(cfg.theme, "light");
            assert!(!cfg.show_activity);
            assert_eq!(cfg.claude.model, "claude-opus-4");
            assert!(!cfg.claude.auto_attach);
            assert_eq!(cfg.tmux.default_shell, "/bin/bash");
            assert_eq!(cfg.this_barn.as_deref(), Some("imac"));
        });
    }

    /// The same file, re-saved: the stale `slack:` block does not come back.
    ///
    /// Asserted because it is the half of the story the test above does not
    /// cover. `save_config` serializes `Config`, which has no such field, so the
    /// key is dropped on the next write — and the rest of the file has to survive
    /// that write intact.
    #[test]
    fn saving_a_config_that_had_a_slack_block_drops_the_block_and_keeps_the_rest() {
        testing::with_temp_ranch(|_| {
            ensure_config_dirs();
            crate::store::write_atomic(
                &config_file(),
                "version: 1\ndefault_project: api\neditor: nvim\nslack:\n  enabled: true\n",
            )
            .unwrap();

            let cfg = load_config();
            save_config(&cfg).unwrap();

            let written = std::fs::read_to_string(config_file()).unwrap();
            assert!(!written.contains("slack"), "the removed block came back:\n{written}");
            assert_eq!(load_config().default_project.as_deref(), Some("api"));
            assert_eq!(load_config().editor, "nvim");
        });
    }

    /// A ranch that has never been adopted has no machine name, and once one is
    /// recorded it has to survive the trip through the file — the whole point of
    /// the field is that a peer reading this ranch learns which machine it is.
    #[test]
    fn this_barn_defaults_to_none_and_round_trips() {
        testing::with_temp_ranch(|_| {
            assert!(load_config().this_barn.is_none());

            let mut cfg = load_config();
            cfg.this_barn = Some("imac".into());
            save_config(&cfg).unwrap();

            assert_eq!(load_config().this_barn.as_deref(), Some("imac"));
        });
    }

    /// `local` is a display alias for "the barn I am running on". Before
    /// adoption there is no such barn and it has to stay literal; after, it has
    /// to name the machine — otherwise the alias points at a barn that exists
    /// nowhere in the store.
    #[test]
    fn resolve_barn_name_maps_local_to_this_barn_only_after_adoption() {
        testing::with_temp_ranch(|_| {
            assert_eq!(resolve_barn_name(LOCAL_BARN_NAME), LOCAL_BARN_NAME);
            assert_eq!(resolve_barn_name("pi"), "pi");

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert_eq!(resolve_barn_name(LOCAL_BARN_NAME), "imac");
            assert_eq!(
                resolve_barn_name("pi"),
                "pi",
                "only the alias resolves; a real name is already the stored name"
            );
        });
    }

    /// The one reader of `config.this_barn`. Everything that resolves the alias
    /// goes through it, so a version that answers `None` on an adopted ranch
    /// silently un-migrates every lookup.
    #[test]
    fn this_barn_name_reports_what_adoption_recorded() {
        testing::with_temp_ranch(|_| {
            assert!(this_barn_name().is_none());

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert_eq!(this_barn_name().as_deref(), Some("imac"));
        });
    }

    /// Adoption rewrites `barn: None` to a real name, so a `local` lookup that
    /// still only matches `None` finds nothing the moment the machine is
    /// adopted — the local barn's whole livestock list would empty out.
    #[test]
    fn get_livestock_for_barn_follows_this_barn_after_adoption() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            p.livestock.push(bare_livestock("web"));
            save_project(&mut p).unwrap();

            // Before adoption: "local" finds it.
            assert_eq!(get_livestock_for_barn(LOCAL_BARN_NAME).len(), 1);

            crate::migrate::adopt_this_machine("imac").unwrap();

            // After adoption: the real name finds it, and "local" still does,
            // because "local" now means "the barn I am running on".
            assert_eq!(get_livestock_for_barn("imac").len(), 1);
            assert_eq!(get_livestock_for_barn(LOCAL_BARN_NAME).len(), 1);
        });
    }

    /// A ranch that has never been adopted must behave exactly as it did before
    /// any of this existed: `local` means "unpinned", and a named barn means
    /// only what is pinned to it.
    #[test]
    fn before_adoption_local_still_means_an_unpinned_livestock() {
        testing::with_temp_ranch(|_| {
            let mut p = bare_project("api");
            p.livestock.push(bare_livestock("web"));
            let mut pinned = bare_livestock("worker");
            pinned.barn = Some("pi".into());
            p.livestock.push(pinned);
            save_project(&mut p).unwrap();

            assert_eq!(get_livestock_for_barn(LOCAL_BARN_NAME).len(), 1);
            assert_eq!(get_livestock_for_barn("pi").len(), 1);
            assert_eq!(get_livestock_for_barn("pi")[0].1.name, "worker");
        });
    }

    // === the machine-relative barn helpers ==================================

    /// Every spelling of "this machine", before and after adoption. A version
    /// that only knows `None` treats an adopted machine's own livestock as
    /// remote, which is what every call site outside `get_livestock_for_barn`
    /// used to do.
    #[test]
    fn livestock_is_on_this_machine_knows_every_spelling_of_here() {
        testing::with_temp_ranch(|_| {
            let unpinned = bare_livestock("web");
            let mut literal_local = bare_livestock("ios");
            literal_local.barn = Some(LOCAL_BARN_NAME.into());
            let mut remote = bare_livestock("worker");
            remote.barn = Some("pi".into());
            let mut adopted = bare_livestock("api");
            adopted.barn = Some("imac".into());

            // Before adoption.
            assert!(livestock_is_on_this_machine(&unpinned));
            assert!(livestock_is_on_this_machine(&literal_local));
            assert!(!livestock_is_on_this_machine(&remote));
            assert!(
                !livestock_is_on_this_machine(&adopted),
                "an unadopted machine is not `imac`"
            );

            crate::migrate::adopt_this_machine("imac").unwrap();

            // After adoption. `unpinned` is what a peer's file still says and
            // what a half-migrated ranch keeps; `adopted` is what this
            // machine's own files now say.
            assert!(livestock_is_on_this_machine(&unpinned));
            assert!(livestock_is_on_this_machine(&literal_local));
            assert!(livestock_is_on_this_machine(&adopted));
            assert!(!livestock_is_on_this_machine(&remote));
        });
    }

    /// `None` is the machine-relative answer on purpose: it is exactly what the
    /// livestock said before adoption, so a call site that swaps
    /// `ls.barn.as_deref()` for this behaves identically on both sides of the
    /// migration.
    #[test]
    fn resolve_livestock_barn_reports_none_for_this_machine_and_the_name_otherwise() {
        testing::with_temp_ranch(|_| {
            let unpinned = bare_livestock("web");
            let mut literal_local = bare_livestock("ios");
            literal_local.barn = Some(LOCAL_BARN_NAME.into());
            let mut remote = bare_livestock("worker");
            remote.barn = Some("pi".into());
            let mut adopted = bare_livestock("api");
            adopted.barn = Some("imac".into());

            assert_eq!(resolve_livestock_barn(&unpinned), None);
            assert_eq!(resolve_livestock_barn(&literal_local), None);
            assert_eq!(resolve_livestock_barn(&remote), Some("pi"));
            assert_eq!(resolve_livestock_barn(&adopted), Some("imac"));

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert_eq!(resolve_livestock_barn(&unpinned), None);
            assert_eq!(resolve_livestock_barn(&literal_local), None);
            assert_eq!(
                resolve_livestock_barn(&remote),
                Some("pi"),
                "a real remote barn is untouched by adoption"
            );
            assert_eq!(
                resolve_livestock_barn(&adopted),
                None,
                "after adoption this machine's own name means here, not away"
            );
        });
    }

    /// The `Barn` record form of the same question. Adoption persists a real
    /// barn for this machine, so `is_local_barn` — which only knows the
    /// synthetic `local` — starts answering `false` about the machine the user
    /// is sitting at.
    #[test]
    fn barn_is_this_machine_recognises_the_adopted_self_barn() {
        testing::with_temp_ranch(|_| {
            let synthetic = local_barn();
            let mut imac = bare_barn("imac");
            imac.host = None;
            let pi = bare_barn("pi");

            assert!(barn_is_this_machine(&synthetic));
            assert!(!barn_is_this_machine(&imac));
            assert!(!barn_is_this_machine(&pi));

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert!(barn_is_this_machine(&synthetic));
            assert!(barn_is_this_machine(&imac));
            assert!(!barn_is_this_machine(&pi));
        });
    }

    /// Two names for one machine have to compare equal, or a filter written
    /// with one spelling silently drops everything tagged with the other.
    #[test]
    fn canonical_barn_name_collapses_both_spellings_of_this_machine() {
        testing::with_temp_ranch(|_| {
            assert_eq!(canonical_barn_name(LOCAL_BARN_NAME), LOCAL_BARN_NAME);
            assert_eq!(canonical_barn_name("imac"), "imac");
            assert_eq!(canonical_barn_name("pi"), "pi");

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert_eq!(canonical_barn_name(LOCAL_BARN_NAME), LOCAL_BARN_NAME);
            assert_eq!(
                canonical_barn_name("imac"),
                LOCAL_BARN_NAME,
                "this machine's real name canonicalises to the alias"
            );
            assert_eq!(canonical_barn_name("pi"), "pi");
        });
    }

    /// The display side of the alias. `local` means "the machine you are
    /// sitting at", and that is the question these columns answer — is this
    /// here, or somewhere else? Adoption gives the machine a name for *syncing*
    /// purposes; it must not start showing the user their own hostname where
    /// they read `local` yesterday.
    #[test]
    fn barn_label_still_reads_local_for_this_machine_after_adoption() {
        testing::with_temp_ranch(|_| {
            let unpinned = bare_livestock("web");
            let mut literal_local = bare_livestock("ios");
            literal_local.barn = Some(LOCAL_BARN_NAME.into());
            let mut remote = bare_livestock("worker");
            remote.barn = Some("pi".into());
            let mut adopted = bare_livestock("api");
            adopted.barn = Some("imac".into());

            assert_eq!(barn_label(&unpinned), LOCAL_BARN_NAME);
            assert_eq!(barn_label(&literal_local), LOCAL_BARN_NAME);
            assert_eq!(barn_label(&remote), "pi");

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert_eq!(barn_label(&unpinned), LOCAL_BARN_NAME);
            assert_eq!(barn_label(&literal_local), LOCAL_BARN_NAME);
            assert_eq!(
                barn_label(&adopted),
                LOCAL_BARN_NAME,
                "the machine's own name is not news to the user sitting at it"
            );
            assert_eq!(barn_label(&remote), "pi");
        });
    }

    /// What a barn picker should write. The literal `local` is the one value
    /// that must never reach disk: it is machine-relative with a different
    /// spelling than `None`, so it carries the identical sync defect.
    #[test]
    fn stored_barn_name_never_writes_the_literal_local() {
        testing::with_temp_ranch(|_| {
            assert_eq!(stored_barn_name(LOCAL_BARN_NAME), None);
            assert_eq!(stored_barn_name(""), None);
            assert_eq!(stored_barn_name("  local  "), None);
            assert_eq!(stored_barn_name("Local"), None, "case is not a new barn");
            assert_eq!(stored_barn_name("pi"), Some("pi".to_string()));

            crate::migrate::adopt_this_machine("imac").unwrap();

            assert_eq!(
                stored_barn_name(LOCAL_BARN_NAME),
                Some("imac".to_string()),
                "an adopted machine writes its real name, which is the point"
            );
            assert_eq!(stored_barn_name(""), Some("imac".to_string()));
            assert_eq!(stored_barn_name("pi"), Some("pi".to_string()));
        });
    }

    /// A file can say `barn: local` and never pass through the migration: the
    /// UI's barn picker writes it, and a peer can sync one in after this machine
    /// was adopted. `local` still means "whichever machine reads this", so the
    /// resolver has to keep treating it as machine-relative — otherwise the
    /// livestock is visible under no barn at all.
    #[test]
    fn a_livestock_pinned_to_the_literal_local_barn_is_still_this_machine_after_adoption() {
        testing::with_temp_ranch(|_| {
            let mut cfg = load_config();
            cfg.this_barn = Some("imac".into());
            save_config(&cfg).unwrap();

            let mut p = bare_project("api");
            let mut ls = bare_livestock("ios");
            ls.barn = Some(LOCAL_BARN_NAME.into());
            p.livestock.push(ls);
            save_project(&mut p).unwrap();

            assert_eq!(get_livestock_for_barn(LOCAL_BARN_NAME).len(), 1);
            assert_eq!(get_livestock_for_barn("imac").len(), 1);
        });
    }
}
