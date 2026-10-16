//! Sync mechanics: snapshot, diff, transfer.
//!
//! Nothing here runs on its own. The sync view asks for a diff against one
//! barn, and the user pushes or pulls one entity at a time. Deletions are never
//! propagated: an entity missing on one side is offered for copying, never
//! removed from the other.
//!
//! - Remote side: `yeehaw sync dump` prints a [`Snapshot`] as JSON;
//!   `yeehaw sync put <kind>` reads one entity as JSON on stdin and saves it
//!   with the receiving side's machine-local fields preserved ([`put`]).
//! - Local side: [`fetch`] runs the dump over ssh, [`diff`] compares by
//!   `(kind, name)` with `canonical::hash_entity`, [`push`] runs `sync put`
//!   over ssh, and [`pull`] is a local [`put`].

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{anyhow, bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use crate::config;
use crate::sync::canonical;
use crate::trails::Trail;
use crate::types::{Barn, Project, RanchHand, Worm};

// ============================================================================
// Kinds
// ============================================================================

/// The five synced kinds, declared in display order — the derived `Ord` is what
/// sorts diff rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Project,
    Barn,
    Worm,
    Trail,
    Ranchhand,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Project, Kind::Barn, Kind::Worm, Kind::Trail, Kind::Ranchhand];

    /// The spelling `canonical::SHAPES` and `yeehaw sync put <kind>` use.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Project => "project",
            Kind::Barn => "barn",
            Kind::Worm => "worm",
            Kind::Trail => "trail",
            Kind::Ranchhand => "ranchhand",
        }
    }

    pub fn parse(s: &str) -> Result<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| anyhow!("unknown kind '{}' (expected project, barn, worm, trail or ranchhand)", s))
    }

    fn dir(self) -> PathBuf {
        match self {
            Kind::Project => config::projects_dir(),
            Kind::Barn => config::barns_dir(),
            Kind::Worm => config::worms_dir(),
            Kind::Trail => config::trails_dir(),
            Kind::Ranchhand => config::ranchhands_dir(),
        }
    }
}

// ============================================================================
// Snapshot (`yeehaw sync dump`)
// ============================================================================

/// Everything one machine would offer to sync. The JSON `sync dump` prints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub this_barn: Option<String>,
    pub projects: Vec<Project>,
    pub barns: Vec<Barn>,
    pub worms: Vec<Worm>,
    pub trails: Vec<Trail>,
    pub ranchhands: Vec<RanchHand>,
}

/// This machine's snapshot, through config's normal loaders.
///
/// Livestock that say "this machine" (`barn: local`, or no barn at all) are
/// rewritten to this machine's real barn name, so `local` never crosses
/// machines: on the receiver it would mean the receiver.
pub fn local_snapshot() -> Snapshot {
    let mut snapshot = Snapshot {
        this_barn: config::this_barn_name(),
        projects: config::load_projects(),
        barns: load_barn_files(),
        worms: config::load_worms(),
        trails: config::load_all_trails(),
        ranchhands: config::load_ranchhands(),
    };
    name_local_livestock(&mut snapshot);
    snapshot
}

/// Barn records on disk, minus anything spelled `local`.
///
/// Not `config::load_barns`: that injects the synthetic `local` row, and a
/// legacy `barns/local.yaml` may not even parse — so the `local` stem is skipped
/// before the file is opened. Unparseable files are skipped; `sync` is not the
/// place to report them, and the barns panel already does.
fn load_barn_files() -> Vec<Barn> {
    let mut barns: Vec<Barn> = fs::read_dir(config::barns_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "yaml"))
        .filter(|path| path.file_stem().is_some_and(|s| s != config::LOCAL_BARN_NAME))
        .filter_map(|path| fs::read_to_string(path).ok())
        .filter_map(|content| serde_yaml::from_str::<Barn>(&content).ok())
        .filter(|barn| barn.name != config::LOCAL_BARN_NAME)
        .collect();
    barns.sort_by(|a, b| a.name.cmp(&b.name));
    barns
}

/// Rewrites livestock pinned to "this machine" to the snapshot's own barn name.
/// A no-op on a machine that has not been branded with a name yet.
///
/// Both spellings of "this machine" are rewritten: the literal `local` and an
/// absent barn (`config::livestock_is_on_this_machine` treats them alike).
fn name_local_livestock(snapshot: &mut Snapshot) {
    let Some(this_barn) = snapshot.this_barn.clone() else { return };
    for livestock in snapshot.projects.iter_mut().flat_map(|p| p.livestock.iter_mut()) {
        if livestock.barn.as_deref().is_none_or(|b| b == config::LOCAL_BARN_NAME) {
            livestock.barn = Some(this_barn.clone());
        }
    }
}

/// The JSON `yeehaw sync dump` prints.
pub fn dump_json() -> Result<String> {
    serde_json::to_string(&local_snapshot()).context("failed to serialize sync snapshot")
}

// ============================================================================
// Put (`yeehaw sync put <kind>`, and the local half of a pull)
// ============================================================================

