//! One-time migrations of the on-disk config store, and the contents of the
//! record they mint: this machine's own barn.

use std::fs;
use std::process::Command;

use anyhow::{Context, Result};

use crate::config;
use crate::types::*;

// ============================================================================
// What this machine advertises about itself
// ============================================================================

/// One word of `hostname`'s output, lowercased, or `None` when it says nothing
/// usable.
///
/// Shelled out the same way this codebase shells out to `ssh`, `kubectl` and
/// `crontab` — there is no hostname in `std` and no crate for it in this tree.
/// Lowercased for the same reason `ranch::this_machine_default_name` lowercases:
/// `Cams-iMac.local` is not what anybody types, and DNS does not care.
///
/// `localhost` is not an answer. It names the machine asking, which is the one
/// machine that never needs this list.
fn hostname(flag: &str) -> Option<String> {
    let out = Command::new("hostname").arg(flag).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    if name.is_empty() || name == "localhost" || name.starts_with("localhost.") {
        return None;
    }
    Some(name)
}

/// Where this machine believes it can be reached, best effort, freshest first.
///
/// # Why a machine has to volunteer this at all
///
/// Nobody else can work it out. This machine's own barn record carries no `host`
/// — from here there is nothing to dial — and that record is exactly what every
/// peer receives on sync. Unless it says how to be reached, it arrives somewhere
/// else as a barn with no address at all, which is the bug: `yeehaw connect
/// smashed-air` from the iMac, and the same in reverse, both refused a machine
/// the join had reached over ssh minutes earlier.
///
/// # What is on the list, and what is deliberately not
///
/// **`<hostname>.local`.** mDNS answers it on the same network with no DNS
/// server, no DHCP reservation and no configuration — it is what the user's iMac
/// answered to when the join was typed by hand. It survives a new lease, which
/// is the property that matters here.
///
/// **Not the LAN IPs.** They were considered and rejected, on the strength of
/// what the list *is*: `merge::merge_addresses` unions it and honours no
/// removals, so an address that reaches a peer can never be taken off again. A
/// DHCP lease makes `192.168.1.numbers` wrong within the week and then wrong
/// forever, on every machine on the ranch — and since `ssh::dial_host` dials the
/// head of the list, a stale entry that reached the head costs a 10-second
/// `ConnectTimeout` on every connect. A *name* has no such failure mode: it is
/// re-resolved on each dial, so it follows the machine across leases and
/// networks. The union's own safety argument in `canonical.rs` ("an address one
/// machine can use and another cannot costs a connect timeout, not a lost host")
/// is an argument for tolerating a bad entry, not for minting ones that are
/// known to go bad.
///
/// **Not `hostname -f`.** It performs a reverse lookup, which blocks for seconds
/// on a machine with no working resolver — and this runs at the head of `ranch
/// serve`, where the peer is waiting on a greeting.
///
/// An address the *user* demonstrably reached this ranch at is a different
/// matter: that one is known good rather than inferred, and `ranch::join_with`
/// records it on the peer's record for exactly that reason.
pub fn this_machine_addresses() -> Vec<String> {
    let mut out = Vec::new();
    if let Some(short) = hostname("-s") {
        out.push(format!("{}.local", short));
    }
    out
}

/// The user a peer should ssh to this machine as.
///
/// `ssh::ssh_args` falls back to `root` for a barn with no user, and macOS
/// refuses `root@` outright — so a Mac that advertises an address and no user is
/// still unreachable. `whoami` rather than `$USER`: the environment is not set
/// for a process launched by sshd or launchd, and this is read on both paths.
pub fn this_machine_user() -> Option<String> {
    let out = Command::new("whoami").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let user = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if user.is_empty() {
        None
    } else {
        Some(user)
    }
}

/// Folds what this machine knows about itself into its own barn record, and
/// reports whether anything actually changed.
///
/// **The return value is not a courtesy.** `config::save_barn` stamps
/// `updated_at`, and a stamped entity is one the next sync offers; re-writing an
/// unchanged record on every `ranch serve` would make this machine's own barn
/// read as modified forever. Callers save only when this says `true` — the same
/// rule `adopt_this_machine` already applies to projects it did not touch.
///
/// **`host` is never written.** That absence is load-bearing: it is how
/// `adopt_this_machine` tells this machine's own record from a real remote one,
/// what `connect` and `ranch::resolve_target` read as "nothing to dial", and the
/// slot a human's own answer in the barn form occupies. Reachability this
/// machine *inferred* about itself goes to `addresses`, which is a union no
/// machine has to arbitrate.
///
/// **`user` is filled only when absent**, because a user typed into the barn
/// form is an answer somebody gave on purpose.
pub fn advertise_this_machine(barn: &mut Barn) -> bool {
    let mut changed = false;

    if barn.user.is_none() {
        if let Some(user) = this_machine_user() {
            barn.user = Some(user);
            changed = true;
        }
    }

    // Current candidates first, then everything already on the record that is
    // not one of them. Two properties at once: the freshest answer is what
    // `ssh::dial_host` reaches for, and nothing is ever removed — which is what
    // `merge::merge_addresses` does on the wire, so the local list and the merged
    // one agree about order instead of flapping.
    let mut next = this_machine_addresses();
    for existing in &barn.addresses {
        if !next.contains(existing) {
            next.push(existing.clone());
        }
    }
    if next != barn.addresses {
        barn.addresses = next;
        changed = true;
    }

    changed
}

