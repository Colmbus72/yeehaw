//! What this ranch holds, as a peer needs to see it.
//!
//! A sync opens by exchanging manifests: one [`ManifestEntry`] per top-level
//! entity, carrying its uuid, its `updated_at` and the hash of its canonical
//! form. Everything the two sides then decide to ship is decided from these
//! two lists, so a manifest that is wrong in either direction is worse than a
//! failed sync — an entry that should not be there offers a peer a phantom, and
//! an entry that quietly went missing looks like an entity the peer has and we
//! do not.
//!
//! ## The directory is the sync truth
//!
//! `config::load_barns()` and `config::barns_dir()` genuinely disagree about
//! what "the barns" are, and the disagreement is not a bug to fix — it is the
//! difference between what a *user* should see and what a *peer* should be
//! offered. See [`barns_from_disk`].

#![allow(dead_code)] // Wired up in Slice C/E.

use anyhow::{bail, Result};
use serde::Serialize;

use crate::config::{self, LoadError, LoadResult};
use crate::ranch::canonical::hash_entity;
use crate::ranch::wire::ManifestEntry;
use crate::types::{Barn, Identified};

/// Every entity kind a sync moves, in the spelling `tombstones::record` uses.
///
/// Nested entities — `Livestock`, `Critter`, `Herd`, `WikiSection` — are
/// deliberately absent: they carry no uuid and ride inside their parent.
pub const KINDS: [&str; 5] = ["barn", "project", "ranchhand", "trail", "worm"];

/// Every top-level entity on this ranch, ready to hand to a peer.
///
/// Fails rather than shrinks. A file that will not parse is reported, and the
/// whole manifest is refused: a shrunken manifest is indistinguishable from
/// "we do not have that entity", which is how a peer's copy of an entity we
/// merely failed to read gets treated as new and how our unreadable local copy
/// gets overwritten.
pub fn build() -> Result<Vec<ManifestEntry>> {
    // `barns_from_disk` scans `barns_dir()` directly rather than going through a
    // `config` loader, so it is the one enumeration below that does not get its
    // directory created for it. It works today only because the
    // `load_projects_checked()` line happens to come first and calls
    // `ensure_config_dirs()` on the way past — an ordering dependency between
    // two lines that read as independent. Made explicit here so reordering the
    // collects cannot quietly break it.
    config::ensure_config_dirs();

    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut errors: Vec<LoadError> = Vec::new();

    collect("project", config::load_projects_checked(), |p| &p.name, &mut entries, &mut errors)?;
    collect("barn", barns_from_disk(), |b| &b.name, &mut entries, &mut errors)?;
    collect("worm", config::load_worms_checked(), |w| &w.name, &mut entries, &mut errors)?;
    collect("trail", config::load_all_trails_checked(), |t| &t.name, &mut entries, &mut errors)?;
    collect(
        "ranchhand",
        config::load_ranchhands_checked(),
        |r| &r.name,
        &mut entries,
        &mut errors,
    )?;

    // Every bad file at once, not just the first: a user fixing a broken ranch
    // wants the whole list, and finding out about the next one only after
    // repairing this one is the slowest possible way to learn it.
    if !errors.is_empty() {
        let detail = errors
            .iter()
            .map(|e| format!("  {}: {}", e.path.display(), e.message))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "refusing to build a manifest: {} entity file(s) could not be read.\n{}\n\
             Syncing with these missing would look to the peer like they do not exist here.",
            errors.len(),
            detail
        );
    }

    // Not part of the protocol, but a manifest is read by humans in the sync
    // plan pane and diffed against the peer's. Ranch hands come back from their
    // loader unsorted and `HashMap`-backed content makes nothing else stable,
    // so the order is imposed here rather than inherited.
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));

    Ok(entries)
}