/// Saves one entity, given as JSON, through config's save functions.
///
/// The receiving side keeps its own machine-local fields (those listed in
/// `canonical::SHAPES` for the kind — e.g. `project.path`, `barn.connectable`)
/// when the entity already exists. When it does not, the sender's are dropped:
/// another machine's checkout path or reachability verdict means nothing here.
pub fn put(kind: Kind, json: &str) -> Result<()> {
    // Unbranded, this machine could only file incoming livestock under `local`
    // or no barn at all — both of which mean "whoever reads this".
    if config::this_barn_name().is_none() {
        bail!(UNBRANDED_HERE);
    }
    match kind {
        Kind::Project => put_typed(kind, json, config::save_project),
        Kind::Barn => put_typed(kind, json, config::save_barn),
        Kind::Worm => {
            put_typed(kind, json, config::save_worm)?;
            // Every other worm save path does this; a received worm that is
            // enabled is not scheduled until the crontab is rewritten. The worm
            // itself is saved either way.
            crate::crontab::sync_crontab()
                .map_err(|e| anyhow!("worm saved, but updating the crontab failed: {}", e))
        }
        Kind::Trail => put_typed(kind, json, config::save_trail),
        Kind::Ranchhand => put_typed(kind, json, config::save_ranchhand),
    }
}

fn put_typed<T: Serialize + DeserializeOwned>(
    kind: Kind,
    json: &str,
    save: fn(&mut T) -> Result<()>,
) -> Result<()> {
    let mut incoming: Value =
        serde_json::from_str(json).with_context(|| format!("{} is not valid JSON", kind.as_str()))?;
    let name = incoming
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{} has no name", kind.as_str()))?
        .to_string();
    check_name(kind, &name)?;

    let dir = kind.dir();
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    // Held across the read of the existing entity and the save, so a local
    // edit landing in between is not silently overwritten.
    let _guard = crate::store::lock_entity(&dir, &name)?;

    let path = dir.join(format!("{}.yaml", name));
    let existing: Option<Value> = if path.exists() {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let entity: T = serde_yaml::from_str(&content).with_context(|| {
            format!("existing {} '{}' does not parse, refusing to overwrite it", kind.as_str(), name)
        })?;
        Some(serde_json::to_value(entity)?)
    } else {
        None
    };
    keep_machine_local(kind, &mut incoming, existing.as_ref());

    let mut entity: T = serde_json::from_value(incoming)
        .with_context(|| format!("not a valid {}", kind.as_str()))?;
    save(&mut entity)
}

/// The same rule `config`'s private `validate_name` applies, plus `local`
/// for barns. Not every `config::save_*` checks (ranch hands do not), and a
/// name arriving over the wire becomes a filename.
fn check_name(kind: Kind, name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        bail!("invalid {} name '{}'", kind.as_str(), name);
    }
    if kind == Kind::Barn && name == config::LOCAL_BARN_NAME {
        bail!("'local' is this machine's alias, not a barn that can be synced");
    }
    if kind == Kind::Barn && config::this_barn_name().as_deref() == Some(name) {
        bail!(
            "barn '{}' is this machine's own barn record, which is never synced — \
             another machine's copy describes it as seen from there",
            name
        );
    }
    Ok(())
}

/// Replaces `incoming`'s machine-local fields with the receiver's: copied from
/// `existing` when there is one, blanked when there is not.
///
/// Field names come from `canonical::SHAPES`, so this tracks whatever that
/// list says is machine-local. A blank is `null`, except `project.path`, which
/// is a plain `String` and so becomes `""`.
fn keep_machine_local(kind: Kind, incoming: &mut Value, existing: Option<&Value>) {
    let Some(shape) = canonical::SHAPES.iter().find(|s| s.kind == kind.as_str()) else { return };
    let Some(fields) = incoming.as_object_mut() else { return };
    for &key in shape.machine_local {
        let value = match existing {
            Some(existing) => existing.get(key).cloned().unwrap_or(Value::Null),
            None if kind == Kind::Project && key == "path" => Value::String(String::new()),
            None => Value::Null,
        };
        fields.insert(key.to_string(), value);
    }
}

// ============================================================================
// Diff
// ============================================================================

const UNBRANDED_HERE: &str = "this machine has no barn name — run `yeehaw brand <name>`";

fn unbranded_there(barn: &str) -> String {
    format!("{} has no barn name — run `yeehaw brand <name>` on it", barn)
}

/// Which side's `updated_at` is later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newer {
    Here,
    There,
    /// Missing or unparseable timestamp on either side, or identical instants.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Only on this machine — can push.
    OnlyHere,
    /// Only on the barn — can pull.
    OnlyThere,
    /// On both, with different content — push or pull.
    Differs(Newer),
}

/// One entity that is not the same on both sides.
#[derive(Debug, Clone)]
pub struct Row {
    pub kind: Kind,
    pub name: String,
    pub status: Status,
    /// The entity as this machine has it (livestock already named), if it does.
    pub here: Option<Value>,
    /// The entity as the barn has it, if it does.
    pub there: Option<Value>,
}

struct Entry {
    hash: String,
    updated_at: Option<String>,
    value: Value,
}

