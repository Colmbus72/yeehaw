//! One-time migrations of the on-disk config store.

use std::fs;

use anyhow::{Context, Result};

use crate::config;
use crate::types::*;

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
        // name, or a hand-written hostless barn. Nothing to create.
        Some(_) => {}
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