fn collect<T: Serialize + Identified>(
    kind: &str,
    loaded: LoadResult<T>,
    name_of: impl Fn(&T) -> &str,
    entries: &mut Vec<ManifestEntry>,
    errors: &mut Vec<LoadError>,
) -> Result<()> {
    errors.extend(loaded.errors);

    for item in &loaded.items {
        entries.push(ManifestEntry {
            kind: kind.to_string(),
            name: name_of(item).to_string(),
            // `None` is the honest answer for most of a real ranch: nothing
            // restamps a file until it is next written. A peer matches those by
            // name on a first join and by uuid ever after — inventing an id
            // here would make an unstamped entity look stamped and take it down
            // the uuid path with a key no peer can have.
            id: item.id().map(|s| s.to_string()),
            updated_at: item.updated_at().map(|s| s.to_string()),
            hash: hash_entity(kind, item)?,
        });
    }

    Ok(())
}

/// Barns, straight off the disk — the one kind that cannot go through its
/// `config` loader.
///
/// `config::load_barns_checked()` does two things a sync must not inherit:
///
/// - It **injects a synthetic `local` barn** at index 0 of every listing. That
///   barn is never persisted, has no uuid, and means "whichever machine is
///   reading this". Offering it to a peer pushes a phantom that the peer would
///   store as a real barn named after nothing.
/// - It **drops any on-disk `local.yaml`**, because two barns spelled `local`
///   would be indistinguishable in the UI.
///
/// A directory scan misses the synthetic barn entirely, which is exactly right.
/// The one thing it must add back is the second rule: a file claiming the
/// synthetic name is invisible to every loader in this codebase, so shipping it
/// to a peer would plant a barn that neither machine can ever display.
///
/// It is added back *broader* than the original, and deliberately so. The loader
/// skips by the `name` inside the parsed file; this skips by the filename stem
/// **and then** by the parsed name. The stem test is the wider of the two — it
/// also catches a `local.yaml` whose contents say something else, which the
/// loader would keep — and it is the only one that can run before the file is
/// opened, which is what the next paragraph is about. The parsed-name pass after
/// it is what actually reproduces the loader's rule.
///
/// That skip happens **before the file is parsed**, and that ordering is not a
/// detail. `config::load_barns_checked()` parses `local.yaml` first and only
/// then drops it by name, so a stale one lands in its `errors` — where nothing
/// reads it. A real ranch has exactly this: a `barns/local.yaml` left over from
/// an older schema, with critters as bare strings and a `livestock` key `Barn`
/// has never had. Parsing it and then discarding it would refuse every manifest
/// on that machine forever, over a file the app itself does not believe exists.
/// It is skipped the same way `.locks/` is: never opened, never an error.
/// `pub` as of Task D4: `ranch init` has to find a `is_ranch_house` marker and
/// `ranch join` has to resolve a target, and both are the same question a
/// manifest asks — "what barn *files* are there" — rather than the one
/// `load_barns()` answers.
pub fn barns_from_disk() -> LoadResult<Barn> {
    let mut loaded: LoadResult<Barn> =
        load_yaml_dir(&config::barns_dir(), |stem| stem == config::LOCAL_BARN_NAME);
    // Belt and braces for the case the filename cannot catch: some other file
    // whose contents claim the synthetic name.
    loaded.items.retain(|barn| !config::is_local_barn(barn));
    loaded
}