/// Every `(kind, name)` that is not identical on both sides, sorted by kind
/// (projects, barns, worms, trails, ranchhands) and then name.
///
/// Compared with `canonical::hash_entity`, which ignores identity and
/// machine-local fields. Both sides get the livestock rewrite first, so a
/// livestock that says `local` here compares equal to the same livestock
/// naming this machine there.
pub fn diff(here: &Snapshot, there: &Snapshot) -> Result<Vec<Row>> {
    let (Some(here_name), Some(there_name)) = (here.this_barn.clone(), there.this_barn.clone()) else {
        bail!(if here.this_barn.is_none() {
            UNBRANDED_HERE.to_string()
        } else {
            unbranded_there("the other machine")
        });
    };
    let mut here = here.clone();
    let mut there = there.clone();
    name_local_livestock(&mut here);
    name_local_livestock(&mut there);

    let mut both: BTreeMap<(Kind, String), (Option<Entry>, Option<Entry>)> = BTreeMap::new();
    for (kind, name, entry) in entries(&here)? {
        both.entry((kind, name)).or_insert((None, None)).0 = Some(entry);
    }
    for (kind, name, entry) in entries(&there)? {
        both.entry((kind, name)).or_insert((None, None)).1 = Some(entry);
    }

    // Each machine's own barn record is the hostless one adoption wrote
    // (`source: self`). Offered, it would overwrite the other side's working
    // record of that machine. Never synced, in either direction.
    both.retain(|(kind, name), _| {
        !(*kind == Kind::Barn && (*name == here_name || *name == there_name))
    });

    let mut rows: Vec<Row> = both
        .into_iter()
        .filter_map(|((kind, name), sides)| {
            let status = match &sides {
                (Some(_), None) => Status::OnlyHere,
                (None, Some(_)) => Status::OnlyThere,
                (Some(h), Some(t)) if h.hash == t.hash => return None,
                (Some(h), Some(t)) => {
                    Status::Differs(newer(h.updated_at.as_deref(), t.updated_at.as_deref()))
                }
                (None, None) => return None,
            };
            let (h, t) = sides;
            Some(Row { kind, name, status, here: h.map(|e| e.value), there: t.map(|e| e.value) })
        })
        .collect();
    // Already in (kind, name) order from the BTreeMap; explicit so it survives
    // a change of container.
    rows.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    Ok(rows)
}

/// Parsed, never string-compared: RFC 3339 offsets and the `Z` suffix do not
/// sort chronologically as text.
fn newer(here: Option<&str>, there: Option<&str>) -> Newer {
    let parse = |ts: Option<&str>| ts.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
    match (parse(here), parse(there)) {
        (Some(h), Some(t)) if h > t => Newer::Here,
        (Some(h), Some(t)) if h < t => Newer::There,
        _ => Newer::Unknown,
    }
}

fn entries(s: &Snapshot) -> Result<Vec<(Kind, String, Entry)>> {
    let mut out = Vec::new();
    collect(&mut out, Kind::Project, &s.projects, |e| &e.name, |e| e.updated_at.as_deref())?;
    collect(&mut out, Kind::Barn, &s.barns, |e| &e.name, |e| e.updated_at.as_deref())?;
    collect(&mut out, Kind::Worm, &s.worms, |e| &e.name, |e| e.updated_at.as_deref())?;
    collect(&mut out, Kind::Trail, &s.trails, |e| &e.name, |e| e.updated_at.as_deref())?;
    collect(&mut out, Kind::Ranchhand, &s.ranchhands, |e| &e.name, |e| e.updated_at.as_deref())?;
    Ok(out)
}

fn collect<T: Serialize>(
    out: &mut Vec<(Kind, String, Entry)>,
    kind: Kind,
    items: &[T],
    name: fn(&T) -> &String,
    updated_at: fn(&T) -> Option<&str>,
) -> Result<()> {
    for item in items {
        let entry = Entry {
            hash: canonical::hash_entity(kind.as_str(), item)?,
            updated_at: updated_at(item).map(str::to_string),
            value: serde_json::to_value(item)?,
        };
        out.push((kind, name(item).clone(), entry));
    }
    Ok(())
}

// ============================================================================
// Transfer
// ============================================================================

pub const DUMP_CMD: &str = "bash -lc 'yeehaw sync dump'";

pub fn put_cmd(kind: Kind) -> String {
    format!("bash -lc 'yeehaw sync put {}'", kind.as_str())
}

/// Runs a command on a barn. The seam that keeps tests off the network.
pub trait Remote: Send + Sync {
    /// Runs `remote_cmd` on `barn`, feeding it `stdin` if given, and returns
    /// stdout. A failure comes back as an error a person can read.
    fn run(&self, barn: &Barn, remote_cmd: &str, stdin: Option<&str>) -> Result<String>;
}

/// The real [`Remote`]: `ssh::command`, batch mode, stdin piped.
pub struct Ssh;

impl Remote for Ssh {
    fn run(&self, barn: &Barn, remote_cmd: &str, stdin: Option<&str>) -> Result<String> {
        let opts = crate::ssh::Opts { batch: true, ..Default::default() };
        let cmd = crate::ssh::command(barn, remote_cmd, opts)?;
        run_command(cmd, &barn.name, stdin)
    }
}

