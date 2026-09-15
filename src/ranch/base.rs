//! The base snapshot: what each entity looked like the last time it was synced.
//!
//! `~/.yeehaw/.ranch/base/<kind>/<uuid>.yaml`. This is the third leg of the
//! three-way merge — the common ancestor that lets "the peer changed this" be
//! told apart from "we changed this". Without it there is no ancestor, the
//! merge degrades to a two-way comparison, and a two-way comparison cannot do
//! anything with a disagreement except pick a side and discard the other.
//!
//! # The invariant
//!
//! **The base is written only as part of accepting a merge result, and only
//! from what was actually applied.** A base that drifts from the last synced
//! state does not fail loudly; it makes every subsequent merge quietly wrong,
//! and the symptom is a user's edit disappearing days later.
//!
//! There is deliberately **no public "write one base file" function**, because
//! a base write that a caller can issue on its own is a base write that will
//! eventually be issued without the matching apply. [`accept`] is the only way
//! in, and it takes the apply as an argument:
//!
//! - It **runs the apply first** and refuses to record anything if the apply
//!   failed. A base written before a failed apply claims a sync that never
//!   happened, which is the drift direction that loses data.
//! - It **snapshots the entity after the apply**, not the value the caller was
//!   holding. `config::save_*` stamps on the way past, so a pre-apply snapshot
//!   would record an entity with no uuid — one that could not even be filed —
//!   and, once fields start being merged in Slice C, a base that disagrees with
//!   the file it is supposed to describe.
//! - It **files under the id the entity ended up with**, read back out of the
//!   entity rather than passed in beside it, so the name of the base file
//!   cannot disagree with its contents.
//!
//! The two writes — the store's and the base's — are not one atomic unit; no
//! two-file write on a plain filesystem is. The ordering is chosen so that the
//! surviving failure mode is the safe one. If the base write fails after the
//! apply, the base *lags*: the next merge sees an older ancestor, treats an
//! already-synced change as a local edit, and at worst re-offers or conflicts
//! it. If it were written first and the apply failed, the base would *lead* —
//! it would claim a state the store never reached, and the next merge would
//! read a real local change as "already synced" and drop it silently. Lagging
//! is recoverable. Leading is not.
//!
//! # A corrupt base is one entity's problem, and must stay that way
//!
//! [`load`] errors rather than returning `None` when a base exists but will not
//! parse — reading a corrupt ancestor as an absent one is the silent slide back
//! to a two-way merge. The remedy is in the error message, because the user is
//! the one who has to act on it: **delete the file**. The cost of doing so is
//! bounded and worth naming — that one entity loses its ancestor and merges
//! two-way on the next sync, which is what it would have done anyway.
//!
//! **Slice C must isolate this per entity.** One unparseable file under
//! `.ranch/base/` is a single entity with no usable ancestor; if the sync loop
//! propagates the error out of the whole run, that one file blocks every other
//! entity from syncing at all, and the user's only signal is a sync that has
//! quietly stopped working. Collect the failure against the entity, degrade
//! *that* entity to a two-way merge, and surface it in the plan the user
//! approves.
//!
//! # Locking is the caller's job, and it is not optional
//!
//! See [`accept`]. It takes no locks, on purpose, and the natural
//! implementation — locking inside `accept` — is the one that hangs.

#![allow(dead_code)] // Wired up in Slice C/E.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::Identified;

/// `~/.yeehaw/.ranch/base` — created by `config::ensure_config_dirs()`.
///
/// Under a dot directory because it is protocol bookkeeping, not ranch content:
/// nothing in the TUI browses it and no loader should ever pick it up.
pub fn base_dir() -> PathBuf {
    crate::config::yeehaw_dir().join(".ranch").join("base")
}