/// Applies [`advertise_this_machine`] to this machine's own barn record on disk.
///
/// The repair path, and the reason there is deliberately no startup sweep over
/// the store: a self-barn written before this existed is fixed the next time the
/// machine adopts ([`adopt_this_machine`]) or syncs (`ranch::serve_session` and
/// the end of `ranch::join_with`) — on the one record it owns, leaving every
/// other machine's record alone.
///
/// A machine that has never been adopted has no record to write to, and
/// inventing one would plant a barn the user never named. `Ok(false)`, the same
/// refusal `ranch::record_our_brand` makes for the same reason.
///
/// No sync base is recorded, matching `record_our_brand`: this is a local edit
/// that has not been sent anywhere, and a base claiming otherwise would have the
/// next sync read it as already-synced and never offer it.
pub fn advertise_self_barn() -> Result<bool> {
    let Some(name) = config::this_barn_name() else {
        return Ok(false);
    };
    advertise_barn_named(&name)
}

/// [`advertise_self_barn`] against a name the caller already knows.
///
/// Split out for [`adopt_this_machine`], which repairs the record it just found
/// *before* `this_barn` has been written — at that point this machine's own name
/// is the argument the user passed, not something the config can be asked for.
fn advertise_barn_named(name: &str) -> Result<bool> {
    // Held across the read and the write. `save_barn` takes no lock of its own,
    // so a concurrent writer — the TUI's barn form, `record_our_brand` — read
    // either side of this would be silently discarded.
    let _guard = crate::store::lock_entity(&config::barns_dir(), name)?;

    let path = config::barns_dir().join(format!("{}.yaml", name));
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        // `this_barn` names a record that is not there. Not an error worth
        // failing a sync over, and not something to recreate from nothing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    let mut barn: Barn = serde_yaml::from_str(&content)
        .with_context(|| format!("barn file {} does not parse", path.display()))?;

    if !advertise_this_machine(&mut barn) {
        return Ok(false);
    }
    config::save_barn(&mut barn)?;
    Ok(true)
}

/// What [`adopt_this_machine`] actually changed.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptionReport {
    pub barn_created: bool,
    pub livestock_reassigned: usize,
    pub projects_touched: Vec<String>,
}