/// Runs `cmd` (an ssh invocation, in production) feeding it `stdin`, and
/// returns its stdout or [`explain_failure`]'s sentence.
fn run_command(mut cmd: std::process::Command, barn: &str, stdin: Option<&str>) -> Result<String> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| anyhow!("failed to spawn ssh: {}", e))?;
    if let Some(input) = stdin {
        // Dropped at the end of the block, which closes the pipe and lets
        // the remote `sync put` see EOF.
        let mut pipe = child.stdin.take().ok_or_else(|| anyhow!("ssh stdin unavailable"))?;
        // A failed write is almost always EPIPE: the remote exited before
        // reading everything (an old yeehaw, a missing binary, a refusal).
        // Its exit code and stderr say why; the broken pipe does not. So fall
        // through and report those instead.
        let _ = pipe.write_all(input.as_bytes());
    }
    let output = child.wait_with_output().context("ssh did not finish")?;
    if !output.status.success() {
        bail!(explain_failure(barn, output.status.code(), &String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A failed remote `yeehaw sync …`, as a sentence.
///
/// An old yeehaw has no `sync` arm, so it falls through to launching the TUI,
/// which fails without a terminal — that is the tmux/terminal case below.
pub fn explain_failure(barn: &str, code: Option<i32>, stderr: &str) -> String {
    let stderr = stderr.trim();
    if code == Some(127) || stderr.contains("command not found") {
        format!("yeehaw isn't installed on barn {} (or isn't on its login PATH)", barn)
    } else if code == Some(255) {
        // ssh's own exit code: the remote command never ran.
        let why = if stderr.is_empty() { "ssh failed" } else { stderr };
        format!("couldn't reach barn {}: {}", barn, why)
    } else if stderr.contains("not a terminal") || stderr.contains("tmux") {
        not_understood(barn)
    } else if stderr.is_empty() {
        match code {
            Some(c) => format!("barn {}: yeehaw sync failed (exit {})", barn, c),
            None => format!("barn {}: yeehaw sync was killed", barn),
        }
    } else {
        format!("barn {}: {}", barn, stderr)
    }
}

fn not_understood(barn: &str) -> String {
    format!("barn {}'s yeehaw doesn't understand `sync` — update it", barn)
}

/// Parses what `sync dump` printed. Anything a login shell printed before the
/// JSON (a motd, a greeting) is skipped.
pub fn parse_dump(barn: &str, stdout: &str) -> Result<Snapshot> {
    let json = stdout.find('{').map(|i| &stdout[i..]).ok_or_else(|| anyhow!(not_understood(barn)))?;
    serde_json::from_str(json).map_err(|e| anyhow!("{} ({})", not_understood(barn), e))
}

/// The barn's snapshot, over ssh.
pub fn fetch(remote: &dyn Remote, barn: &Barn) -> Result<Snapshot> {
    let stdout = remote.run(barn, DUMP_CMD, None)?;
    let snapshot = parse_dump(&barn.name, &stdout)?;
    if snapshot.this_barn.is_none() {
        bail!(unbranded_there(&format!("barn {}", barn.name)));
    }
    Ok(snapshot)
}

/// Sends this machine's copy of `row`'s entity to `barn`.
pub fn push(remote: &dyn Remote, barn: &Barn, row: &Row) -> Result<()> {
    let entity = row
        .here
        .as_ref()
        .ok_or_else(|| anyhow!("{} '{}' is not on this machine — nothing to push", row.kind.as_str(), row.name))?;
    // `local`, or no barn at all, means "whichever machine reads this": on the
    // barn it would mean the barn. Only an unbranded machine produces these.
    if row.kind == Kind::Project {
        let says_this_machine = entity
            .get("livestock")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|ls| ls.get("barn").and_then(Value::as_str).is_none_or(|b| b == config::LOCAL_BARN_NAME));
        if says_this_machine {
            bail!(UNBRANDED_HERE);
        }
    }
    remote.run(barn, &put_cmd(row.kind), Some(&entity.to_string()))?;
    Ok(())
}

/// Saves the barn's copy of `row`'s entity here, keeping this machine's
/// machine-local fields.
pub fn pull(row: &Row) -> Result<()> {
    let entity = row
        .there
        .as_ref()
        .ok_or_else(|| anyhow!("{} '{}' is not on the barn — nothing to pull", row.kind.as_str(), row.name))?;
    put(row.kind, &entity.to_string())
}

// ============================================================================
// CLI
// ============================================================================

/// `yeehaw sync …`, given the arguments after `sync`.
pub fn run_cli(args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["dump"] => {
            println!("{}", dump_json()?);
            Ok(())
        }
        ["put", kind] => {
            let kind = Kind::parse(kind)?;
            let mut json = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut json)
                .context("failed to read entity from stdin")?;
            put(kind, &json)
        }
        _ => bail!("usage: yeehaw sync dump | yeehaw sync put <project|barn|worm|trail|ranchhand>"),
    }
}

// ============================================================================
// Test support
// ============================================================================