/// `config::load_dir`, which is private to that module.
///
/// The `.yaml` filter is the load-bearing part and is copied deliberately:
/// `store` keeps its lock files in a `.locks` subdirectory (extension `None`)
/// and writes through `.<name>.tmp-<pid>-<seq>` temp files (extension
/// `Some("tmp-123-4")`). Both live inside the entity directories. Neither must
/// surface — not as an entry, and not as a parse error either, which after the
/// rule above would abort every sync on a ranch that merely has a lock file.
///
/// `skip_stem` names files that are not entities at all, and it is consulted
/// *before* the file is opened — see [`barns_from_disk`] for why that matters.
fn load_yaml_dir<T: serde::de::DeserializeOwned>(
    dir: &std::path::Path,
    skip_stem: impl Fn(&str) -> bool,
) -> LoadResult<T> {
    let mut items = Vec::new();
    let mut errors = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // An empty ranch is legitimately empty, and a kind whose directory has
        // never been created is the normal state of a fresh install.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return LoadResult { items, errors }
        }
        // Anything else — a permission bit, a mount that went away — is a
        // directory we *cannot read*, which is not the same statement as "it is
        // empty". Swallowing it builds a manifest silently missing an entire
        // kind, the peer reads that as "they do not have these", and it ships
        // its own copies over the top of files we merely failed to open. That
        // is precisely what the module docs promise cannot happen, so it is an
        // error on the directory, and `build` refuses the whole manifest.
        Err(e) => {
            errors.push(LoadError { path: dir.to_path_buf(), message: e.to_string() });
            return LoadResult { items, errors };
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "yaml") {
            continue;
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        if skip_stem(&stem) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_yaml::from_str::<T>(&content) {
                Ok(item) => items.push(item),
                Err(e) => errors.push(LoadError { path, message: e.to_string() }),
            },
            Err(e) => errors.push(LoadError { path, message: e.to_string() }),
        }
    }

    LoadResult { items, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// Writes a file straight into an entity directory, the way the store
    /// would. Tests here deliberately do not go through `config::save_*`: the
    /// point of a manifest is what is *on disk*, including files no saver would
    /// ever have produced.
    fn write_entity(dir: std::path::PathBuf, file: &str, yaml: &str) {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), yaml).unwrap();
    }

    fn project_yaml(name: &str) -> String {
        format!("name: {}\npath: /tmp/{}\n", name, name)
    }
    fn barn_yaml(name: &str) -> String {
        format!("name: {}\nhost: {}.local\n", name, name)
    }
    fn worm_yaml(name: &str) -> String {
        format!(
            "name: {}\ncommand: echo hi\nschedule: '0 0 * * *'\ntype: cron\nenabled: true\n",
            name
        )
    }
    fn trail_yaml(name: &str) -> String {
        format!("name: {}\njobs: {{}}\n", name)
    }
    fn ranchhand_yaml(name: &str) -> String {
        format!(
            "name: {}\nproject: api\ntype: k8s\nconfig:\n  kubeconfig_path: /tmp/kc\n\
             sync_settings:\n  auto_sync: false\n  interval_minutes: null\nherd: web\n",
            name
        )
    }

    fn kinds_and_names(entries: &[ManifestEntry]) -> Vec<(String, String)> {
        entries.iter().map(|e| (e.kind.clone(), e.name.clone())).collect()
    }

    /// Nothing on disk means nothing offered. In particular the synthetic
    /// `local` barn, which `load_barns()` would put at index 0 of an otherwise
    /// empty ranch, is not an entity and must not be one here.
    #[test]
    fn an_empty_ranch_has_an_empty_manifest() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            assert_eq!(build().unwrap(), Vec::new(), "an empty ranch offers a peer nothing");
        });
    }

    #[test]
    fn every_entity_kind_appears() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));
            write_entity(config::barns_dir(), "pi.yaml", &barn_yaml("pi"));
            write_entity(config::worms_dir(), "nightly.yaml", &worm_yaml("nightly"));
            write_entity(config::trails_dir(), "deploy.yaml", &trail_yaml("deploy"));
            write_entity(config::ranchhands_dir(), "k8s.yaml", &ranchhand_yaml("k8s"));

            let entries = build().unwrap();

            assert_eq!(
                kinds_and_names(&entries),
                vec![
                    ("barn".to_string(), "pi".to_string()),
                    ("project".to_string(), "api".to_string()),
                    ("ranchhand".to_string(), "k8s".to_string()),
                    ("trail".to_string(), "deploy".to_string()),
                    ("worm".to_string(), "nightly".to_string()),
                ],
                "every kind a sync moves has to be offered, and only once"
            );
            for entry in &entries {
                assert!(!entry.hash.is_empty(), "{} {} has no hash", entry.kind, entry.name);
            }
        });
    }

    /// Finding 5. `local` is injected into every `load_barns()` listing, is
    /// never persisted and has no uuid. A sync that enumerated barns through
    /// that loader would offer the peer a barn that does not exist, and the
    /// peer would have no uuid to match it on and no file to ask for.
    #[test]
    fn the_synthetic_local_barn_is_never_offered_to_a_peer() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::barns_dir(), "pi.yaml", &barn_yaml("pi"));

            // Pin the disagreement itself, so this test still means something
            // if `load_barns` ever changes: the user-facing listing *does*
            // contain `local`, and the manifest still must not.
            assert!(
                config::load_barns().iter().any(config::is_local_barn),
                "load_barns() is expected to inject the synthetic local barn"
            );

            let names = kinds_and_names(&build().unwrap());
            assert_eq!(names, vec![("barn".to_string(), "pi".to_string())]);
        });
    }

    /// The other half of the same rule. A `local.yaml` left on disk — by a hand
    /// edit, or by a machine that predates `migrate::adopt_this_machine` — is
    /// dropped by every loader in this codebase. Shipping it to a peer would
    /// plant a barn neither machine can ever show.
    #[test]
    fn an_on_disk_local_barn_is_not_offered_either() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            // Caught by the filename, before the file is ever opened.
            write_entity(config::barns_dir(), "local.yaml", &barn_yaml("local"));
            // Caught only by the name inside it — the filename says nothing.
            write_entity(config::barns_dir(), "stray.yaml", &barn_yaml("local"));
            write_entity(config::barns_dir(), "pi.yaml", &barn_yaml("pi"));

            let names = kinds_and_names(&build().unwrap());
            assert_eq!(names, vec![("barn".to_string(), "pi".to_string())]);
        });
    }

    /// Found on the real ranch, not imagined. `~/.yeehaw/barns/local.yaml` there
    /// is a leftover from an older schema — critters as bare strings, and a
    /// `livestock` key `Barn` has never had — and it does not parse.
    ///
    /// The app never notices, because `load_barns_checked()` drops it *by name
    /// after parsing it*, leaving the failure in a `LoadResult.errors` nothing
    /// reads. A manifest that did the same would refuse to build on that machine
    /// forever, and the sync would be dead on arrival over a file the app itself
    /// does not believe exists. The skip has to happen before the open.
    #[test]
    fn a_stale_unparseable_local_barn_does_not_fail_the_manifest() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(
                config::barns_dir(),
                "local.yaml",
                "name: local\nhost: localhost\ncritters:\n  - nginx\n  - mysql\n\
                 livestock:\n  - name: demo-app\n    type: node\n",
            );
            write_entity(config::barns_dir(), "pi.yaml", &barn_yaml("pi"));

            let entries =
                build().expect("a stale local.yaml must not be able to refuse every sync");
            assert_eq!(kinds_and_names(&entries), vec![("barn".to_string(), "pi".to_string())]);
        });
    }

    /// Most of a real ranch has no uuid: nothing restamps a file until it is
    /// next written. The manifest has to say so, because `None` is what routes
    /// the entity down the name-matching path on a first join. Anything
    /// invented here — the name, a fresh uuid — is a key the peer cannot have.
    #[test]
    fn an_unstamped_entity_appears_with_no_id() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));
            write_entity(
                config::projects_dir(),
                "web.yaml",
                &format!(
                    "{}id: 11111111-2222-3333-4444-555555555555\n\
                     updated_at: '2026-01-01T00:00:00+00:00'\n",
                    project_yaml("web")
                ),
            );

            let entries = build().unwrap();
            let api = entries.iter().find(|e| e.name == "api").unwrap();
            let web = entries.iter().find(|e| e.name == "web").unwrap();

            assert_eq!(api.id, None, "an unstamped entity must report no id");
            assert_eq!(api.updated_at, None);
            assert_eq!(web.id.as_deref(), Some("11111111-2222-3333-4444-555555555555"));
            assert_eq!(web.updated_at.as_deref(), Some("2026-01-01T00:00:00+00:00"));
        });
    }

    /// `.locks/` and `.<name>.tmp-<pid>-<seq>` live inside the entity
    /// directories. Neither is an entity, and neither is a broken entity —
    /// surfacing a lock file as a parse error would abort every sync on any
    /// ranch that has ever taken a lock, which is all of them.
    #[test]
    fn lock_and_temp_files_are_invisible() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::barns_dir(), "pi.yaml", &barn_yaml("pi"));
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));

            // Real lock files, taken the way the store takes them.
            drop(crate::store::lock_entity(&config::barns_dir(), "pi").unwrap());
            drop(crate::store::lock_entity(&config::projects_dir(), "api").unwrap());

            // A temp file mid-write. Valid content on purpose: if the extension
            // filter were dropped this would appear as a *second* entity rather
            // than as an error, which is the quieter and worse failure.
            write_entity(config::barns_dir(), ".pi.yaml.tmp-999-0", &barn_yaml("ghost"));
            write_entity(config::projects_dir(), ".api.yaml.tmp-999-1", &project_yaml("ghost"));

            let entries = build().expect("a lock file must not be reported as a broken entity");

            assert_eq!(
                kinds_and_names(&entries),
                vec![
                    ("barn".to_string(), "pi".to_string()),
                    ("project".to_string(), "api".to_string()),
                ],
                "only real entities may appear"
            );
        });
    }

    /// A file that will not parse must abort the manifest, not shrink it. A
    /// shrunken manifest tells the peer "we do not have that entity", and the
    /// peer answers by sending its own copy over the top of the local file we
    /// merely failed to read.
    #[test]
    fn a_parse_failure_surfaces_rather_than_shrinking_the_manifest() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));
            write_entity(config::projects_dir(), "broken.yaml", "name: [unclosed\n");

            let err = build().expect_err("a broken entity file must fail the manifest");
            let message = format!("{}", err);
            assert!(
                message.contains("broken.yaml"),
                "the error must name the file that could not be read: {}",
                message
            );
        });
    }

    /// The manifest's `hash` is the most load-bearing field in the protocol —
    /// it is the whole of "did this entity change?" — and this is the seam
    /// where it is either wired to the canonicalizer or is a decoration.
    /// Asserting it is non-empty does not tell the two apart: a constant
    /// passes that.
    #[test]
    fn an_edit_to_an_entity_moves_its_manifest_hash() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));

            let before = build().unwrap();
            let before = before.iter().find(|e| e.name == "api").unwrap().hash.clone();

            write_entity(
                config::projects_dir(),
                "api.yaml",
                &format!("{}summary: now it has a summary\n", project_yaml("api")),
            );

            let after = build().unwrap();
            let after = after.iter().find(|e| e.name == "api").unwrap().hash.clone();

            assert_ne!(
                before, after,
                "editing a project's summary must move its manifest hash, or a peer is told \
                 nothing changed and the edit never ships"
            );
        });
    }

    /// The other half: a hash that moves on an edit but is shared between two
    /// different entities is just as useless — the peer would read one entity's
    /// hash as evidence about the other. Kind is part of the identity of a
    /// manifest row, so two entities of different kinds with the same name must
    /// not collide either.
    #[test]
    fn entities_that_differ_have_different_manifest_hashes() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));
            write_entity(config::projects_dir(), "web.yaml", &project_yaml("web"));
            write_entity(config::barns_dir(), "api.yaml", &barn_yaml("api"));

            let entries = build().unwrap();
            let hash_of = |kind: &str, name: &str| {
                entries
                    .iter()
                    .find(|e| e.kind == kind && e.name == name)
                    .unwrap_or_else(|| panic!("{} {} missing from the manifest", kind, name))
                    .hash
                    .clone()
            };

            assert_ne!(
                hash_of("project", "api"),
                hash_of("project", "web"),
                "two projects with different content must not share a hash"
            );
            assert_ne!(
                hash_of("project", "api"),
                hash_of("barn", "api"),
                "a project and a barn of the same name must not share a hash"
            );
        });
    }

    /// A hash the peer cannot reproduce is worse than no hash: it is a
    /// permanent disagreement. Whatever the canonical form is, it has to be a
    /// pure function of the entity, and re-reading an untouched ranch has to
    /// yield the identical manifest.
    #[test]
    fn an_untouched_ranch_hashes_the_same_on_every_build() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));
            write_entity(config::trails_dir(), "deploy.yaml", &trail_yaml("deploy"));
            write_entity(config::ranchhands_dir(), "k8s.yaml", &ranchhand_yaml("k8s"));

            let first = build().unwrap();
            for round in 0..8 {
                assert_eq!(build().unwrap(), first, "the manifest changed on round {}", round);
            }
        });
    }

    /// An entity directory that is *present but unreadable* is not an empty
    /// one, and the difference is the whole contract of this module. Swallowed,
    /// it yields a manifest with an entire kind silently missing; the peer reads
    /// that as "they do not have any barns" and answers by shipping its own over
    /// the top of the files we merely failed to open.
    ///
    /// `NotFound` stays silent on purpose — a ranch with no barns yet is a
    /// perfectly ordinary ranch, and erroring on it would break every fresh
    /// install.
    #[test]
    fn an_unreadable_entity_directory_fails_the_manifest_rather_than_shrinking_it() {
        use std::os::unix::fs::PermissionsExt;

        fn chmod(dir: &std::path::Path, mode: u32) {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        }

        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            write_entity(config::projects_dir(), "api.yaml", &project_yaml("api"));
            write_entity(config::barns_dir(), "pi.yaml", &barn_yaml("pi"));

            let barns = config::barns_dir();
            chmod(&barns, 0o000);

            // Running as root, or on a filesystem that ignores the mode bits,
            // the condition simply cannot be expressed — say so rather than
            // pass vacuously.
            if std::fs::read_dir(&barns).is_ok() {
                chmod(&barns, 0o755);
                eprintln!("skipped: this environment can read a 0o000 directory");
                return;
            }

            let result = build();

            // Restored before the assertion, so a failure still leaves a
            // tempdir that can be cleaned up.
            chmod(&barns, 0o755);

            let err = result
                .expect_err("an unreadable barns directory must fail the manifest, not empty it");
            let message = format!("{}", err);
            assert!(
                message.contains("barns"),
                "the error must name the directory that could not be read: {}",
                message
            );
        });
    }

    /// The other side of the same rule, and the reason it cannot simply be
    /// "any `read_dir` error is fatal": a kind whose directory does not exist
    /// is a ranch that has none of that kind. `build` calls
    /// `ensure_config_dirs()` so it never sees this itself, which is exactly
    /// why the branch is pinned here rather than through `build` — an
    /// `ensure_config_dirs` that fails to create a directory, or a directory
    /// removed between the two calls, must still read as empty and not as a
    /// refused sync.
    #[test]
    fn a_missing_entity_directory_is_an_empty_one_not_an_error() {
        testing::with_temp_ranch(|ranch| {
            let absent = ranch.dir.path().join("no-such-kind");
            let loaded: LoadResult<Barn> = load_yaml_dir(&absent, |_| false);
            assert!(loaded.items.is_empty());
            assert!(
                loaded.errors.is_empty(),
                "a ranch that has none of a kind is empty, not broken: {:?}",
                loaded.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
            );
        });
    }

    /// The same ranch must produce the same manifest every time. Ranch hands
    /// come back unsorted from their loader and directory iteration order is
    /// not defined, so without an imposed order the sync plan a user is asked
    /// to approve would reshuffle between runs.
    #[test]
    fn entries_are_ordered_by_kind_then_name() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            for name in ["zulu", "alpha", "mike"] {
                write_entity(
                    config::projects_dir(),
                    &format!("{}.yaml", name),
                    &project_yaml(name),
                );
                write_entity(config::barns_dir(), &format!("{}.yaml", name), &barn_yaml(name));
            }

            assert_eq!(
                kinds_and_names(&build().unwrap()),
                vec![
                    ("barn".to_string(), "alpha".to_string()),
                    ("barn".to_string(), "mike".to_string()),
                    ("barn".to_string(), "zulu".to_string()),
                    ("project".to_string(), "alpha".to_string()),
                    ("project".to_string(), "mike".to_string()),
                    ("project".to_string(), "zulu".to_string()),
                ]
            );
        });
    }
}