/// Turns this machine into a real barn named `name`.
///
/// Creates the barn record if absent, records the name in config, and rewrites
/// every `livestock.barn: None` to `name` — because `None` means "the machine
/// reading this file", which stops being true the moment the file syncs.
///
/// Idempotent: running it twice under the same name is a no-op the second time.
///
/// **Opt-in.** Nothing calls this in Phase 1; Phase 2's `ranch init` will.
///
/// # Refusals
///
/// Every check below runs before the first byte is written, because a
/// half-migrated ranch that reports success is worse than a refusal the user
/// can act on. This bails, writing nothing, when:
///
/// - `name` is the reserved `local`;
/// - the ranch is already adopted under a *different* name;
/// - any project file failed to parse;
/// - a barn already named `name` turns out to be another machine.
pub fn adopt_this_machine(name: &str) -> Result<AdoptionReport> {
    // First, before anything is written. `local` is synthetic — injected into
    // every listing, never persisted, and dropped by `load_barns_checked` if a
    // file ever claims the name — so adopting it would write a barn nothing can
    // display and leave every livestock pointing at a name that still means
    // "whichever machine reads this".
    if name == config::LOCAL_BARN_NAME {
        anyhow::bail!("'{}' is reserved; choose a real name for this machine", name);
    }

    // A rename-adoption is refused, not performed. After `adopt_this_machine
    // ("imac")` every local livestock says `Some("imac")` — neither `None` nor
    // `"local"` — so a second run under a new name reassigns *nothing*, sets
    // `this_barn` to the new name, and every one of those livestock silently
    // vanishes from the local barn view: they name a barn this machine is no
    // longer, and `get_livestock_for_barn` has no alias to follow.
    //
    // Rewriting `Some(old)` as well is not the fix either, because that is only
    // half of a rename. The barn *record* still lives at `barns/imac.yaml`, and
    // moving it has to preserve the uuid — that surviving uuid is the entire
    // signal that tells a peer this was a rename rather than a delete plus a
    // create (see `config::rename_project`). Doing the livestock half here and
    // leaving the barn half undone strands every livestock on a barn name with
    // no record behind it. That is a `rename_barn` operation spanning the barn
    // file and every project at once, not a one-time migration, so this refuses
    // and says so rather than doing half of it.
    if let Some(current) = config::load_config().this_barn.as_deref() {
        if current != name {
            anyhow::bail!(
                "this ranch is already adopted as '{current}'; renaming a barn has to move \
                 barns/{current}.yaml and every livestock that names it in one step, which \
                 this migration does not do. Re-run with '{current}', or rename the barn first"
            );
        }
    }

    // The lenient `load_projects()` would drop an unparseable project on the
    // floor: the migration would never see it, never rewrite its `barn: None`,
    // and report nothing — leaving that livestock machine-relative forever while
    // `this_barn` claims the ranch is migrated. Read the checked result instead,
    // and refuse before writing anything. The paths go in the error because that
    // is the only place the user can act on them; there is deliberately no
    // `unparsed` field on `AdoptionReport`, since a report is only ever returned
    // from a run that had none.
    let listing = config::load_projects_checked();
    if !listing.errors.is_empty() {
        let detail = listing
            .errors
            .iter()
            .map(|e| format!("  {}: {}", e.path.display(), e.message))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "refusing to adopt this machine: {} project file(s) could not be read, and a \
             migration that skips them leaves their livestock machine-relative forever while \
             this ranch claims to be migrated. Fix or remove these, then re-run:\n{}",
            listing.errors.len(),
            detail
        );
    }

    let mut report = AdoptionReport {
        barn_created: false,
        livestock_reassigned: 0,
        projects_touched: vec![],
    };

    let existing = config::load_barns()
        .into_iter()
        .find(|b| b.name == name && !config::is_local_barn(b));
    match existing {
        // "A barn file with this name exists" is not the same as "this machine
        // is already adopted". If the user types `pi` and `barns/pi.yaml` is a
        // real Raspberry Pi, the old check saw the file, created nothing, set
        // `this_barn = "pi"`, and reassigned every local livestock onto the Pi —
        // after which the ranch asserts that this laptop's deployments run
        // there. Refuse instead.
        //
        // The barn record this migration writes for the local machine has no
        // `host`, no `connection_type` and no `connection_config`: the process
        // is already on that machine, so there is nothing to dial. Anything
        // carrying a way to *reach* it — a real Pi, a kubectl node — is some
        // other machine. (Inlined rather than a named helper: nothing outside
        // this module calls the migration in Phase 1, so a helper would be one
        // more dead-code warning.)
        Some(barn)
            if barn.host.is_some()
                || barn.connection_type.is_some()
                || barn.connection_config.is_some() =>
        {
            anyhow::bail!(
                "a barn named '{}' already exists and is another machine ({}); \
                 adopting its name would move every livestock on this machine onto it. \
                 Choose a different name for this machine",
                name,
                barn.host.clone().unwrap_or_else(|| "remote".into())
            );
        }
        // Already this machine's own record: an earlier adoption under the same
        // name, or a hand-written hostless barn. Nothing to create — but the
        // record may well predate the advertisement, which is exactly the state
        // the live ranch is in: a self-barn with `host: null`, `user: null` and
        // no addresses, unreachable from every other machine on the ranch. This
        // is one of the two places that repairs it (the other is a sync), and it
        // is why there is no silent sweep on startup: the user re-runs the
        // adoption, or syncs, and gets their own record fixed — nobody else's.
        //
        // Not fatal. A machine that cannot work out its own hostname is still
        // correctly adopted; it is only harder for a peer to reach.
        Some(_) => {
            let _ = advertise_barn_named(name);
        }
        None => {
            let mut barn = Barn {
                name: name.to_string(),
                // No host, user or port: this record names the machine the
                // process is running on, and there is nothing to dial.
                host: None,
                user: None,
                port: None,
                identity_file: None,
                critters: vec![],
                source: Some("self".into()),
                connection_type: None,
                connection_config: None,
                // You cannot ssh to yourself. `connect.rs` and the TUI both
                // refuse a barn marked `Some(false)` with "not connectable over
                // SSH" instead of spawning an ssh window at a barn with no host
                // to dial. `None` would read as "unknown, try it".
                connectable: Some(false),
                ..Default::default()
            };
            // How every *other* machine reaches this one. `host` stays `None`
            // above because there is nothing to dial from here — but this record
            // is precisely what syncs to every peer, and to them it is remote.
            // Without this it arrives as a barn with no address at all, which is
            // why `yeehaw connect smashed-air` from the iMac answered "has no
            // host configured" for a machine the join had reached over ssh
            // minutes earlier. See `advertise_this_machine`.
            advertise_this_machine(&mut barn);
            // `create_barn`, never `save_barn`: a plain save writes over
            // whatever is at `barns/<name>.yaml`, so a stale existence check
            // would replace a real barn's host, user and critters with this
            // empty record *and* mint a new uuid over its old one — to a peer, a
            // different entity wearing the same name. `create_barn` holds the
            // entity lock across its own check and write, so a concurrent
            // adoption fails loudly rather than clobbering.
            config::create_barn(&mut barn)?;
            report.barn_created = true;
        }
    }

    // Before the livestock rewrite, not after. Between the two steps `local`
    // already resolves to `name`, and `get_livestock_for_barn` treats a `None`
    // barn as this machine's whenever the target *is* this machine — so a run
    // interrupted here leaves every livestock still visible under both names.
    // The other order hides the already-rewritten ones from `local` until the
    // config write lands.
    //
    // Re-read rather than reusing the copy from the rename check above: the
    // config file has other writers (the TUI, the mcp-server) and a barn has been
    // created since.
    let mut cfg = config::load_config();
    cfg.this_barn = Some(name.to_string());
    config::save_config(&cfg)?;

    for listed in &listing.items {
        // Locked, and re-read from disk inside the lock — never the copy from
        // the listing above. `save_project` takes no lock of its own, so a
        // concurrent `add_livestock_to_project` (which does) can land between
        // the listing and this write and be silently discarded. That loss is
        // undetectable after the fact: `save_project` stamps, so the surviving
        // file carries the *newest* `updated_at` and the correct uuid, and no
        // timestamp-ordered merge can tell it lost anything. The migration is
        // not latency-sensitive; a lock per project costs nothing that matters.
        let _guard = crate::store::lock_entity(&config::projects_dir(), &listed.name)?;

        let path = config::projects_dir().join(format!("{}.yaml", listed.name));
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            // Deleted between the listing and the lock. There is nothing left to
            // migrate, and recreating it from the stale copy would resurrect an
            // entity the user just deleted.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("failed to re-read project '{}'", listed.name))
            }
        };
        // It parsed during the pre-flight, so a failure here is a change made
        // since. Loud, not skipped — skipping is the defect this whole path
        // exists to remove.
        let mut project: Project = serde_yaml::from_str(&content)
            .with_context(|| format!("project '{}' stopped parsing mid-migration", listed.name))?;

        let mut changed = false;
        for ls in project.livestock.iter_mut() {
            // Both machine-relative spellings, and only those. `None` is the
            // default, and `Some("local")` is what the UI's barn picker writes
            // when the user chooses the local barn out of the list — the same
            // "whichever machine reads this" with a different spelling, and the
            // same defect the moment the file syncs. A livestock pinned to any
            // other barn already names the machine it runs on.
            let machine_relative = matches!(
                ls.barn.as_deref(),
                None | Some(config::LOCAL_BARN_NAME)
            );
            if machine_relative {
                ls.barn = Some(name.to_string());
                report.livestock_reassigned += 1;
                changed = true;
            }
        }
        // Only the projects that actually changed. `save_project` stamps
        // `updated_at`, so rewriting an untouched project publishes an edit that
        // never happened — and under last-write-wins that stamp beats a real
        // change made on another machine.
        if changed {
            report.projects_touched.push(project.name.clone());
            config::save_project(&mut project)?;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn livestock(name: &str, barn: Option<&str>) -> Livestock {
        Livestock {
            name: name.into(),
            path: format!("/tmp/{name}"),
            barn: barn.map(str::to_string),
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    fn project(name: &str, livestock: Vec<Livestock>) -> Project {
        Project {
            name: name.into(),
            path: "/tmp".into(),
            summary: None,
            color: None,
            gradient_spread: None,
            gradient_inverted: None,
            livestock,
            herds: vec![],
            wiki: vec![],
            issue_provider: None,
            wiki_provider: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    /// One livestock on this machine, one deliberately pinned to another barn.
    fn project_with_local_livestock(name: &str) -> Project {
        project(
            name,
            vec![livestock("web", None), livestock("worker", Some("pi"))],
        )
    }

    /// `barn: None` means "whichever machine is reading this file", which stops
    /// being true the instant the file syncs. Adoption resolves that to a real
    /// name — but only for the livestock that actually meant *this* machine.
    #[test]
    fn adopts_machine_and_reassigns_only_local_livestock() {
        crate::testing::with_temp_ranch(|_| {
            let mut p = project_with_local_livestock("api");
            config::save_project(&mut p).unwrap();

            let report = adopt_this_machine("imac").unwrap();
            assert!(report.barn_created);
            assert_eq!(report.livestock_reassigned, 1);
            assert_eq!(report.projects_touched, vec!["api".to_string()]);

            let loaded = config::load_projects();
            let web = loaded[0].livestock.iter().find(|l| l.name == "web").unwrap();
            let worker = loaded[0].livestock.iter().find(|l| l.name == "worker").unwrap();
            assert_eq!(web.barn.as_deref(), Some("imac"));
            assert_eq!(worker.barn.as_deref(), Some("pi"), "must not touch other barns");

            assert_eq!(config::load_config().this_barn.as_deref(), Some("imac"));
        });
    }

    /// `barn: local` is what the UI's barn picker writes when the user chooses
    /// the local barn from the list — it is spelled differently from `None` and
    /// means exactly the same machine-relative thing, so it carries exactly the
    /// same sync defect. Found in the real ranch during the dry run.
    #[test]
    fn also_reassigns_livestock_pinned_to_the_synthetic_local_barn() {
        crate::testing::with_temp_ranch(|_| {
            let mut p = project(
                "api",
                vec![
                    livestock("web", None),
                    livestock("ios", Some(config::LOCAL_BARN_NAME)),
                    livestock("worker", Some("pi")),
                ],
            );
            config::save_project(&mut p).unwrap();

            let report = adopt_this_machine("imac").unwrap();
            assert_eq!(report.livestock_reassigned, 2);

            let loaded = config::load_projects();
            let by = |n: &str| {
                loaded[0]
                    .livestock
                    .iter()
                    .find(|l| l.name == n)
                    .unwrap()
                    .barn
                    .clone()
            };
            assert_eq!(by("web").as_deref(), Some("imac"));
            assert_eq!(
                by("ios").as_deref(),
                Some("imac"),
                "'local' names no machine once this file leaves this machine"
            );
            assert_eq!(by("worker").as_deref(), Some("pi"), "must not touch other barns");
        });
    }

    /// The migration is opt-in and hand-run, so it will be run twice. A second
    /// run has to be a no-op *and say so*: a report claiming it created the
    /// barn again is how a caller learns it clobbered the real record.
    #[test]
    fn is_idempotent() {
        crate::testing::with_temp_ranch(|_| {
            let mut p = project_with_local_livestock("api");
            config::save_project(&mut p).unwrap();

            adopt_this_machine("imac").unwrap();
            let second = adopt_this_machine("imac").unwrap();

            assert!(!second.barn_created);
            assert_eq!(second.livestock_reassigned, 0);
            assert!(second.projects_touched.is_empty());
        });
    }

    /// `local` is synthetic: it is injected into every listing, never persisted,
    /// and `load_barns_checked` drops any file that claims the name. Adopting it
    /// would write a barn nothing can ever show and leave every livestock
    /// pointing at a name that still means "whichever machine reads this".
    #[test]
    fn rejects_the_reserved_local_name() {
        crate::testing::with_temp_ranch(|_| {
            assert!(adopt_this_machine("local").is_err());
        });
    }

    /// `save_project` stamps `updated_at`, so writing a project the migration
    /// did not actually change publishes a modification that never happened —
    /// and under last-write-wins that stamp beats a real edit made elsewhere.
    #[test]
    fn a_project_with_nothing_on_this_machine_is_not_rewritten() {
        crate::testing::with_temp_ranch(|_| {
            let mut elsewhere = project("edge", vec![livestock("cdn", Some("pi"))]);
            config::save_project(&mut elsewhere).unwrap();
            let before = config::load_projects()
                .into_iter()
                .find(|p| p.name == "edge")
                .unwrap();

            let report = adopt_this_machine("imac").unwrap();

            assert!(
                !report.projects_touched.contains(&"edge".to_string()),
                "nothing in 'edge' belongs to this machine"
            );
            let after = config::load_projects()
                .into_iter()
                .find(|p| p.name == "edge")
                .unwrap();
            assert_eq!(
                after.updated_at, before.updated_at,
                "an untouched project must not be restamped"
            );
        });
    }

    // === what this machine advertises about itself =========================
    //
    // THE BUG these cover: `adopt_this_machine` writes this machine's own record
    // with `host: None` and `user: None`, because from here there is nothing to
    // dial. That record is exactly what every other machine receives on sync,
    // and to them it is remote and unreachable — `yeehaw connect smashed-air`
    // from the iMac answered "barn 'smashed-air' has no host configured", and the
    // same in reverse, even though the join had just reached both over ssh.
    //
    // Nobody else can know how to reach this machine, so it has to volunteer
    // candidates.

    /// `hostname -s` here, so the expectations below are about *this* machine
    /// rather than a name baked into the test.
    fn short_hostname() -> Option<String> {
        let out = std::process::Command::new("hostname").arg("-s").output().ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
        if !out.status.success() || name.is_empty() || name == "localhost" {
            None
        } else {
            Some(name)
        }
    }

    #[test]
    fn this_machine_advertises_its_mdns_name() {
        let Some(short) = short_hostname() else {
            // A machine with no usable hostname has nothing to advertise, and
            // the candidate list is empty by design. Nothing to assert.
            return;
        };
        let addresses = this_machine_addresses();
        assert!(
            addresses.contains(&format!("{}.local", short)),
            "`<hostname>.local` resolves over mDNS on the same network with no DNS server and \
             no DHCP reservation — it is what the user's iMac answered to. Got {:?}",
            addresses
        );
    }

    /// Loopback names a machine only to itself, which is the one machine that
    /// never needs the list. Advertised, it would be dialled by a peer and
    /// connect to *that peer*.
    #[test]
    fn this_machine_never_advertises_loopback() {
        for a in this_machine_addresses() {
            assert!(
                !a.starts_with("localhost") && a != "127.0.0.1" && a != "::1",
                "loopback names the dialer, not this machine: {:?}",
                a
            );
        }
    }

    #[test]
    fn this_machine_knows_which_user_to_be_reached_as() {
        // `ssh::ssh_args` falls back to `root` for a barn with no user, and a Mac
        // refuses `root@` outright. The record has to carry the real one.
        let user = this_machine_user().expect("`whoami` names the user running this");
        assert!(!user.trim().is_empty());
        assert_ne!(user, "root", "this suite is not expected to run as root");
    }

    /// The freshest candidate goes to the head of the list, because that is the
    /// one `ssh::dial_host` dials and `merge::merge_addresses` preserves the
    /// order on every peer.
    #[test]
    fn advertising_puts_this_machines_own_candidates_first() {
        let Some(short) = short_hostname() else { return };
        let mut barn = Barn {
            name: "smashed-air".into(),
            addresses: vec!["an-old-name.local".into()],
            ..Default::default()
        };

        assert!(advertise_this_machine(&mut barn), "there was something to add");
        assert_eq!(
            barn.addresses.first().map(String::as_str),
            Some(format!("{}.local", short).as_str()),
            "the current candidate must be dialled first: {:?}",
            barn.addresses
        );
        assert!(
            barn.addresses.iter().any(|a| a == "an-old-name.local"),
            "`merge_addresses` honours no removals, so neither may this: {:?}",
            barn.addresses
        );
    }

    /// `save_barn` stamps `updated_at`, and a stamped entity is one the next sync
    /// offers. Re-advertising the same thing every serve session would make this
    /// machine's own record look changed on every sync, forever.
    #[test]
    fn advertising_twice_changes_nothing_the_second_time() {
        let mut barn = Barn { name: "smashed-air".into(), ..Default::default() };
        advertise_this_machine(&mut barn);
        let after_first = barn.clone();

        assert!(
            !advertise_this_machine(&mut barn),
            "nothing changed, so nothing may be reported as changed"
        );
        assert_eq!(barn.addresses, after_first.addresses);
        assert_eq!(barn.user, after_first.user);
    }

    /// A user the person typed into the TUI's barn form beats `whoami`. They are
    /// the same machine either way, and the record is the place the answer was
    /// deliberately given.
    #[test]
    fn advertising_does_not_overwrite_a_user_already_on_the_record() {
        let mut barn = Barn {
            name: "smashed-air".into(),
            user: Some("deploy".into()),
            ..Default::default()
        };
        advertise_this_machine(&mut barn);
        assert_eq!(barn.user.as_deref(), Some("deploy"));
    }

    /// `host` stays `None`, and that is load-bearing rather than an oversight.
    /// It is what `adopt_this_machine` reads to tell this machine's own record
    /// from a real remote one, what `connect` and the TUI see as "nothing to
    /// dial", and what a human's own answer in the barn form would occupy.
    /// Reachability that this machine *inferred* about itself belongs in
    /// `addresses`, which is a union nobody has to arbitrate.
    #[test]
    fn advertising_never_invents_a_host() {
        let mut barn = Barn { name: "smashed-air".into(), ..Default::default() };
        advertise_this_machine(&mut barn);
        assert_eq!(barn.host, None, "a machine does not ssh to itself");
    }

    /// The repair path, and the reason there is no startup sweep: an existing
    /// self-barn with `host: null` is fixed the next time this machine adopts or
    /// syncs, on the record it already has, without rewriting anything else.
    #[test]
    fn the_self_barn_is_repaired_in_place() {
        crate::testing::with_temp_ranch(|_| {
            let Some(short) = short_hostname() else { return };
            // Exactly what the live ranch holds today: adopted, no host, no user,
            // no addresses.
            let mut stale = Barn {
                name: "smashed-air".into(),
                connectable: Some(false),
                source: Some("self".into()),
                ..Default::default()
            };
            config::save_barn(&mut stale).unwrap();
            let mut cfg = config::load_config();
            cfg.this_barn = Some("smashed-air".into());
            config::save_config(&cfg).unwrap();

            assert!(advertise_self_barn().unwrap(), "there was a repair to make");

            let fixed = config::load_barns()
                .into_iter()
                .find(|b| b.name == "smashed-air")
                .expect("the record survives");
            assert!(
                fixed.addresses.contains(&format!("{}.local", short)),
                "{:?}",
                fixed.addresses
            );
            assert!(fixed.user.is_some(), "a peer dialling this must not fall back to root@");
            assert_eq!(fixed.source.as_deref(), Some("self"), "nothing else may be rewritten");
            assert_eq!(fixed.connectable, Some(false));

            assert!(
                !advertise_self_barn().unwrap(),
                "a second pass has nothing to do, and must not restamp the record"
            );
        });
    }

    /// Before `ranch init` or `ranch join` there is no record to write to, and
    /// inventing one would plant a barn the user never named — the same refusal
    /// `ranch::record_our_brand` makes.
    #[test]
    fn a_machine_that_was_never_adopted_advertises_nothing() {
        crate::testing::with_temp_ranch(|_| {
            assert!(!advertise_self_barn().unwrap());
            assert!(
                config::load_barns().iter().all(config::is_local_barn),
                "no barn may be created by an advertisement"
            );
        });
    }

    /// Adoption is where the record is minted, so it is the first chance to say
    /// how to reach this machine — and the record it mints is precisely the one
    /// that syncs to every other machine.
    #[test]
    fn adoption_mints_a_record_that_says_how_to_reach_this_machine() {
        crate::testing::with_temp_ranch(|_| {
            let Some(short) = short_hostname() else { return };
            adopt_this_machine("imac").unwrap();

            let imac = config::load_barns()
                .into_iter()
                .find(|b| b.name == "imac")
                .expect("the barn was created");
            assert!(
                imac.addresses.contains(&format!("{}.local", short)),
                "a record with no addresses is unreachable from every other machine: {:?}",
                imac
            );
            assert!(imac.user.is_some(), "and `root@` is not the answer on a Mac");
            assert_eq!(imac.host, None, "still nothing to dial from here");
            assert_eq!(imac.connectable, Some(false));
            assert!(
                crate::ssh::dial_host(&imac).is_some(),
                "the whole point: a peer holding this record can build an ssh destination"
            );
        });
    }

    /// The live ranch's self-barns predate the advertisement, and a second
    /// adoption is how the user re-runs it. It has to repair them — while still
    /// reporting that it created nothing, which is what `is_idempotent` pins.
    #[test]
    fn re_adopting_repairs_a_self_barn_that_advertises_nothing() {
        crate::testing::with_temp_ranch(|_| {
            let Some(short) = short_hostname() else { return };
            let mut stale = Barn {
                name: "imac".into(),
                connectable: Some(false),
                source: Some("self".into()),
                ..Default::default()
            };
            config::save_barn(&mut stale).unwrap();

            let report = adopt_this_machine("imac").unwrap();
            assert!(!report.barn_created, "the record was already there");

            let imac = config::load_barns().into_iter().find(|b| b.name == "imac").unwrap();
            assert!(
                imac.addresses.contains(&format!("{}.local", short)),
                "an existing self-barn must be repaired, not left unreachable: {:?}",
                imac
            );
        });
    }

    fn remote_barn(name: &str, host: &str) -> Barn {
        Barn {
            name: name.into(),
            host: Some(host.into()),
            user: Some("forge".into()),
            port: Some(22),
            identity_file: None,
            critters: vec![],
            source: None,
            connection_type: None,
            connection_config: None,
            connectable: Some(true),
            ..Default::default()
        }
    }

    /// `load_projects()` drops a project that fails to parse. The migration is
    /// the highest-stakes consumer of that leniency: a dropped project is never
    /// visited, its `barn: None` is never rewritten, and nothing is reported —
    /// so its livestock stays machine-relative forever while `this_barn` claims
    /// the ranch is migrated. Refuse, and name the files.
    #[test]
    fn refuses_to_adopt_when_a_project_file_cannot_be_parsed() {
        crate::testing::with_temp_ranch(|_| {
            let mut good = project_with_local_livestock("api");
            config::save_project(&mut good).unwrap();
            std::fs::write(
                config::projects_dir().join("broken.yaml"),
                "name: broken\nlivestock: [oh no: {{",
            )
            .unwrap();

            let result = adopt_this_machine("imac");
            assert!(
                result.is_err(),
                "an unparseable project must block the migration, not be skipped; got {result:?}"
            );
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("broken.yaml"),
                "the refusal must name the file the user has to fix, got: {err}"
            );

            // Nothing may have been written: the check runs before the first byte.
            assert_eq!(
                config::load_config().this_barn, None,
                "a refused adoption must not record this_barn"
            );
            assert!(
                !config::barns_dir().join("imac.yaml").exists(),
                "a refused adoption must not create the barn"
            );
            let web = config::load_projects()
                .into_iter()
                .find(|p| p.name == "api")
                .unwrap()
                .livestock
                .remove(0);
            assert_eq!(web.barn, None, "a refused adoption must not reassign livestock");
        });
    }

    /// Line ~41 used to treat "a barn file with this name exists" as "already
    /// adopted". Type `pi` when `barns/pi.yaml` is a real Raspberry Pi and the
    /// old code created nothing, set `this_barn = "pi"`, and moved every
    /// livestock on this laptop onto the Pi — after which the ranch asserts the
    /// laptop's deployments run there.
    #[test]
    fn refuses_to_adopt_the_name_of_a_barn_that_is_another_machine() {
        crate::testing::with_temp_ranch(|_| {
            let mut pi = remote_barn("pi", "10.0.0.2");
            config::save_barn(&mut pi).unwrap();
            let mut p = project_with_local_livestock("api");
            config::save_project(&mut p).unwrap();

            let result = adopt_this_machine("pi");
            assert!(
                result.is_err(),
                "adopting the name of another machine must be refused; got {result:?}"
            );
            let err = result.unwrap_err().to_string();
            assert!(err.contains("pi"), "the refusal must name the barn, got: {err}");

            assert_eq!(
                config::load_config().this_barn, None,
                "this machine must not be recorded as the Pi"
            );
            let loaded = config::load_projects().remove(0);
            let by = |n: &str| loaded.livestock.iter().find(|l| l.name == n).unwrap().barn.clone();
            assert_eq!(by("web"), None, "local livestock must not be moved onto the Pi");
            assert_eq!(by("worker").as_deref(), Some("pi"), "the Pi's own livestock is untouched");

            let reloaded = config::load_barns().into_iter().find(|b| b.name == "pi").unwrap();
            assert_eq!(reloaded.host.as_deref(), Some("10.0.0.2"), "the Pi's record must survive");
        });
    }

    /// After adopting as `imac`, every local livestock says `Some("imac")` —
    /// neither `None` nor `"local"` — so adopting again as `macbook` reassigns
    /// nothing, sets `this_barn = "macbook"`, and every one of those livestock
    /// vanishes from the local barn view. Doing the livestock half of a real
    /// rename here would still strand them on a barn whose record is at the old
    /// name, so this refuses rather than half-renaming. See the comment on
    /// `adopt_this_machine`.
    #[test]
    fn refuses_to_re_adopt_this_machine_under_a_different_name() {
        crate::testing::with_temp_ranch(|_| {
            let mut p = project_with_local_livestock("api");
            config::save_project(&mut p).unwrap();
            adopt_this_machine("imac").unwrap();

            let result = adopt_this_machine("macbook");
            assert!(
                result.is_err(),
                "re-adopting under a different name must be refused, not half-performed; got {result:?}"
            );
            let err = result.unwrap_err().to_string();
            assert!(err.contains("imac"), "the refusal must name the current barn, got: {err}");

            assert_eq!(
                config::load_config().this_barn.as_deref(),
                Some("imac"),
                "a refused rename must leave this_barn alone"
            );
            assert!(
                !config::barns_dir().join("macbook.yaml").exists(),
                "a refused rename must not leave a stray barn record behind"
            );
            // The whole point: these must still be findable under the local barn.
            assert_eq!(
                config::get_livestock_for_barn(config::LOCAL_BARN_NAME).len(),
                1,
                "this machine's livestock must not vanish from the local view"
            );
        });
    }

    /// The migration used to read every project up front, mutate the copies in
    /// memory, and write them back through `save_project`, which takes no lock.
    /// A concurrent `add_livestock_to_project` — which does lock — landing in
    /// that window was silently discarded, and undetectably so: `save_project`
    /// stamps, so the surviving file carried the newest `updated_at` and the
    /// right uuid, and no timestamp-ordered merge could ever notice the loss.
    ///
    /// The adder waits until the migration has demonstrably started (the first
    /// project is on disk carrying its new barn) and then writes to the *last*
    /// one, which the unlocked version has already read and not yet written.
    #[test]
    fn a_concurrent_livestock_addition_is_not_lost() {
        // Enough projects that the unlocked version's read-everything-first
        // window is wide: it fsyncs its way through the rest while the adder
        // needs one write.
        const PROJECTS: usize = 60;
        let names: Vec<String> = (0..PROJECTS).map(|i| format!("p{:02}", i)).collect();
        let target = names.last().unwrap().clone();

        let ranch = crate::testing::temp_ranch();
        for n in &names {
            let mut p = project(n, vec![livestock("web", None)]);
            config::save_project(&mut p).unwrap();
        }

        let path = ranch.dir.path().to_path_buf();
        let first = names[0].clone();
        let adder_target = target.clone();
        let adder = std::thread::spawn(move || {
            // A spawned thread does not inherit the thread-local ranch.
            crate::testing::with_ranch_env(path, || {
                let first_path = config::projects_dir().join(format!("{first}.yaml"));
                let started = std::time::Instant::now();
                loop {
                    if std::fs::read_to_string(&first_path)
                        .map(|c| c.contains("barn: imac"))
                        .unwrap_or(false)
                    {
                        break;
                    }
                    assert!(
                        started.elapsed() < std::time::Duration::from_secs(30),
                        "the migration never reached its first project"
                    );
                    std::thread::yield_now();
                }
                config::add_livestock_to_project(&adder_target, &livestock("worker", Some("pi")))
                    .unwrap();
            })
        });

        adopt_this_machine("imac").unwrap();
        adder.join().unwrap();

        let last = config::load_projects()
            .into_iter()
            .find(|p| p.name == target)
            .unwrap();
        assert!(
            last.livestock.iter().any(|l| l.name == "worker"),
            "the concurrent livestock addition was discarded by the migration: {:?}",
            last.livestock.iter().map(|l| &l.name).collect::<Vec<_>>()
        );
        assert_eq!(
            last.livestock.iter().find(|l| l.name == "web").unwrap().barn.as_deref(),
            Some("imac"),
            "the migration still has to do its own job"
        );
    }
}