/// A [`Remote`] that answers `sync dump` with a canned snapshot and records
/// every call. Shared with the sync view's tests.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeRemote {
        /// Per barn name: what `sync dump` prints, or the error it fails with.
        pub dumps: Mutex<BTreeMap<String, std::result::Result<String, String>>>,
        /// Every `(barn, command, stdin)` run, in order.
        pub calls: Mutex<Vec<(String, String, Option<String>)>>,
        /// When set, every `sync put` fails with this.
        pub put_error: Mutex<Option<String>>,
    }

    impl FakeRemote {
        pub fn with_dump(barn: &str, snapshot: &Snapshot) -> Self {
            let fake = FakeRemote::default();
            fake.set_dump(barn, Ok(serde_json::to_string(snapshot).unwrap()));
            fake
        }

        pub fn set_dump(&self, barn: &str, dump: std::result::Result<String, String>) {
            self.dumps.lock().unwrap().insert(barn.to_string(), dump);
        }

        pub fn calls(&self) -> Vec<(String, String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Remote for FakeRemote {
        fn run(&self, barn: &Barn, remote_cmd: &str, stdin: Option<&str>) -> Result<String> {
            self.calls.lock().unwrap().push((
                barn.name.clone(),
                remote_cmd.to_string(),
                stdin.map(str::to_string),
            ));
            if remote_cmd == DUMP_CMD {
                return match self.dumps.lock().unwrap().get(&barn.name) {
                    Some(Ok(out)) => Ok(out.clone()),
                    Some(Err(e)) => Err(anyhow!(e.clone())),
                    None => Err(anyhow!("no fake dump for {}", barn.name)),
                };
            }
            match self.put_error.lock().unwrap().clone() {
                Some(e) => Err(anyhow!(e)),
                None => Ok(String::new()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRemote;
    use super::*;
    use crate::testing;

    fn project(name: &str, path: &str, summary: &str) -> Project {
        serde_yaml::from_str(&format!("name: {name}\npath: {path}\nsummary: {summary}\n")).unwrap()
    }

    fn project_with_livestock(name: &str, barns: &[Option<&str>]) -> Project {
        let mut p = project(name, "/src", "s");
        for (i, barn) in barns.iter().enumerate() {
            let barn_line = match barn {
                Some(b) => format!("barn: {b}\n"),
                None => String::new(),
            };
            p.livestock.push(
                serde_yaml::from_str(&format!("name: ls{i}\npath: /app\n{barn_line}")).unwrap(),
            );
        }
        p
    }

    fn worm(name: &str) -> Worm {
        serde_yaml::from_str(&format!(
            "name: {name}\ncommand: echo\nschedule: '* * * * *'\ntype: shell\nenabled: true\n"
        ))
        .unwrap()
    }

    fn trail(name: &str) -> Trail {
        serde_yaml::from_str(&format!("name: {name}\njobs: {{}}\n")).unwrap()
    }

    fn ranchhand(name: &str, config: &str, last_sync: Option<&str>) -> RanchHand {
        let last = last_sync.map(|t| format!("last_sync: '{t}'\n")).unwrap_or_default();
        serde_yaml::from_str(&format!(
            "name: {name}\nproject: api\ntype: kubernetes\nconfig: {config}\n\
             sync_settings: {{auto_sync: false}}\nherd: h\n{last}"
        ))
        .unwrap()
    }

    fn barn(name: &str, host: &str) -> Barn {
        Barn { name: name.into(), host: Some(host.into()), ..Default::default() }
    }

    fn stamped<T: crate::types::Identified>(mut t: T, ts: &str) -> T {
        t.set_updated_at(ts.into());
        t
    }

    /// An empty snapshot from a machine branded `name`.
    fn branded(name: &str) -> Snapshot {
        Snapshot { this_barn: Some(name.into()), ..Default::default() }
    }

    /// A temp ranch branded `name`, the way `yeehaw brand` leaves one.
    fn branded_ranch(name: &str) -> testing::TempRanch {
        let ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        let mut cfg = config::load_config();
        cfg.this_barn = Some(name.into());
        config::save_config(&cfg).unwrap();
        ranch
    }

    fn keys(rows: &[Row]) -> Vec<(Kind, String, Status)> {
        rows.iter().map(|r| (r.kind, r.name.clone(), r.status)).collect()
    }

    // ------------------------------------------------------------------ diff

    #[test]
    fn diff_offers_what_only_one_side_has() {
        let here = Snapshot { projects: vec![project("a", "/a", "x")], ..branded("imac") };
        let there = Snapshot { projects: vec![project("b", "/b", "y")], ..branded("pi") };
        assert_eq!(
            keys(&diff(&here, &there).unwrap()),
            vec![
                (Kind::Project, "a".into(), Status::OnlyHere),
                (Kind::Project, "b".into(), Status::OnlyThere),
            ]
        );
    }

    #[test]
    fn diff_omits_entities_that_differ_only_in_identity_or_machine_local_fields() {
        let mut mine = stamped(project("api", "/Users/cam/api", "same"), "2026-10-01T00:00:00Z");
        mine.id = Some("uuid-here".into());
        let mut theirs = stamped(project("api", "/home/cam/api", "same"), "2026-10-09T00:00:00Z");
        theirs.id = Some("uuid-there".into());
        let here = Snapshot { projects: vec![mine], ..branded("imac") };
        let there = Snapshot { projects: vec![theirs], ..branded("pi") };
        assert!(diff(&here, &there).unwrap().is_empty());
    }

    #[test]
    fn diff_marks_the_newer_side_by_instant_not_by_string() {
        // 10:00 at -05:00 is 15:00Z: later than 12:00Z, though it sorts lower as a string.
        let a = stamped(project("api", "/p", "mine"), "2026-10-15T10:00:00-05:00");
        let b = stamped(project("api", "/p", "theirs"), "2026-10-15T12:00:00Z");
        let here = Snapshot { projects: vec![a.clone()], ..branded("imac") };
        let there = Snapshot { projects: vec![b.clone()], ..branded("pi") };
        assert_eq!(diff(&here, &there).unwrap()[0].status, Status::Differs(Newer::Here));
        let here = Snapshot { projects: vec![b], ..branded("imac") };
        let there = Snapshot { projects: vec![a], ..branded("pi") };
        assert_eq!(diff(&here, &there).unwrap()[0].status, Status::Differs(Newer::There));
    }

    #[test]
    fn diff_says_unknown_when_a_timestamp_is_missing_or_garbage() {
        let a = stamped(project("api", "/p", "mine"), "not a time");
        let b = stamped(project("api", "/p", "theirs"), "2026-10-15T12:00:00Z");
        let here = Snapshot { projects: vec![a], ..branded("imac") };
        let there = Snapshot { projects: vec![b], ..branded("pi") };
        assert_eq!(diff(&here, &there).unwrap()[0].status, Status::Differs(Newer::Unknown));
        let here = Snapshot { projects: vec![project("api", "/p", "mine")], ..branded("imac") };
        let there = Snapshot {
            projects: vec![stamped(project("api", "/p", "theirs"), "2026-10-15T12:00:00Z")],
            ..branded("pi")
        };
        assert_eq!(diff(&here, &there).unwrap()[0].status, Status::Differs(Newer::Unknown));
    }

    #[test]
    fn diff_sorts_by_kind_then_name() {
        let here = Snapshot {
            projects: vec![project("z", "/z", "s")],
            barns: vec![barn("m", "m.ts.net")],
            worms: vec![worm("a"), worm("Ab")],
            trails: vec![trail("c")],
            ranchhands: vec![ranchhand("b", "{}", None)],
            ..branded("imac")
        };
        let names: Vec<(Kind, String)> = diff(&here, &branded("pi"))
            .unwrap()
            .into_iter()
            .map(|r| (r.kind, r.name))
            .collect();
        assert_eq!(
            names,
            vec![
                (Kind::Project, "z".into()),
                (Kind::Barn, "m".into()),
                (Kind::Worm, "Ab".into()),
                (Kind::Worm, "a".into()),
                (Kind::Trail, "c".into()),
                (Kind::Ranchhand, "b".into()),
            ]
        );
    }

    #[test]
    fn diff_treats_local_livestock_as_this_machines_name() {
        let here = Snapshot {
            this_barn: Some("imac".into()),
            projects: vec![project_with_livestock("api", &[Some("local"), None, Some("pi")])],
            ..Default::default()
        };
        let there = Snapshot {
            this_barn: Some("pi".into()),
            projects: vec![project_with_livestock("api", &[Some("imac"), Some("imac"), Some("pi")])],
            ..Default::default()
        };
        assert!(diff(&here, &there).unwrap().is_empty());

        // And what would be pushed carries the name, never `local`.
        let rows = diff(&here, &branded("pi")).unwrap();
        let pushed: Project = serde_json::from_value(rows[0].here.clone().unwrap()).unwrap();
        let barns: Vec<_> = pushed.livestock.iter().map(|l| l.barn.as_deref()).collect();
        assert_eq!(barns, vec![Some("imac"), Some("imac"), Some("pi")]);
    }

    /// `migrate::adopt_this_machine` writes each machine's own barn record
    /// hostless (`source: self`, `connectable: false`). Synced, it would wipe
    /// the other side's good record of the same machine — so neither machine's
    /// own name is ever offered.
    #[test]
    fn diff_excludes_barn_records_named_after_either_machine() {
        let here = Snapshot {
            this_barn: Some("imac".into()),
            barns: vec![barn("imac", "imac.old"), barn("mini", "mini.old"), barn("pi", "pi.old")],
            ..Default::default()
        };
        let there = Snapshot {
            this_barn: Some("pi".into()),
            barns: vec![barn("imac", "imac.new"), barn("mini", "mini.new"), barn("pi", "pi.new")],
            ..Default::default()
        };
        assert_eq!(
            keys(&diff(&here, &there).unwrap()),
            vec![(Kind::Barn, "mini".into(), Status::Differs(Newer::Unknown))]
        );
        // Only on one side, too: still never offered.
        let there = Snapshot { this_barn: Some("pi".into()), ..Default::default() };
        assert_eq!(
            keys(&diff(&here, &there).unwrap()),
            vec![(Kind::Barn, "mini".into(), Status::OnlyHere)]
        );
    }

    #[test]
    fn diff_refuses_when_either_side_has_no_barn_name() {
        let unbranded = Snapshot { projects: vec![project("a", "/a", "x")], ..Default::default() };
        let err = diff(&unbranded, &branded("pi")).unwrap_err().to_string();
        assert_eq!(err, "this machine has no barn name — run `yeehaw brand <name>`");
        let err = diff(&branded("imac"), &unbranded).unwrap_err().to_string();
        assert!(err.contains("has no barn name — run `yeehaw brand <name>` on it"), "{err}");
    }

    #[test]
    fn fetch_refuses_a_barn_with_no_barn_name_naming_it() {
        let fake = FakeRemote::with_dump("pi", &Snapshot::default());
        let err = fetch(&fake, &barn("pi", "pi")).unwrap_err().to_string();
        assert_eq!(err, "barn pi has no barn name — run `yeehaw brand <name>` on it");
    }

    // ------------------------------------------------------------------- put

    fn json<T: Serialize>(t: &T) -> String {
        serde_json::to_string(t).unwrap()
    }

    fn load_project(name: &str) -> Project {
        config::load_projects().into_iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn put_of_a_new_project_drops_the_senders_path() {
        let _ranch = branded_ranch("imac");
        put(Kind::Project, &json(&project("api", "/Users/other/api", "hi"))).unwrap();
        let saved = load_project("api");
        assert_eq!(saved.path, "");
        assert_eq!(saved.summary.as_deref(), Some("hi"));
    }

    #[test]
    fn put_over_an_existing_project_keeps_the_local_path() {
        let _ranch = branded_ranch("imac");
        config::save_project(&mut project("api", "/home/me/api", "old")).unwrap();
        put(Kind::Project, &json(&project("api", "/Users/other/api", "new"))).unwrap();
        let saved = load_project("api");
        assert_eq!(saved.path, "/home/me/api");
        assert_eq!(saved.summary.as_deref(), Some("new"));
    }

    #[test]
    fn put_over_an_existing_barn_keeps_the_local_connectable_verdict() {
        let _ranch = branded_ranch("imac");
        let mut mine = barn("pi", "pi.old");
        mine.connectable = Some(false);
        config::save_barn(&mut mine).unwrap();
        let mut theirs = barn("pi", "pi.new");
        theirs.connectable = Some(true);
        put(Kind::Barn, &json(&theirs)).unwrap();
        let saved = config::load_barns().into_iter().find(|b| b.name == "pi").unwrap();
        assert_eq!(saved.connectable, Some(false));
        assert_eq!(saved.host.as_deref(), Some("pi.new"));
    }

    #[test]
    fn put_of_a_ranchhand_keeps_local_config_and_last_sync_or_drops_the_senders() {
        let _ranch = branded_ranch("imac");
        let theirs = ranchhand("k8s", "{kubeconfig: /theirs}", Some("2026-01-01T00:00:00Z"));

        put(Kind::Ranchhand, &json(&theirs)).unwrap();
        let saved = config::load_ranchhands().into_iter().find(|r| r.name == "k8s").unwrap();
        assert!(saved.config.is_null(), "sender's config leaked: {:?}", saved.config);
        assert_eq!(saved.last_sync, None);

        let mut mine = ranchhand("k8s", "{kubeconfig: /mine}", Some("2026-05-05T00:00:00Z"));
        config::save_ranchhand(&mut mine).unwrap();
        put(Kind::Ranchhand, &json(&theirs)).unwrap();
        let saved = config::load_ranchhands().into_iter().find(|r| r.name == "k8s").unwrap();
        assert_eq!(saved.config.get("kubeconfig").and_then(|v| v.as_str()), Some("/mine"));
        assert_eq!(saved.last_sync.as_deref(), Some("2026-05-05T00:00:00Z"));
    }

    #[test]
    fn put_refuses_names_that_are_not_safe_filenames_or_are_local() {
        let ranch = branded_ranch("imac");
        let evil = ranchhand("../../evil", "{}", None);
        assert!(put(Kind::Ranchhand, &json(&evil)).is_err());
        assert!(!ranch.dir.path().join("evil.yaml").exists());
        assert!(put(Kind::Barn, &json(&barn("local", "x"))).is_err());
        assert!(!config::barns_dir().join("local.yaml").exists());
    }

    /// The receiver's own barn record is the hostless one adoption wrote;
    /// a peer's copy of it would describe this machine as seen from there.
    #[test]
    fn put_refuses_the_receivers_own_barn_record() {
        let _ranch = branded_ranch("imac");
        let mut mine = Barn { name: "imac".into(), source: Some("self".into()), ..Default::default() };
        config::save_barn(&mut mine).unwrap();
        let err = put(Kind::Barn, &json(&barn("imac", "imac.ts.net"))).unwrap_err().to_string();
        assert!(err.contains("this machine's own barn record"), "{err}");
        let saved = config::load_barns().into_iter().find(|b| b.name == "imac").unwrap();
        assert_eq!(saved.host, None, "the peer's record of this machine overwrote its own");
    }

    #[test]
    fn put_refuses_when_this_machine_has_no_barn_name() {
        let _ranch = testing::temp_ranch();
        let err = put(Kind::Project, &json(&project("api", "/p", "s"))).unwrap_err().to_string();
        assert_eq!(err, "this machine has no barn name — run `yeehaw brand <name>`");
        assert!(config::load_projects().is_empty());
    }

    #[test]
    fn put_of_a_worm_keeps_the_local_working_dir_or_drops_the_senders() {
        let _ranch = branded_ranch("imac");
        let mut theirs = worm("w");
        theirs.working_dir = Some("/home/pi/api".into());

        put(Kind::Worm, &json(&theirs)).unwrap();
        let saved = config::load_worms().into_iter().find(|w| w.name == "w").unwrap();
        assert_eq!(saved.working_dir, None, "the sender's working_dir came along");

        let mut mine = worm("w");
        mine.working_dir = Some("/Users/cam/api".into());
        config::save_worm(&mut mine).unwrap();
        put(Kind::Worm, &json(&theirs)).unwrap();
        let saved = config::load_worms().into_iter().find(|w| w.name == "w").unwrap();
        assert_eq!(saved.working_dir.as_deref(), Some("/Users/cam/api"));
    }

    /// Every other worm save path rewrites the crontab; a received worm that is
    /// not scheduled would silently never run.
    #[test]
    fn put_of_a_worm_reinstalls_the_crontab() {
        let _ranch = branded_ranch("imac");
        put(Kind::Worm, &json(&worm("nightly"))).unwrap();
        let written = crate::crontab::sys::written_crontabs();
        assert_eq!(written.len(), 1, "crontab not rewritten after a worm put");
        assert!(written[0].contains("worm exec nightly"), "{}", written[0]);
    }

    // ------------------------------------------------------------------ dump

    #[test]
    fn dump_skips_the_local_barn_file_and_names_local_livestock() {
        let _ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        let mut cfg = config::load_config();
        cfg.this_barn = Some("imac".into());
        config::save_config(&cfg).unwrap();
        fs::write(config::barns_dir().join("local.yaml"), "{{{ not yaml").unwrap();
        config::save_barn(&mut barn("pi", "pi.ts.net")).unwrap();
        config::save_project(&mut project_with_livestock("api", &[Some("local")])).unwrap();
        config::save_worm(&mut worm("w")).unwrap();
        config::save_trail(&mut trail("t")).unwrap();
        config::save_ranchhand(&mut ranchhand("r", "{}", None)).unwrap();

        let dumped: Snapshot = serde_json::from_str(&dump_json().unwrap()).unwrap();
        assert_eq!(dumped.this_barn.as_deref(), Some("imac"));
        assert_eq!(dumped.barns.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["pi"]);
        assert_eq!(dumped.projects[0].livestock[0].barn.as_deref(), Some("imac"));
        assert_eq!(dumped.worms.len(), 1);
        assert_eq!(dumped.trails.len(), 1);
        assert_eq!(dumped.ranchhands.len(), 1);
    }

    // -------------------------------------------------------------- transfer

    #[test]
    fn fetch_parses_a_dump_even_after_login_shell_noise() {
        let snap = Snapshot { projects: vec![project("api", "/p", "s")], ..branded("pi") };
        let fake = FakeRemote::default();
        fake.set_dump("pi", Ok(format!("Welcome to the pi!\n{}\n", serde_json::to_string(&snap).unwrap())));
        let got = fetch(&fake, &barn("pi", "pi")).unwrap();
        assert_eq!(got.projects[0].name, "api");
        assert_eq!(fake.calls()[0].1, "bash -lc 'yeehaw sync dump'");
    }

    #[test]
    fn fetch_from_a_yeehaw_without_sync_says_to_update_it() {
        let fake = FakeRemote::default();
        fake.set_dump("pi", Ok("Update available: 0.1.17\n".into()));
        let err = fetch(&fake, &barn("pi", "pi")).unwrap_err().to_string();
        assert!(err.contains("barn pi's yeehaw doesn't understand `sync`"), "{err}");
    }

    #[test]
    fn remote_failures_read_as_sentences() {
        let missing = explain_failure("pi", Some(127), "bash: yeehaw: command not found");
        assert!(missing.contains("yeehaw isn't installed on barn pi"), "{missing}");

        let old = explain_failure("pi", Some(1), "open terminal failed: not a terminal");
        assert!(old.contains("barn pi's yeehaw doesn't understand `sync`"), "{old}");

        let unreachable = explain_failure("pi", Some(255), "ssh: connect to host pi port 22: Connection refused");
        assert!(unreachable.starts_with("couldn't reach barn pi: "), "{unreachable}");
        assert!(unreachable.contains("Connection refused"), "{unreachable}");

        let other = explain_failure("pi", Some(1), "Error: disk full");
        assert_eq!(other, "barn pi: Error: disk full");

        let silent = explain_failure("pi", Some(3), "  \n");
        assert_eq!(silent, "barn pi: yeehaw sync failed (exit 3)");
    }

    /// A remote that exits without reading its stdin breaks the pipe. The
    /// sentence the user sees must come from its stderr, not from EPIPE.
    #[test]
    fn a_remote_that_exits_before_reading_stdin_reports_its_stderr() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "echo 'Error: refused' >&2; exit 1"]);
        // Far more than a pipe buffer, so the write cannot complete before
        // the child has exited.
        let big = "x".repeat(4 * 1024 * 1024);
        let err = run_command(cmd, "pi", Some(&big)).unwrap_err().to_string();
        assert_eq!(err, "barn pi: Error: refused");
    }

    /// A row built without a branded `here` would carry livestock saying
    /// `local` (or nothing), which on the barn means the barn. Never sent.
    #[test]
    fn push_refuses_livestock_that_still_say_this_machine() {
        let row = Row {
            kind: Kind::Project,
            name: "api".into(),
            status: Status::OnlyHere,
            here: Some(serde_json::to_value(project_with_livestock("api", &[Some("local"), None])).unwrap()),
            there: None,
        };
        let fake = FakeRemote::default();
        let err = push(&fake, &barn("pi", "pi"), &row).unwrap_err().to_string();
        assert_eq!(err, UNBRANDED_HERE);
        assert!(fake.calls().is_empty(), "sent anyway");
    }

    #[test]
    fn push_sends_the_entity_as_json_to_sync_put() {
        let here = Snapshot {
            this_barn: Some("imac".into()),
            projects: vec![project_with_livestock("api", &[Some("local")])],
            ..Default::default()
        };
        let rows = diff(&here, &branded("pi")).unwrap();
        let fake = FakeRemote::default();
        push(&fake, &barn("pi", "pi"), &rows[0]).unwrap();

        let (barn_name, cmd, stdin) = fake.calls().pop().unwrap();
        assert_eq!(barn_name, "pi");
        assert_eq!(cmd, "bash -lc 'yeehaw sync put project'");
        let sent: Project = serde_json::from_str(&stdin.expect("entity on stdin")).unwrap();
        assert_eq!(sent.name, "api");
        assert_eq!(sent.livestock[0].barn.as_deref(), Some("imac"));
    }

    #[test]
    fn pull_saves_the_barns_copy_keeping_the_local_path() {
        let _ranch = branded_ranch("imac");
        config::save_project(&mut project("api", "/home/me/api", "old")).unwrap();
        let there = Snapshot { projects: vec![project("api", "/Users/them/api", "new")], ..branded("pi") };
        let rows = diff(&local_snapshot(), &there).unwrap();
        pull(&rows[0]).unwrap();
        let saved = load_project("api");
        assert_eq!(saved.path, "/home/me/api");
        assert_eq!(saved.summary.as_deref(), Some("new"));
        // And now the two agree.
        assert!(diff(&local_snapshot(), &there).unwrap().is_empty());
    }
}