/// The last-synced state of one entity, or `None` if it has never been synced.
///
/// `None` is a real and common answer — every entity created locally since the
/// last sync has no base — and it means "no common ancestor, merge two-way".
///
/// A base that exists but will not parse is an **error**, not a `None`. The
/// difference matters: silently treating a corrupt ancestor as an absent one
/// turns a loud problem into the exact silent two-way clobber this file exists
/// to prevent.
pub fn load<T: DeserializeOwned>(kind: &str, id: &str) -> Result<Option<T>> {
    let path = path_for(kind, id)?;
    match fs::read_to_string(&path) {
        Ok(content) => {
            let entity = serde_yaml::from_str(&content).with_context(|| {
                format!(
                    "sync base at {} is unreadable. Delete the file to clear it: the cost is \
                     that this one entity loses its common ancestor and merges two-way on the \
                     next sync, so a change made on both machines picks a side instead of \
                     merging. Nothing else on the ranch is affected",
                    path.display()
                )
            })?;
            Ok(Some(entity))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Applies one merge result and records it as the new base.
///
/// This is the **only** way a base is ever written. `apply` is whatever puts
/// the entity into the live store — `config::save_project`, `save_barn`, and so
/// on — and it runs first: nothing is recorded if it fails, and what is
/// recorded is the entity as `apply` left it, stamp and all.
///
/// ```ignore
/// base::accept("project", &mut merged, |p| config::save_project(p))?;
/// ```
///
/// See the module docs for why the shape is this way rather than a plain
/// `write_base(kind, id, yaml)`.
///
/// # Locking: the caller holds them, and `accept` takes none
///
/// **The caller must already hold `store::lock_entity` for every entity in the
/// batch before the first `accept`, acquired in sorted order over
/// `store::lock_key`** — not over the entity name; the two orders disagree,
/// because `lock_key` collapses characters and `'X'` sorts below `'_'`. Dedupe
/// on the key too: two names can share one. `config::rename_project` is the
/// reference implementation.
///
/// `accept` deliberately takes no lock of its own. Locking here is the obvious
/// design and it is the one that hangs: `fs2`'s locks are not re-entrant and
/// have no timeout, so an `accept` whose caller already holds that entity's
/// lock blocks the thread against itself, forever, with no error and no way out
/// but a kill. The lock also has to be held for the whole batch for a second
/// reason — a sync that locks one entity at a time applies half a merge while
/// another writer is free to change the rest of the ranch underneath it.
pub fn accept<T>(kind: &str, entity: &mut T, apply: impl FnOnce(&mut T) -> Result<()>) -> Result<()>
where
    T: Serialize + Identified,
{
    // Before the apply, not after. `kind` is a compile-time constant at every
    // call site, so a bad one is a bug in this crate and has nothing to do with
    // the entity — there is no reason to discover it only after a real store
    // write has already landed.
    validate_kind(kind)?;

    // First, and fatal. A base recorded ahead of a failed apply describes a
    // sync that never happened.
    apply(entity).with_context(|| format!("failed to apply the merged {}", kind))?;

    // Everything past this point has already changed the store. The messages
    // say so: a caller batching accepts behind `?` will abort here, and
    // "failed to record the sync base" on its own reads as "nothing happened",
    // which would send the user looking for an apply that did in fact land.
    let applied = |what: &str| {
        format!(
            "the {} was applied, but {} — so its sync base was not recorded. The change is in \
             the store; the next sync will see it as a local edit and re-offer it",
            kind, what
        )
    };

    // Read back out of the entity, not taken from the caller: `apply` mints the
    // uuid for anything that did not have one, and the base has to be filed
    // under the id the store now holds.
    let id = entity.id().map(|s| s.to_string()).ok_or_else(|| {
        anyhow!(
            "{}",
            applied(
                "it came back unstamped, and an unstamped entity has no identity to file a \
                 base under"
            )
        )
    })?;

    let path = path_for(kind, &id)
        .with_context(|| applied(&format!("its id {:?} cannot name a base file", id)))?;
    let content = serde_yaml::to_string(entity)
        .with_context(|| applied("it could not be serialized"))?;

    crate::store::write_atomic(&path, &content)
        .with_context(|| applied(&format!("writing {} failed", path.display())))
}

/// The five kinds are the only directories a base may live in.
///
/// Separate from [`path_for`] so [`accept`] can check it before running the
/// apply. Still called from `path_for` as well: this is the guard that stops a
/// typo quietly creating a sixth directory whose bases nothing ever reads back.
fn validate_kind(kind: &str) -> Result<()> {
    if !crate::ranch::manifest::KINDS.contains(&kind) {
        bail!("no such entity kind for a sync base: {:?}", kind);
    }
    Ok(())
}

fn path_for(kind: &str, id: &str) -> Result<PathBuf> {
    validate_kind(kind)?;
    if id.is_empty() {
        bail!("a sync base needs an entity id");
    }
    Ok(base_dir().join(kind).join(format!("{}.yaml", file_stem_for(id)?)))
}

/// The filename stem for `id` — or an error, if `id` is not already one.
///
/// Two jobs, and the order matters. The sanitizer (`store::sanitized_stem`, the
/// same one `store::lock_key` and `tombstones::file_stem_for` use) is
/// containment: an id reaching this function came out of an entity file a *peer*
/// sent us, and `Path::join` on `"../../escaped"` walks straight out of the
/// store. That stays.
///
/// What does not stay is *accepting* the sanitized result. The mapping is
/// many-to-one, so a peer holding ids `a.b` and `a/b` would collapse both to
/// `a_b`: two entities sharing one base file, and one entity's ancestor
/// silently becoming the other's. That is the two-way clobber this module
/// exists to prevent, reachable with no traversal at all and no need for the id
/// to be hostile — only malformed.
///
/// So an id that *needs* sanitizing is refused instead. Every id this is given
/// in a real path is a uuid minted by `Identified::stamp` — hex and `-` — and
/// passes through untouched; anything else is already a malformed record, and a
/// malformed record is better refused loudly at one entity than merged quietly
/// into the wrong one.
fn file_stem_for(id: &str) -> Result<String> {
    let stem = crate::store::sanitized_stem(id);
    if stem != id {
        bail!(
            "refusing to file a sync base under the entity id {:?}: it is not a plain \
             identifier, and the sanitized form {:?} is shared with every other id that \
             differs from it only in punctuation — two entities would overwrite each \
             other's ancestor and the merge would silently degrade to two-way. Ids are \
             minted as uuids; this one did not come from `Identified::stamp`",
            id,
            stem
        );
    }
    Ok(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::ranch::canonical::hash_entity;
    use crate::testing;
    use crate::types::Project;

    fn project(name: &str) -> Project {
        serde_yaml::from_str(&format!("name: {}\npath: /tmp/{}\n", name, name)).unwrap()
    }

    /// The directory has to exist before the first sync, for the same reason
    /// every other store directory is made up front: a missing one turns the
    /// first write of a feature into its first bug report.
    #[test]
    fn ensure_config_dirs_creates_the_base_directory() {
        testing::with_temp_ranch(|ranch| {
            config::ensure_config_dirs();
            assert!(
                ranch.dir.path().join(".ranch").join("base").is_dir(),
                "the sync base directory must be created with the rest of the store"
            );
        });
    }

    #[test]
    fn a_base_written_through_accept_reads_back_as_the_same_entity() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            p.summary = Some("the API".into());

            accept("project", &mut p, |p| config::save_project(p)).unwrap();

            let id = p.id.clone().expect("save stamps an id");
            let stored: Project = load("project", &id).unwrap().expect("a base was just written");

            assert_eq!(stored.name, "api");
            assert_eq!(stored.summary.as_deref(), Some("the API"));
            assert_eq!(stored.id, p.id);
            assert_eq!(hash_entity("project", &stored).unwrap(), hash_entity("project", &p).unwrap());
        });
    }

    /// The common case on any ranch that has just gained an entity. `None` is
    /// the answer that tells a merge "no ancestor here", and it must not be an
    /// error, because erroring would make every newly created entity fail a
    /// sync.
    #[test]
    fn there_is_no_base_for_an_entity_that_was_never_synced() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let missing: Option<Project> =
                load("project", "11111111-2222-3333-4444-555555555555").unwrap();
            assert!(missing.is_none(), "an unsynced entity has no base");
        });
    }

    /// The drift test. `config::save_project` stamps on the way past, so the
    /// value the caller handed in and the value that reached disk are not the
    /// same value. The base has to be the second one — anything else describes
    /// a state the store was never in.
    #[test]
    fn the_base_records_what_apply_actually_wrote_not_what_it_was_asked_to_write() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            assert!(p.id.is_none(), "the entity starts unstamped, as most of a real ranch is");

            accept("project", &mut p, |p| config::save_project(p)).unwrap();

            let id = p.id.clone().unwrap();
            let base: Project = load("project", &id).unwrap().expect("a base was written");

            assert_eq!(base.id.as_deref(), Some(id.as_str()), "the base must carry the stamped id");
            assert!(
                base.updated_at.is_some(),
                "the base must be the post-apply entity, not the caller's pre-apply copy"
            );

            // And it must agree with what is actually in the store.
            let on_disk = config::load_projects().into_iter().find(|x| x.name == "api").unwrap();
            assert_eq!(base.id, on_disk.id);
            assert_eq!(base.updated_at, on_disk.updated_at);
        });
    }

    /// A base recorded when the apply failed claims a sync that never happened,
    /// and the next merge reads the real local state as "already synced" and
    /// discards it. Nothing at all is the only safe outcome.
    #[test]
    fn a_failed_apply_records_no_base() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            p.id = Some("11111111-2222-3333-4444-555555555555".into());

            let result = accept("project", &mut p, |_| anyhow::bail!("disk full"));
            assert!(result.is_err(), "a failed apply must fail the accept");

            let base: Option<Project> = load("project", p.id.as_ref().unwrap()).unwrap();
            assert!(base.is_none(), "no base may survive an apply that did not happen");
        });
    }

    /// Phase 1 stamps on every save, so a re-sync with nothing to merge still
    /// rewrites the file with a fresh `updated_at`. What must not change is the
    /// entity the base describes — and there must still be exactly one base
    /// file for it, not one per sync.
    #[test]
    fn a_resync_with_no_changes_leaves_the_base_unchanged() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            p.summary = Some("the API".into());

            accept("project", &mut p, |p| config::save_project(p)).unwrap();
            let id = p.id.clone().unwrap();
            let first: Project = load("project", &id).unwrap().unwrap();

            accept("project", &mut p, |p| config::save_project(p)).unwrap();
            let second: Project = load("project", &id).unwrap().unwrap();

            assert_eq!(
                hash_entity("project", &first).unwrap(),
                hash_entity("project", &second).unwrap(),
                "a sync that changed nothing must not change what the base describes"
            );

            let files: Vec<String> = fs::read_dir(base_dir().join("project"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            assert_eq!(files.len(), 1, "one base per entity, not one per sync: {:?}", files);
        });
    }

    /// The other direction, and the one that actually rots. If a base is
    /// written once and then left alone, every later merge is measured against
    /// an ancestor that is months out of date, and changes that were synced long
    /// ago keep coming back as local edits.
    #[test]
    fn a_resync_after_an_edit_moves_the_base_forward() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");

            accept("project", &mut p, |p| config::save_project(p)).unwrap();
            let id = p.id.clone().unwrap();

            p.summary = Some("edited".into());
            accept("project", &mut p, |p| config::save_project(p)).unwrap();

            let base: Project = load("project", &id).unwrap().unwrap();
            assert_eq!(
                base.summary.as_deref(),
                Some("edited"),
                "the base must follow what was last synced, not stay at the first sync"
            );
        });
    }

    /// Ids reaching this module came out of entity files a *peer* sent. A base
    /// filed at `../../` would write outside the store entirely.
    ///
    /// Both halves are asserted: the id is refused (see
    /// `two_ids_that_sanitize_alike_are_refused_rather_than_sharing_a_base`),
    /// **and** nothing escaped. The containment is defence in depth — the
    /// refusal is what should stop this, but the sanitizer has to still be there
    /// if the refusal is ever relaxed.
    #[test]
    fn a_hostile_id_cannot_escape_the_base_directory() {
        testing::with_temp_ranch(|ranch| {
            config::ensure_config_dirs();
            let mut p = project("api");
            p.id = Some("../../../../etc/pwned".into());

            // The apply is a no-op here: the id, not the store, is under test.
            let result = accept("project", &mut p, |_| Ok(()));
            assert!(result.is_err(), "a traversing id must be refused, not sanitized and stored");

            let escaped = ranch.dir.path().parent().unwrap().join("etc");
            assert!(!escaped.exists(), "a base escaped the ranch to {}", escaped.display());

            let written: Vec<String> = fs::read_dir(base_dir().join("project"))
                .map(|entries| {
                    entries.map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect()
                })
                .unwrap_or_default();
            assert!(written.is_empty(), "a refused id still wrote a base: {:?}", written);
        });
    }

    /// The collision that needs no traversal to reach. Ids arrive in entity
    /// files a peer sent, and nothing anywhere enforces that one is a uuid. A
    /// peer holding `a.b` and `a/b` collapses both to `a_b` under a sanitizer
    /// that merely rewrites: two entities share one base file, and whichever
    /// syncs second silently inherits the other's ancestor. That is the merge
    /// degrading to two-way and clobbering — the exact failure this module
    /// exists to prevent.
    #[test]
    fn two_ids_that_sanitize_alike_are_refused_rather_than_sharing_a_base() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();

            let mut first = project("first");
            first.id = Some("a.b".into());
            let mut second = project("second");
            second.id = Some("a/b".into());

            // Guard: if these ever stop colliding the test is proving nothing.
            assert_eq!(
                crate::store::sanitized_stem("a.b"),
                crate::store::sanitized_stem("a/b"),
                "the two ids no longer collide; the test has no teeth"
            );

            let one = accept("project", &mut first, |_| Ok(()));
            let two = accept("project", &mut second, |_| Ok(()));

            assert!(one.is_err(), "an id that is not a plain identifier must be refused");
            assert!(two.is_err(), "the colliding id must be refused too");

            let written: Vec<String> = fs::read_dir(base_dir().join("project"))
                .map(|entries| {
                    entries.map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect()
                })
                .unwrap_or_default();
            assert!(
                written.is_empty(),
                "two entities must not end up sharing one base file: {:?}",
                written
            );

            // And the same refusal on the way back out, so a base written by an
            // older build cannot be read as some other entity's ancestor.
            let read: Result<Option<Project>> = load("project", "a.b");
            assert!(read.is_err(), "a colliding id must not resolve to a base on load either");
        });
    }

    /// The uuids `Identified::stamp` actually mints must pass through untouched
    /// — a refusal that also refuses the real case is not a fix.
    #[test]
    fn a_minted_uuid_is_accepted_unchanged() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            accept("project", &mut p, |p| config::save_project(p)).unwrap();

            let id = p.id.clone().expect("save stamps a uuid");
            let path = base_dir().join("project").join(format!("{}.yaml", id));
            assert!(path.is_file(), "a minted uuid must file under itself: {}", path.display());
        });
    }

    /// M8. An entity that reaches the end of an apply with no id cannot be
    /// filed, and `accept` is the only writer of bases — reporting success here
    /// would leave the entity in the store with no ancestor, which is the
    /// silent two-way merge on the next sync.
    #[test]
    fn an_entity_that_survives_the_apply_unstamped_is_an_error() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            assert!(p.id.is_none());

            // An apply that does not stamp — a merge that writes through some
            // path other than `config::save_*`, or a future caller that forgets.
            let err = accept("project", &mut p, |_| Ok(()))
                .expect_err("an unstamped entity has no identity to file a base under");

            let message = format!("{:#}", err);
            assert!(
                message.contains("was applied"),
                "the message must say the apply landed, or it reads as 'nothing happened': {}",
                message
            );
        });
    }

    /// `kind` is a compile-time constant at every call site, so a bad one is a
    /// bug in this crate and has nothing to do with the entity. Discovering it
    /// only *after* a real store write has landed means reporting a failed sync
    /// over a ranch that was in fact modified.
    #[test]
    fn an_unknown_kind_is_refused_before_anything_is_applied() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            let mut applied = false;

            let result = accept("livestock", &mut p, |_| {
                applied = true;
                Ok(())
            });

            assert!(result.is_err(), "only the five top-level kinds have bases");
            assert!(!applied, "the apply must not run for a kind that can never be recorded");
        });
    }

    /// Every failure after the apply has to say the apply landed. A Slice C/E
    /// loop running accepts behind `?` aborts on the first of these and reports
    /// the sync as failed — over a ranch that is half applied. "failed to record
    /// the sync base at <path>" on its own reads as "nothing happened", and
    /// sends the user looking for a change that is already on disk.
    #[test]
    fn a_failure_after_the_apply_says_the_apply_landed() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let mut p = project("api");
            p.id = Some("11111111-2222-3333-4444-555555555555".into());

            // The base directory replaced by a file, so `write_atomic` cannot
            // create it — a real post-apply failure with the store already
            // written.
            let dir = base_dir().join("project");
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.parent().unwrap()).unwrap();
            fs::write(&dir, "not a directory").unwrap();

            let mut applied = false;
            let err = accept("project", &mut p, |_| {
                applied = true;
                Ok(())
            })
            .expect_err("the base write must fail with the base directory blocked");

            assert!(applied, "the apply must have run, or this test is measuring the wrong thing");
            let message = format!("{:#}", err);
            assert!(
                message.contains("was applied"),
                "a post-apply failure must not read as 'nothing happened': {}",
                message
            );
        });
    }

    /// A corrupt ancestor read as an absent one is the silent path back to a
    /// two-way merge — exactly what this file exists to stop.
    #[test]
    fn a_corrupt_base_is_an_error_not_a_missing_base() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let id = "11111111-2222-3333-4444-555555555555";
            let path = base_dir().join("project").join(format!("{}.yaml", id));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "name: [unclosed\n").unwrap();

            let result: Result<Option<Project>> = load("project", id);
            assert!(
                result.is_err(),
                "an unreadable base must be reported, never read as 'never synced'"
            );
        });
    }

    /// A kind that is not one of the five is a bug in this crate, and one that
    /// would otherwise leave bases in a directory nothing ever reads.
    #[test]
    fn an_unknown_entity_kind_is_refused() {
        testing::with_temp_ranch(|_| {
            config::ensure_config_dirs();
            let result: Result<Option<Project>> = load("livestock", "abc");
            assert!(result.is_err(), "only the five top-level kinds have bases");
        });
    }
}
