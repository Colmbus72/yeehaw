//! The joining side of a sync: the conversation, the plan, and the apply.
//!
//! `mod.rs` owns the verbs (`ranch init`, `ranch join`) and the user-facing
//! prompting. This module owns everything underneath that a test can drive
//! without a terminal: loading this machine's entities, asking a peer for its
//! own, turning the two into a [`MergePlan`], rendering that plan as text, and
//! writing the accepted half to disk.
//!
//! # Why the whole ranch crosses the wire on a first join
//!
//! A manifest carries `(kind, name, id, updated_at, hash)` — enough to decide
//! *whether* two machines differ, and nowhere near enough to merge. `merge::
//! plan_kind` wants the structs. So the client asks for every entity the peer's
//! manifest lists rather than only the ones whose hashes differ, and on a first
//! join that is the right set anyway: the two machines minted their uuids
//! independently, so even a byte-identical entity has to be merged in order to
//! **adopt the Ranch House's uuid**. Skipping the matching ones would leave the
//! joining machine holding its own ids forever, and name-matching would stay the
//! permanent state instead of being a one-time event.
//!
//! # Both directions, and the one rule that makes the push safe
//!
//! A join now pushes as well as pulls: the outgoing half of the plan is sent to
//! the house and written *there*. The machines on a real ranch hold largely
//! disjoint project sets, so a pull-only join left every entity a joining
//! machine had invented invisible to the rest of the ranch forever.
//!
//! The rule that governs the whole design: **no base is recorded for an
//! outgoing entity until the house says it applied it.** A base write is a
//! claim about what the *other* machine holds, and `base.rs` spells out the
//! asymmetry of getting that claim wrong — a base that lags the store is
//! recoverable (the entity is re-offered, the house finds it identical, nothing
//! happens) while a base that leads it is not (a real local change reads as
//! already-synced and is dropped forever). A base written when the entity was
//! *sent* leads the store the instant the send fails, so the send is not the
//! event the base hangs off. The acknowledgement is.
//!
//! That acknowledgement is per entity, not per batch — see
//! [`crate::ranch::wire::Message::Applied`] — and it arrives as a list of keys,
//! which [`apply`] takes alongside the plan. `apply` is where both halves' base
//! writes live, under one lock batch, so "a base only for what landed" is one
//! rule in one place rather than two rules that can drift.
//!
//! # What a push cannot carry yet
//!
//! Only upserts, and only ones that are not renames. A rename needs the receiver
//! to *remove* the file the entity used to live in, and a deletion needs it to
//! record a tombstone — both destructive, and letting `serve` write at all is
//! already a larger grant of authority than it had. Those outgoing changes are
//! held back, named in the plan the user approves, and given no base, so they are
//! re-offered on the next sync. That is the lagging direction: incomplete, not
//! lossy. See [`pushable`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config;
use crate::ranch::base;
use crate::ranch::merge::{self, Ancestor, Change, MergePlan, PlannedChange, Side};
use crate::ranch::wire::{ManifestEntry, Message, WireTombstone};
use crate::types::{Barn, Identified, Project, RanchHand, Worm};

/// Every top-level entity on one machine, as structs, plus its tombstones.
///
/// The struct-shaped twin of a manifest. Five named fields rather than a map of
/// kind to `Vec<Box<dyn ..>>`: `merge::plan_kind` is generic over
/// `Mergeable`, so the kinds have to be separated by type somewhere, and doing
/// it here means every later step is a plain field access instead of a downcast.
#[derive(Debug, Default, Clone)]
pub struct Ranch {
    pub projects: Vec<Project>,
    pub barns: Vec<Barn>,
    pub worms: Vec<Worm>,
    pub trails: Vec<crate::trails::Trail>,
    pub ranchhands: Vec<RanchHand>,
    pub tombstones: Vec<WireTombstone>,
}

/// This machine's entities, or a refusal naming every file that would not read.
///
/// **Fails rather than shrinks**, for the reason `manifest::build` gives: an
/// entity missing from one side's list is indistinguishable from an entity that
/// side does not have, and the merge would then offer to create it — over the
/// top of the file we merely failed to parse.
///
/// Barns come from [`crate::ranch::manifest::barns_from_disk`], never
/// `config::load_barns()`. That loader injects a synthetic `local` barn which is
/// not a real entity, and drops a real `barns/local.yaml` — and the actual ranch
/// has one of those, from an older schema, which does not parse.
pub fn load_local() -> Result<Ranch> {
    use crate::ranch::manifest::barns_from_disk;

    // `barns_from_disk` scans the directory itself rather than going through a
    // `config` loader, so nothing creates that directory for it. Same explicit
    // call, for the same reason, as `manifest::build`.
    config::ensure_config_dirs();

    let mut errors: Vec<config::LoadError> = Vec::new();

    let projects = config::load_projects_checked();
    let barns = barns_from_disk();
    let worms = config::load_worms_checked();
    let trails = config::load_all_trails_checked();
    let ranchhands = config::load_ranchhands_checked();

    for set in [
        &projects.errors,
        &barns.errors,
        &worms.errors,
        &trails.errors,
        &ranchhands.errors,
    ] {
        errors.extend(set.iter().cloned());
    }

    if !errors.is_empty() {
        let detail = errors
            .iter()
            .map(|e| format!("  {}: {}", e.path.display(), e.message))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "refusing to sync: {} entity file(s) on this machine could not be read.\n{}\n\
             Syncing with these missing would look to the peer like they do not exist here, \
             and it would offer to create its own copies over the top of them.",
            errors.len(),
            detail
        );
    }

    Ok(Ranch {
        projects: projects.items,
        barns: barns.items,
        worms: worms.items,
        trails: trails.items,
        ranchhands: ranchhands.items,
        tombstones: crate::tombstones::load_all().into_iter().map(Into::into).collect(),
    })
}

/// The `Manifest` message for a ranch already loaded as structs.
///
/// Built from the same `Ranch` the entities will be served out of, rather than
/// from a second independent walk of the directories: a manifest that lists an
/// entity the `Want` cannot then resolve is the one inconsistency a peer has no
/// way to interpret.
pub fn manifest_of(ranch: &Ranch) -> Result<Message> {
    let mut entries: Vec<ManifestEntry> = Vec::new();
    entries.extend(entries_for("project", &ranch.projects, |p| &p.name)?);
    entries.extend(entries_for("barn", &ranch.barns, |b| &b.name)?);
    entries.extend(entries_for("worm", &ranch.worms, |w| &w.name)?);
    entries.extend(entries_for("trail", &ranch.trails, |t| &t.name)?);
    entries.extend(entries_for("ranchhand", &ranch.ranchhands, |r| &r.name)?);

    // Sorted for the same reason `manifest::build` sorts: a plan pane and a
    // transcript are read by people, and ranch hands come back unsorted.
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));

    Ok(Message::Manifest { entries, tombstones: ranch.tombstones.clone() })
}

fn entries_for<T: serde::Serialize + Identified>(
    kind: &str,
    items: &[T],
    name_of: impl Fn(&T) -> &str,
) -> Result<Vec<ManifestEntry>> {
    items
        .iter()
        .map(|item| {
            Ok(ManifestEntry {
                kind: kind.to_string(),
                name: name_of(item).to_string(),
                id: item.id().map(|s| s.to_string()),
                updated_at: item.updated_at().map(|s| s.to_string()),
                hash: crate::ranch::canonical::hash_entity(kind, item)?,
            })
        })
        .collect()
}

/// `kind/id`, or `kind/name` for an entity the peer has never stamped.
///
/// The protocol's own spelling (see [`Message::Want`]). Every entry, not only
/// the ones whose hash differs — see the module docs for why a first join needs
/// the identical ones too.
pub fn want_keys(entries: &[ManifestEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| format!("{}/{}", e.kind, e.id.as_deref().unwrap_or(&e.name)))
        .collect()
}

/// One entity as an `Entity` message, looked up by a `Want` key.
///
/// `None` when nothing of that kind answers to that id or name — which on a
/// live ranch means the entity was deleted between the manifest and the `Want`,
/// and is the one case where serving nothing is better than failing the whole
/// session.
///
/// Matched on id **first**, then name. A key carrying a uuid must never be
/// answered by a same-named entity with a different uuid: that is two entities,
/// and handing one over under the other's key is how a peer ends up writing the
/// wrong record.
pub fn entity_for_key(ranch: &Ranch, key: &str) -> Option<Message> {
    let (kind, rest) = key.split_once('/')?;

    fn pick<T: serde::Serialize + Identified>(
        items: &[T],
        rest: &str,
        name_of: impl Fn(&T) -> &str,
    ) -> Option<(String, String)> {
        let found = items
            .iter()
            .find(|i| i.id() == Some(rest))
            .or_else(|| items.iter().find(|i| name_of(i) == rest))?;
        Some((name_of(found).to_string(), serde_yaml::to_string(found).ok()?))
    }

    let (name, yaml) = match kind {
        "project" => pick(&ranch.projects, rest, |p| &p.name),
        "barn" => pick(&ranch.barns, rest, |b| &b.name),
        "worm" => pick(&ranch.worms, rest, |w| &w.name),
        "trail" => pick(&ranch.trails, rest, |t| &t.name),
        "ranchhand" => pick(&ranch.ranchhands, rest, |r| &r.name),
        _ => None,
    }?;

    Some(Message::Entity { kind: kind.to_string(), name, yaml })
}

/// Files one received `Entity` into the right field of a [`Ranch`].
///
/// An entity whose YAML will not parse is an error, never a skip: the peer put
/// it on the wire as a thing it holds, so dropping it makes the merge plan offer
/// to *create* it here, which is a write built on a read that failed.
pub fn absorb_entity(ranch: &mut Ranch, kind: &str, name: &str, yaml: &str) -> Result<()> {
    let what = || format!("the peer's {} '{}' did not parse", kind, name);
    match kind {
        "project" => ranch.projects.push(serde_yaml::from_str(yaml).with_context(what)?),
        "barn" => ranch.barns.push(serde_yaml::from_str(yaml).with_context(what)?),
        "worm" => ranch.worms.push(serde_yaml::from_str(yaml).with_context(what)?),
        "trail" => ranch.trails.push(serde_yaml::from_str(yaml).with_context(what)?),
        "ranchhand" => ranch.ranchhands.push(serde_yaml::from_str(yaml).with_context(what)?),
        other => anyhow::bail!("the peer sent an entity of unknown kind '{}'", other),
    }
    Ok(())
}

/// The plan for every kind at once, against the bases on this machine.
///
/// `house` says which side's uuid and which side's value wins a tie — a
/// property of the *machine*, not of an argument slot. For `ranch join` it is
/// [`Side::Remote`]: the target is the Ranch House.
///
/// Bases are looked up per entity through [`Ancestor::from_load`], so one
/// unparseable file under `.ranch/base/` degrades that entity to a two-way
/// merge and lands in the plan as a note, instead of aborting the run.
pub fn build_plan(ours: &Ranch, theirs: &Ranch, house: Side) -> Result<MergePlan> {
    let mut plan = MergePlan::default();

    plan.absorb(merge::plan_kind(
        &ours.projects,
        &theirs.projects,
        |id| Ancestor::from_load(base::load::<Project>("project", id)),
        &ours.tombstones,
        &theirs.tombstones,
        house,
    )?);
    plan.absorb(merge::plan_kind(
        &ours.barns,
        &theirs.barns,
        |id| Ancestor::from_load(base::load::<Barn>("barn", id)),
        &ours.tombstones,
        &theirs.tombstones,
        house,
    )?);
    plan.absorb(merge::plan_kind(
        &ours.worms,
        &theirs.worms,
        |id| Ancestor::from_load(base::load::<Worm>("worm", id)),
        &ours.tombstones,
        &theirs.tombstones,
        house,
    )?);
    plan.absorb(merge::plan_kind(
        &ours.trails,
        &theirs.trails,
        |id| Ancestor::from_load(base::load::<crate::trails::Trail>("trail", id)),
        &ours.tombstones,
        &theirs.tombstones,
        house,
    )?);
    plan.absorb(merge::plan_kind(
        &ours.ranchhands,
        &theirs.ranchhands,
        |id| Ancestor::from_load(base::load::<RanchHand>("ranchhand", id)),
        &ours.tombstones,
        &theirs.tombstones,
        house,
    )?);

    Ok(plan)
}

/// The plan as the user reads it before answering y/n.
///
/// Plain text, no TUI and no colour: this is printed by a CLI verb that may be
/// running over ssh in somebody else's terminal. Every section names what it
/// would do and, for a conflict, which side's value survived — a plan that
/// reports a conflict without saying who won is not a thing a user can consent
/// to.
pub fn render_plan(plan: &MergePlan, house_name: &str) -> String {
    let mut out = String::new();

    out.push_str(&format!("Sync plan against the Ranch House '{}':\n", house_name));

    out.push_str(&format!("\n  Incoming — written here ({}):\n", plan.incoming.len()));
    if plan.incoming.is_empty() {
        out.push_str("    (nothing)\n");
    }
    for change in &plan.incoming {
        out.push_str(&format!("    {}\n", describe(change)));
    }

    // Split, not one list with footnotes: the user is consenting to a write on
    // a machine they are not sitting at, so what will be written there and what
    // will not are two different statements and the second one is the surprise.
    let (sending, held): (Vec<&PlannedChange>, Vec<&PlannedChange>) =
        plan.outgoing.iter().partition(|c| unpushable(c).is_none());

    out.push_str(&format!(
        "\n  Outgoing — pushed to the house '{}' ({}):\n",
        house_name,
        sending.len()
    ));
    if sending.is_empty() {
        out.push_str("    (nothing)\n");
    }
    for change in sending {
        out.push_str(&format!("    {}\n", describe(change)));
    }

    if !held.is_empty() {
        out.push_str(&format!(
            "\n  Outgoing — held back, a push cannot carry these yet ({}):\n",
            held.len()
        ));
        for change in held {
            out.push_str(&format!(
                "    {} — {}\n",
                describe(change),
                unpushable(change).expect("partitioned as unpushable")
            ));
        }
        // Said here rather than in a release note, because the user is about to
        // consent to something and this is the part that does not happen.
        out.push_str(
            "    NOTE: these stay here, unchanged, with no sync base — so they are offered\n\
             \x20         again on the next sync rather than being lost.\n",
        );
    }

    if !plan.conflicts.is_empty() {
        out.push_str(&format!("\n  Conflicts ({}):\n", plan.conflicts.len()));
        for c in &plan.conflicts {
            out.push_str(&format!(
                "    {} {} — field {}\n      here:  {}\n      house: {}\n      kept:  {}\n",
                c.kind,
                c.name,
                c.field,
                one_line(&c.ours),
                one_line(&c.theirs),
                match c.winner {
                    Side::Local => "here",
                    Side::Remote => "the house",
                }
            ));
        }
    }

    if !plan.notes.is_empty() {
        out.push_str(&format!("\n  Notes ({}):\n", plan.notes.len()));
        for n in &plan.notes {
            out.push_str(&format!("    {} {}: {}\n", n.kind, n.name, one_line(&n.reason)));
        }
    }

    out
}

fn describe(change: &PlannedChange) -> String {
    match (&change.change, &change.previous_name) {
        (Change::Upsert { .. }, Some(old)) if old != &change.name => {
            format!("write  {} {} (renamed from {})", change.kind, change.name, old)
        }
        (Change::Upsert { .. }, _) => format!("write  {} {}", change.kind, change.name),
        (Change::Delete, _) => format!("delete {} {}", change.kind, change.name),
    }
}

// ============================================================================
// The push: sending the outgoing half, and basing only what landed
// ============================================================================

/// The key an [`Message::Applied`] acknowledgement names one entity by.
///
/// `kind/id`, or `kind/name` for an entity nobody has stamped — the same
/// spelling [`want_keys`] uses. Both machines compute it from the entity
/// itself, the receiver doing so **before** its save stamps anything, so the
/// two cannot disagree about what a key refers to.
pub fn push_key(kind: &str, id: Option<&str>, name: &str) -> String {
    format!("{}/{}", kind, id.unwrap_or(name))
}

/// Why an outgoing change cannot be pushed, in words for the plan — or `None`
/// when it can.
///
/// The boundary is deliberate and narrow: an `Entity` message carries a kind, a
/// name and a payload, which is exactly enough to *write* an entity and not
/// enough to remove one. A rename needs the receiver to delete the file the
/// entity used to live in; a deletion needs it to record a tombstone. Both are
/// destructive, and `serve` accepting writes at all is already a larger grant
/// of authority than it had — widening it to deletions in the same change would
/// mean the first version of a remote-write path could also remove the house's
/// config, which is not a thing to get right on the second try.
///
/// Held-back changes get no base (see [`apply`]), so they are offered again on
/// the next sync. The cost, stated plainly: until the push learns to carry
/// them, those two cases are re-offered forever rather than landing. That is
/// the lagging direction `base.rs` calls recoverable.
fn unpushable(change: &PlannedChange) -> Option<&'static str> {
    match (&change.change, change.previous_name.as_deref()) {
        (Change::Delete, _) => Some(
            "a push writes entities and cannot delete one: the house keeps its copy until a \
             later slice carries deletions",
        ),
        (Change::Upsert { .. }, Some(old)) if old != change.name => Some(
            "a push cannot carry a rename: the house would hold the entity under both names",
        ),
        (Change::Upsert { .. }, _) => None,
    }
}

/// The outgoing changes this build is able to push, in plan order.
///
/// See [`unpushable`] for what is left out and why. The caller sends one
/// [`push_message`] per change and then one [`Message::Commit`].
pub fn pushable(plan: &MergePlan) -> Vec<&PlannedChange> {
    plan.outgoing.iter().filter(|c| unpushable(c).is_none()).collect()
}

/// One outgoing change as the `Entity` message that carries it.
///
/// The payload is the plan's own YAML, untouched — the merged entity with the
/// *receiver's* machine-local fields, which is what `merge::plan_kind` already
/// built for the remote side. Re-serializing it here would be a second chance
/// to ship this machine's `path` to a Pi.
pub fn push_message(change: &PlannedChange) -> Result<Message> {
    if let Some(why) = unpushable(change) {
        anyhow::bail!("refusing to push {} '{}': {}", change.kind, change.name, why);
    }
    let Change::Upsert { yaml } = &change.change else {
        unreachable!("unpushable() returns Some for every non-upsert");
    };
    Ok(Message::Entity {
        kind: change.kind.clone(),
        name: change.name.clone(),
        yaml: yaml.clone(),
    })
}

/// One entity a peer pushed at this machine, as it arrived off the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct PushedEntity {
    pub kind: String,
    pub name: String,
    pub yaml: String,
}

/// What a pushed batch did to this machine.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Received {
    /// The keys actually written, in [`push_key`]'s spelling. This is the list
    /// that goes back as [`Message::Applied`], and the only thing the sender is
    /// allowed to record a base from.
    pub landed: Vec<String>,
    /// Why the batch stopped partway, if it did. Not an `Err`, because some of
    /// it *was* written and the sender has to be told which — an `Err` would
    /// throw away the one fact that keeps its bases honest.
    pub problem: Option<String>,
    /// Where `~/.yeehaw` was copied before the first write, if this was the
    /// first batch this machine ever received.
    pub backup: Option<PathBuf>,
}

/// Applies a batch a peer pushed, and reports exactly what landed.
///
/// This is the write path `serve` gained, and the one place the authority to use
/// it is checked. Everything here exists to make "what the acknowledgement says"
/// and "what is on the disk" the same sentence.
///
/// # Only the Ranch House may accept a push
///
/// The roster *is* the namespace and the house is the only machine holding an
/// authoritative copy of it — the same reason `ClaimName` is refused elsewhere.
/// A non-house accepting writes would merge a joining machine's entities into a
/// ranch whose uuids it does not own, against a roster it cannot vouch for.
///
/// The check is here, at the write, rather than at the frame that delivers each
/// entity: buffering is not writing, and a check at the only write path cannot
/// drift from the write it guards. An empty batch is still refused on a
/// non-house, because "I cannot accept writes" is the true answer regardless of
/// how much was offered.
///
/// # The three refusals, all before the first write
///
/// An unparseable payload or unknown kind refuses the **whole** batch rather
/// than writing the rest: a batch this machine cannot fully read means the two
/// sides disagree about the protocol, and a partial write the sender cannot
/// attribute is worse than a clean refusal it can retry. A lock-target
/// collision refuses for the reason [`apply`] gives — two entities claiming one
/// filename writes one over the other. A failed backup refuses because the undo
/// is the point of taking it.
///
/// # Locks
///
/// Every `(kind, name)` in the batch, sorted on `store::lock_key` and deduped
/// on it, acquired before the first write and held until this returns.
/// `base::accept` takes none by design and locking inside it hangs — `fs2`'s
/// locks are neither re-entrant nor timed.
///
/// # The one place this under-reports, and why that is the right way round
///
/// `base::accept` returns a single `Err` whether the *entity* write failed or
/// only the base write after it, so an entity that landed but whose base did not
/// is acknowledged as not landed. The sender therefore keeps no base for it
/// either and re-offers it, where it will be found identical — so that entity
/// merges two-way until something edits it again. A degradation, and the one
/// available in this direction: the alternative reading of an ambiguous `Err`
/// is to claim an entity landed when it may not have, which puts a base *ahead*
/// of a store on the sender. That is the direction `base.rs` calls
/// unrecoverable.
pub fn accept_pushed(batch: &[PushedEntity]) -> Result<Received> {
    if !crate::ranch::this_machine_is_ranch_house() {
        anyhow::bail!(
            "this machine is not the Ranch House, so it cannot accept config pushed at it. \
             The house owns the roster and the uuids a join adopts; a machine holding a copy \
             of them cannot vouch for either. Run `yeehaw ranch init` on the house and join \
             that"
        );
    }

    let mut received = Received::default();
    if batch.is_empty() {
        return Ok(received);
    }

    // Parsed completely first, so an unreadable payload is a refusal rather
    // than a half-written batch — and so the key can be taken from the entity
    // *before* a save stamps it.
    let mut parsed: Vec<(String, Parsed)> = Vec::with_capacity(batch.len());
    for entity in batch {
        let one = Parsed::read(&entity.kind, &entity.name, &entity.yaml)?;
        parsed.push((push_key(one.kind(), one.id(), one.name()), one));
    }

    // Same rule as `apply`'s, for the same reason, on the machine that is about
    // to do the writing: two different entities claiming one lock key collapse
    // to one lock and one file, and applying both writes one on top of the
    // other. "Different entity" is `(name, id)` — two names that share a lock
    // key are two entities however their ids read.
    let mut claims: std::collections::BTreeMap<
        (String, String),
        BTreeSet<(String, Option<String>)>,
    > = std::collections::BTreeMap::new();
    for (_, one) in &parsed {
        claims
            .entry((one.kind().to_string(), crate::store::lock_key(one.name())))
            .or_default()
            .insert((one.name().to_string(), one.id().map(|s| s.to_string())));
    }
    let collisions: Vec<String> = claims
        .into_iter()
        .filter(|(_, claimants)| claimants.len() > 1)
        .map(|((kind, _), claimants)| {
            let names: Vec<String> = claimants.into_iter().map(|(name, _)| name).collect();
            format!("  {} {}", kind, names.join(" / "))
        })
        .collect();
    if !collisions.is_empty() {
        anyhow::bail!(
            "refusing the pushed batch: {} name(s) are claimed by two different entities, and \
             writing both puts one on top of the other.\n{}\n\
             Rename one side and sync again.",
            collisions.len(),
            collisions.join("\n")
        );
    }

    // The undo, before the first write and after every refusal: this machine is
    // having another machine's config merged into it, which is the same event
    // the joining side takes a backup for, and a batch that is about to be
    // refused must not leave a gigabyte of copy behind.
    received.backup = backup_once()?;

    let mut targets: Vec<(String, String, String)> = parsed
        .iter()
        .map(|(_, one)| {
            (
                one.kind().to_string(),
                crate::store::lock_key(one.name()),
                one.name().to_string(),
            )
        })
        .collect();
    targets.sort();
    targets.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

    let mut locks = Vec::with_capacity(targets.len());
    for (kind, _, name) in &targets {
        locks.push(crate::store::lock_entity(&dir_for(kind)?, name)?);
    }

    let offered = parsed.len();
    for (key, one) in &mut parsed {
        match one.accept() {
            Ok(()) => received.landed.push(key.clone()),
            Err(e) => {
                // Stop rather than press on. Whatever failed the first write —
                // a full disk, a directory replaced by a file — will fail the
                // rest, and every extra attempt is another chance to write
                // something the acknowledgement then has to account for.
                received.problem = Some(format!(
                    "stopped after {} of {} pushed entities: {} '{}' could not be written: {:#}",
                    received.landed.len(),
                    offered,
                    one.kind(),
                    one.name(),
                    e
                ));
                break;
            }
        }
    }

    drop(locks);
    Ok(received)
}

/// A pushed entity parsed into its kind.
///
/// Exists so the batch can be validated, keyed and collision-checked before a
/// single lock is taken, without parsing the same YAML twice and risking the
/// validation pass and the write pass disagreeing about what it said.
enum Parsed {
    Project(Project),
    Barn(Barn),
    Worm(Worm),
    Trail(crate::trails::Trail),
    RanchHand(RanchHand),
}

impl Parsed {
    fn read(kind: &str, name: &str, yaml: &str) -> Result<Self> {
        let what = || format!("the peer pushed a {} '{}' that did not parse", kind, name);
        Ok(match kind {
            "project" => Parsed::Project(serde_yaml::from_str(yaml).with_context(what)?),
            "barn" => Parsed::Barn(serde_yaml::from_str(yaml).with_context(what)?),
            "worm" => Parsed::Worm(serde_yaml::from_str(yaml).with_context(what)?),
            "trail" => Parsed::Trail(serde_yaml::from_str(yaml).with_context(what)?),
            "ranchhand" => Parsed::RanchHand(serde_yaml::from_str(yaml).with_context(what)?),
            other => anyhow::bail!("the peer pushed an entity of unknown kind '{}'", other),
        })
    }

    fn kind(&self) -> &'static str {
        match self {
            Parsed::Project(_) => "project",
            Parsed::Barn(_) => "barn",
            Parsed::Worm(_) => "worm",
            Parsed::Trail(_) => "trail",
            Parsed::RanchHand(_) => "ranchhand",
        }
    }

    /// The entity's own name, read back out of the payload rather than taken
    /// from the `Entity` message's `name` field: the file this writes is named
    /// from the struct, so the lock has to be too.
    fn name(&self) -> &str {
        match self {
            Parsed::Project(e) => &e.name,
            Parsed::Barn(e) => &e.name,
            Parsed::Worm(e) => &e.name,
            Parsed::Trail(e) => &e.name,
            Parsed::RanchHand(e) => &e.name,
        }
    }

    /// The id **as it arrived**. Read before [`Parsed::accept`] stamps an
    /// unstamped entity, because the key the sender is waiting to hear back is
    /// the one it sent, not the uuid this machine happened to mint.
    fn id(&self) -> Option<&str> {
        match self {
            Parsed::Project(e) => e.id(),
            Parsed::Barn(e) => e.id(),
            Parsed::Worm(e) => e.id(),
            Parsed::Trail(e) => e.id(),
            Parsed::RanchHand(e) => e.id(),
        }
    }

    /// Writes it and records this machine's own base for it, in that order.
    ///
    /// `base::accept` is the only base writer on either side of the wire. The
    /// receiver's base is its own business — it describes what *this* machine
    /// now holds — and has nothing to do with the sender's, which is gated on
    /// the acknowledgement this write earns.
    fn accept(&mut self) -> Result<()> {
        match self {
            Parsed::Project(e) => base::accept("project", e, |e| config::save_project(e)),
            Parsed::Barn(e) => base::accept("barn", e, |e| config::save_barn(e)),
            Parsed::Worm(e) => base::accept("worm", e, |e| config::save_worm(e)),
            Parsed::Trail(e) => base::accept("trail", e, |e| config::save_trail(e)),
            Parsed::RanchHand(e) => base::accept("ranchhand", e, |e| config::save_ranchhand(e)),
        }
    }
}

/// A rendered YAML value squashed onto one line, for a list the user scans.
fn one_line(value: &str) -> String {
    let squashed: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() <= 100 {
        return squashed;
    }
    let cut: String = squashed.chars().take(100).collect();
    format!("{}…", cut)
}

/// The entity directory one kind lives in — where its lock files are.
fn dir_for(kind: &str) -> Result<PathBuf> {
    Ok(match kind {
        "project" => config::projects_dir(),
        "barn" => config::barns_dir(),
        "worm" => config::worms_dir(),
        "trail" => config::trails_dir(),
        "ranchhand" => config::ranchhands_dir(),
        other => anyhow::bail!("no entity directory for kind '{}'", other),
    })
}

/// What an [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Applied {
    pub written: usize,
    pub deleted: usize,
    /// Outgoing entities the house confirmed, and which therefore earned a sync
    /// base here. Never larger than what was pushed, and smaller whenever the
    /// house wrote some of a batch and not the rest.
    pub based: usize,
}

/// Writes the incoming half of a plan and bases the acknowledged outgoing half,
/// holding every lock the plan touches.
///
/// # `landed` is the house's acknowledgement, and it is the whole gate
///
/// The keys — in [`push_key`]'s spelling — that the house said it **actually
/// wrote**, from the [`Message::Applied`] answering the [`Message::Commit`]. An
/// outgoing entity gets a sync base recorded here if and only if its key is in
/// that list. Pass an empty slice for a plan that was never pushed, or whose
/// push failed, and no outgoing base is written at all.
///
/// That gate is the reason this argument exists rather than the push recording
/// its own bases as it sends. A base written at send time *leads* the store the
/// moment the send fails — it claims the house holds something it does not, so
/// the next merge reads this machine's real local entity as already-synced and
/// drops it, which `base.rs` names as the one unrecoverable direction. Gating
/// on the acknowledgement puts the error the other way round: an entity whose
/// ack is lost keeps no base, is re-offered next sync, and the house finds it
/// identical. Nothing is lost by offering twice.
///
/// Both halves' base writes live here, in one lock batch, because the locks are
/// the reason they cannot live in two places: `MergePlan::lock_targets` already
/// covers `outgoing`, and taking that lock twice in one thread is an
/// unbreakable hang.
///
/// An outgoing entity that is **unstamped** is written on the house (which
/// mints a uuid of its own for it) but gets no base here: there is no id to
/// file one under, and `base::accept` refuses rather than invent one. It is
/// re-offered next sync, matched by name, and adopts the house's uuid — which is
/// the ordinary first-join mechanism, arriving one sync late.
///
/// # The two refusals, both before the first write
///
/// 1. **A lock-target collision.** Two different entities claiming one filename
///    means applying both writes one over the other. `merge::plan_kind` never
///    emits such a plan — it turns the clash into a conflict and emits neither
///    change — so this is the guard for a plan that came from somewhere else,
///    and `MergePlan::lock_targets`' mandatory dedupe is exactly why it cannot
///    be noticed any later than here.
/// 2. **An unknown kind.** Checked while building the lock list, so a typo
///    cannot be discovered with half the entities already written.
///
/// # Locks
///
/// Every target in the plan, in [`MergePlan::lock_targets`]' order — sorted on
/// `store::lock_key` and deduped on it — acquired **before the first write** and
/// held until this returns. `base::accept` takes none by design and documents
/// that the caller holds them for the whole batch; taking them inside it is the
/// shape that hangs, because `fs2` locks are neither re-entrant nor timed.
///
/// Outgoing targets are locked too — `lock_targets` includes them, and the
/// outgoing base writes below are exactly why.
pub fn apply(plan: &MergePlan, landed: &[String]) -> Result<Applied> {
    let collisions = plan.lock_target_collisions();
    if !collisions.is_empty() {
        let detail = collisions
            .iter()
            .map(|(kind, name)| format!("  {} {}", kind, name))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "refusing to apply: {} filename(s) are claimed by two different entities, and \
             writing both puts one on top of the other.\n{}\n\
             Rename one side and sync again.",
            collisions.len(),
            detail
        );
    }

    // Built first, and completely, so an unknown kind is a refusal rather than a
    // half-applied plan.
    let targets = plan.lock_targets();
    let mut dirs: Vec<(PathBuf, String)> = Vec::with_capacity(targets.len());
    for (kind, name) in &targets {
        dirs.push((dir_for(kind)?, name.clone()));
    }

    let mut locks = Vec::with_capacity(dirs.len());
    for (dir, name) in &dirs {
        locks.push(crate::store::lock_entity(dir, name)?);
    }

    let mut applied = Applied::default();
    for change in &plan.incoming {
        match &change.change {
            Change::Upsert { .. } => {
                write_one(change)?;
                applied.written += 1;
            }
            Change::Delete => {
                if delete_one(change)? {
                    applied.deleted += 1;
                }
            }
        }
    }

    // The outgoing half. Nothing is *written to the store* here — the house did
    // that — so all that is recorded is the base, and only for the keys the
    // house named back.
    for change in &plan.outgoing {
        if unpushable(change).is_some() {
            continue;
        }
        let key = push_key(&change.kind, change.id.as_deref(), &change.name);
        if !landed.iter().any(|k| k == &key) {
            continue;
        }
        if base_one(change)? {
            applied.based += 1;
        }
    }

    drop(locks);
    Ok(applied)
}

/// Records the sync base for one outgoing entity the house confirmed.
///
/// `false` for an entity with no uuid: it landed on the house, which minted one
/// of its own for it, and a base cannot be filed under an id this machine does
/// not have. See [`apply`] for why that is the safe direction.
///
/// The "apply" handed to `base::accept` is a no-op, and that is not a
/// workaround. `base::accept`'s contract is that the base is recorded only from
/// what a successful apply left behind, and for an outgoing entity the apply
/// happened on the far side — the house's acknowledgement is the evidence, which
/// is why reaching this function at all is gated on it. What is snapshotted is
/// the entity as it crossed the wire: its content is what both machines now
/// hold, and the machine-local fields it carries are the house's, which no merge
/// ever reads out of a base (`Mergeable::merge_content` takes them from `ours`
/// and never from the ancestor).
fn base_one(change: &PlannedChange) -> Result<bool> {
    if change.id.is_none() {
        return Ok(false);
    }
    let kind = change.kind.as_str();
    match kind {
        "project" => base::accept(kind, &mut change.entity::<Project>()?, |_| Ok(()))?,
        "barn" => base::accept(kind, &mut change.entity::<Barn>()?, |_| Ok(()))?,
        "worm" => base::accept(kind, &mut change.entity::<Worm>()?, |_| Ok(()))?,
        "trail" => base::accept(kind, &mut change.entity::<crate::trails::Trail>()?, |_| Ok(()))?,
        "ranchhand" => base::accept(kind, &mut change.entity::<RanchHand>()?, |_| Ok(()))?,
        other => anyhow::bail!("cannot base an entity of unknown kind '{}'", other),
    }
    Ok(true)
}

/// One incoming upsert: apply it and record its base, in that order.
///
/// The rename half is done here rather than through `config::rename_project`,
/// which takes the very locks [`apply`] is already holding — calling it would
/// block the thread against itself forever. So: write under the new name, then
/// remove the file the entity used to live in. No tombstone: a rename is not a
/// deletion, and recording one would tell every other machine to throw the
/// entity away.
fn write_one(change: &PlannedChange) -> Result<()> {
    let kind = change.kind.as_str();
    match kind {
        "project" => {
            let mut e: Project = change.entity()?;
            base::accept(kind, &mut e, |e| config::save_project(e))?;
        }
        "barn" => {
            let mut e: Barn = change.entity()?;
            base::accept(kind, &mut e, |e| config::save_barn(e))?;
        }
        "worm" => {
            let mut e: Worm = change.entity()?;
            base::accept(kind, &mut e, |e| config::save_worm(e))?;
        }
        "trail" => {
            let mut e: crate::trails::Trail = change.entity()?;
            base::accept(kind, &mut e, |e| config::save_trail(e))?;
        }
        "ranchhand" => {
            let mut e: RanchHand = change.entity()?;
            base::accept(kind, &mut e, |e| config::save_ranchhand(e))?;
        }
        other => anyhow::bail!("cannot write an entity of unknown kind '{}'", other),
    }

    if let Some(old) = change.previous_name.as_deref() {
        if old != change.name {
            let path = dir_for(kind)?.join(format!("{}.yaml", old));
            if path.exists() {
                std::fs::remove_file(&path).with_context(|| {
                    format!(
                        "the merged {} was written as '{}' but its old file at {} could not be \
                         removed, so the ranch now holds it under both names",
                        kind,
                        change.name,
                        path.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

/// One incoming deletion, through the `config` deleter for its kind.
///
/// The deleters record a tombstone of their own, which is what carries the
/// deletion on to a third machine, and none of them takes a lock — so calling
/// them under [`apply`]'s locks is safe. `false` means the file was already
/// gone, which is not a failure.
fn delete_one(change: &PlannedChange) -> Result<bool> {
    match change.kind.as_str() {
        "project" => config::delete_project(&change.name),
        "barn" => config::delete_barn(&change.name),
        "worm" => config::delete_worm(&change.name),
        "trail" => config::delete_trail(&change.name),
        "ranchhand" => config::delete_ranchhand(&change.name),
        other => anyhow::bail!("cannot delete an entity of unknown kind '{}'", other),
    }
}

/// Every brand known on this ranch, this machine's own first.
///
/// The set that goes into a barn's managed `authorized_keys` block: every
/// machine on the ranch has to be able to reach every barn, which is the whole
/// point of the brand. `brand::rewrite_block` filters and dedupes what it is
/// given, so a barn with no brand yet, or two barns sharing one, cost nothing
/// here.
pub fn known_brands(ours: Option<&str>, barns: &[Barn]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for key in ours.into_iter().chain(barns.iter().filter_map(|b| b.brand.as_deref())) {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if seen.insert(key.to_string()) {
            out.push(key.to_string());
        }
    }
    out
}

/// Copies `~/.yeehaw` aside, once, before the first join ever writes to it.
///
/// Returns the backup's path, or `None` when one already exists — which is what
/// makes this "once, on the first join" rather than "on every join". The check
/// is for an existing backup rather than for a flag in the config, because the
/// thing that must be true is that *a* pre-ranch copy survives: a flag saying
/// one was taken is not the same statement as one being there.
///
/// # What is copied, and what cannot be
///
/// Regular files and directories. Everything else is skipped and counted:
/// `~/.yeehaw/ssh` holds ssh `ControlPath` **sockets**, and `fs::copy` on a unix
/// socket fails — so a naive recursive copy turns "back up the ranch" into "the
/// join refused because a multiplexed ssh session was open". Nothing that is not
/// a regular file is config, so none of it belongs in an undo anyway.
///
/// Symlinks are copied as their targets' contents, not recreated as links. That
/// is the conservative direction for a backup: a dangling link restores as
/// nothing, and a link into the ranch restores as a second copy rather than as a
/// loop.
pub fn backup_once() -> Result<Option<PathBuf>> {
    let ranch = config::yeehaw_dir();
    let Some(parent) = ranch.parent().map(|p| p.to_path_buf()) else {
        anyhow::bail!("cannot back up {}: it has no parent directory", ranch.display());
    };
    let stem = ranch
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".yeehaw".to_string());
    let prefix = format!("{}.pre-ranch-", stem);

    if let Ok(entries) = std::fs::read_dir(&parent) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                return Ok(None);
            }
        }
    }

    // Seconds, and no colons: this is a directory name a user types at a shell.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let target = parent.join(format!("{}{}", prefix, stamp));

    copy_tree(&ranch, &target)
        .with_context(|| format!("failed to back up {} to {}", ranch.display(), target.display()))?;
    Ok(Some(target))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)
        .with_context(|| format!("failed to create {}", to.display()))?;
    for entry in std::fs::read_dir(from)
        .with_context(|| format!("failed to read {}", from.display()))?
        .flatten()
    {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        // `metadata`, not `symlink_metadata`: a symlink to a file is copied as a
        // file, which is what a restorable backup wants.
        let Ok(meta) = std::fs::metadata(&src) else {
            continue; // a dangling symlink, or a file deleted under us
        };
        if meta.is_dir() {
            copy_tree(&src, &dst)?;
        } else if meta.is_file() {
            std::fs::copy(&src, &dst)
                .with_context(|| format!("failed to copy {}", src.display()))?;
        }
        // Anything else — a socket under `ssh/`, a fifo — is not config and
        // cannot be copied. Skipped deliberately; see the doc comment.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// How many entities a ranch holds, across every kind. A test helper rather
    /// than a method on `Ranch`: nothing in production counts them, and a method
    /// only tests call is a method that drifts from what the code does.
    fn count(ranch: &Ranch) -> usize {
        ranch.projects.len()
            + ranch.barns.len()
            + ranch.worms.len()
            + ranch.trails.len()
            + ranch.ranchhands.len()
    }

    /// Built exhaustively: `Project` has no `Default`, so a new field fails to
    /// compile here instead of being silently defaulted.
    fn project(name: &str) -> Project {
        Project {
            name: name.into(),
            path: format!("/tmp/{}", name),
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

    // ---- load_local --------------------------------------------------------

    #[test]
    fn a_fresh_ranch_loads_as_empty_rather_than_failing() {
        let _ranch = testing::temp_ranch();
        let loaded = load_local().expect("an empty ranch is a legitimate ranch");
        assert_eq!(count(&loaded), 0, "nothing was created, so nothing should load: {:?}", loaded);
    }

    #[test]
    fn every_kind_on_disk_is_loaded() {
        let _ranch = testing::temp_ranch();
        config::save_project(&mut project("api")).unwrap();
        config::save_barn(&mut Barn { name: "pi".into(), ..Default::default() }).unwrap();
        let loaded = load_local().unwrap();
        assert_eq!(loaded.projects.len(), 1, "{:?}", loaded);
        assert_eq!(loaded.barns.len(), 1, "{:?}", loaded);
        assert_eq!(count(&loaded), 2, "{:?}", loaded);
    }

    /// The synthetic `local` barn is injected by `config::load_barns()` into
    /// every listing and is not a real entity. Offering it to a peer plants a
    /// barn neither machine can display.
    #[test]
    fn the_synthetic_local_barn_is_never_loaded() {
        let _ranch = testing::temp_ranch();
        let loaded = load_local().unwrap();
        assert!(
            loaded.barns.iter().all(|b| b.name != config::LOCAL_BARN_NAME),
            "the synthetic barn must not reach a peer: {:?}",
            loaded.barns
        );
    }

    /// Fails rather than shrinks. A manifest short one entity reads to the peer
    /// as "they do not have it", and the merge then offers to create its copy
    /// over the top of the file we could not read.
    #[test]
    fn an_unreadable_entity_refuses_the_load_and_names_the_file() {
        let ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        std::fs::write(ranch.dir.path().join("projects").join("broken.yaml"), "{[not yaml")
            .unwrap();

        let why = format!("{:#}", load_local().expect_err("an unreadable entity must refuse"));
        assert!(why.contains("broken.yaml"), "the refusal must name the file: {}", why);
    }

    // ---- want keys and entity lookup ---------------------------------------

    #[test]
    fn a_want_key_prefers_the_uuid_and_falls_back_to_the_name() {
        let entries = vec![
            ManifestEntry {
                kind: "project".into(),
                name: "api".into(),
                id: Some("abc".into()),
                updated_at: None,
                hash: String::new(),
            },
            ManifestEntry {
                kind: "worm".into(),
                name: "nightly".into(),
                id: None,
                updated_at: None,
                hash: String::new(),
            },
        ];
        assert_eq!(want_keys(&entries), vec!["project/abc", "worm/nightly"]);
    }

    /// Every entry, including the ones whose hash matches. On a first join the
    /// two machines minted their uuids independently, so an identical entity
    /// still has to be merged in order to adopt the house's id — and an entity
    /// that is never asked for is never merged.
    #[test]
    fn identical_entities_are_still_asked_for() {
        let entries = vec![ManifestEntry {
            kind: "project".into(),
            name: "api".into(),
            id: Some("abc".into()),
            updated_at: None,
            hash: "same".into(),
        }];
        assert_eq!(want_keys(&entries).len(), 1, "a matching hash is not a reason to skip");
    }

    #[test]
    fn an_entity_is_served_by_id_or_by_name() {
        let mut ranch = Ranch::default();
        let mut p = project("api");
        p.id = Some("the-uuid".into());
        ranch.projects.push(p);

        for key in ["project/the-uuid", "project/api"] {
            match entity_for_key(&ranch, key) {
                Some(Message::Entity { kind, name, yaml }) => {
                    assert_eq!(kind, "project");
                    assert_eq!(name, "api");
                    assert!(yaml.contains("name: api"), "{}", yaml);
                }
                other => panic!("{} should resolve, got {:?}", key, other),
            }
        }
    }

    /// A uuid match must beat a name match, not merely be tried alongside one.
    ///
    /// MEASURED: asserting only that an unknown uuid resolves to `None` has no
    /// teeth — a name-first implementation passes it, because nothing is named
    /// like a uuid. The case that separates the two orders is a ranch holding an
    /// entity whose *name* is another entity's *id*, and then the wrong answer is
    /// a peer writing one project's contents over the other's.
    #[test]
    fn an_id_match_beats_a_name_match_rather_than_merely_being_tried_too() {
        let mut ranch = Ranch::default();
        let mut holds_the_id = project("api");
        holds_the_id.id = Some("collides".into());
        // A project a user happened to name the same as the other's uuid.
        let mut named_like_an_id = project("collides");
        named_like_an_id.id = Some("something-else".into());
        ranch.projects.push(named_like_an_id);
        ranch.projects.push(holds_the_id);

        match entity_for_key(&ranch, "project/collides") {
            Some(Message::Entity { name, .. }) => assert_eq!(
                name, "api",
                "the key names a uuid, so the entity holding that uuid is the answer"
            ),
            other => panic!("expected the entity holding the uuid, got {:?}", other),
        }

        assert!(
            entity_for_key(&ranch, "project/no-such-uuid").is_none(),
            "a key matching nothing resolves to nothing"
        );
    }

    #[test]
    fn an_unknown_kind_resolves_to_nothing_rather_than_panicking() {
        assert!(entity_for_key(&Ranch::default(), "lease/x").is_none());
        assert!(entity_for_key(&Ranch::default(), "no-slash").is_none());
    }

    // ---- absorb ------------------------------------------------------------

    #[test]
    fn a_received_entity_that_will_not_parse_is_an_error_not_a_skip() {
        let mut ranch = Ranch::default();
        let why = format!(
            "{:#}",
            absorb_entity(&mut ranch, "project", "api", "{[nope")
                .expect_err("a dropped entity becomes an offer to create it")
        );
        assert!(why.contains("api"), "the error must name the entity: {}", why);
        assert!(ranch.projects.is_empty(), "nothing must be filed from a failed parse");
    }

    // ---- known_brands ------------------------------------------------------

    #[test]
    fn brands_are_collected_with_this_machines_first_and_duplicates_collapsed() {
        let barns = vec![
            Barn { name: "pi".into(), brand: Some("ssh-ed25519 PI".into()), ..Default::default() },
            Barn { name: "nb".into(), brand: None, ..Default::default() },
            Barn {
                name: "dup".into(),
                brand: Some("ssh-ed25519 MINE".into()),
                ..Default::default()
            },
        ];
        assert_eq!(
            known_brands(Some("ssh-ed25519 MINE"), &barns),
            vec!["ssh-ed25519 MINE", "ssh-ed25519 PI"]
        );
    }

    // ---- the backup --------------------------------------------------------

    /// The undo for the one run that merges months of accumulated config.
    #[test]
    fn the_first_backup_copies_the_ranch_and_the_second_does_not_run() {
        let ranch = testing::temp_ranch();
        config::save_project(&mut project("api")).unwrap();

        let first = backup_once().unwrap().expect("the first join must leave an undo");
        assert!(
            first.join("projects").join("api.yaml").exists(),
            "the backup must contain the entities: {}",
            first.display()
        );
        assert!(
            first.file_name().unwrap().to_string_lossy().starts_with(".yeehaw.pre-ranch-")
                || first
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".pre-ranch-"),
            "the backup must be named so a user can find it: {}",
            first.display()
        );

        // Proof the second call is a no-op rather than a second copy: a file
        // created after the first backup must not appear in any backup.
        config::save_project(&mut project("later")).unwrap();
        assert_eq!(backup_once().unwrap(), None, "the backup is taken once, on the first join");
        assert!(
            !first.join("projects").join("later.yaml").exists(),
            "the existing backup must not be rewritten"
        );
        let _ = ranch;
    }

    /// `~/.yeehaw/ssh` holds ssh `ControlPath` sockets. `fs::copy` on a socket
    /// fails, so a naive recursive copy turns an open multiplexed ssh session
    /// into a refused join.
    #[cfg(unix)]
    #[test]
    fn a_socket_in_the_ranch_does_not_fail_the_backup() {
        let ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        let sock = ranch.dir.path().join("ssh").join("cam@pi:22");
        std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&sock)
            .expect("a unix socket is what ssh multiplexing leaves here");

        let backup = backup_once()
            .expect("a socket must not fail the backup")
            .expect("a backup must still be taken");
        assert!(backup.join("ssh").exists(), "the directory is still copied");
        assert!(
            !backup.join("ssh").join("cam@pi:22").exists(),
            "a socket is not config and cannot be copied"
        );
    }

    // ---- rendering ---------------------------------------------------------

    /// DELIBERATE CHANGE, the push slice. This used to require the words "does
    /// not push", which were true and are now a lie. What replaces them is the
    /// distinction that matters to a user consenting to a write on a machine
    /// they are not sitting at: what goes to the house, and what is held back.
    #[test]
    fn the_rendered_plan_names_both_halves_and_who_won_a_conflict() {
        let plan = MergePlan {
            incoming: vec![PlannedChange {
                kind: "project".into(),
                name: "api".into(),
                id: Some("a".into()),
                previous_name: None,
                change: Change::Upsert { yaml: "name: api\n".into() },
            }],
            outgoing: vec![
                PlannedChange {
                    kind: "worm".into(),
                    name: "nightly".into(),
                    id: None,
                    previous_name: None,
                    change: Change::Upsert { yaml: "name: nightly\n".into() },
                },
                // Held back: a push writes entities and cannot remove one.
                PlannedChange {
                    kind: "trail".into(),
                    name: "deploy".into(),
                    id: Some("t1".into()),
                    previous_name: None,
                    change: Change::Delete,
                },
            ],
            conflicts: vec![merge::Conflict {
                kind: "project".into(),
                name: "api".into(),
                id: Some("a".into()),
                field: "summary".into(),
                ours: "mine".into(),
                theirs: "theirs".into(),
                winner: Side::Remote,
            }],
            notes: vec![merge::Note {
                kind: "barn".into(),
                name: "pi".into(),
                id: None,
                reason: "base unreadable".into(),
            }],
        };

        let text = render_plan(&plan, "imac");
        for expected in [
            "imac",
            "Incoming",
            "project api",
            "Outgoing — pushed to the house 'imac' (1)",
            "worm nightly",
            "held back",
            "delete trail deploy",
            "cannot delete one",
            "offered\n",
            "Conflicts",
            "summary",
            "the house",
            "Notes",
            "base unreadable",
        ] {
            assert!(text.contains(expected), "the plan must mention {:?}:\n{}", expected, text);
        }
    }

    // ---- apply -------------------------------------------------------------

    fn upsert(kind: &str, name: &str, yaml: &str, id: Option<&str>) -> PlannedChange {
        PlannedChange {
            kind: kind.into(),
            name: name.into(),
            id: id.map(str::to_string),
            previous_name: None,
            change: Change::Upsert { yaml: yaml.into() },
        }
    }

    #[test]
    fn applying_an_upsert_writes_the_entity_and_records_its_base() {
        let _ranch = testing::temp_ranch();
        let plan = MergePlan {
            incoming: vec![upsert(
                "project",
                "api",
                "name: api\npath: /tmp/api\nid: the-uuid\n",
                Some("the-uuid"),
            )],
            ..Default::default()
        };

        assert_eq!(apply(&plan, &[]).unwrap(), Applied { written: 1, deleted: 0, based: 0 });
        assert_eq!(config::load_projects().len(), 1);
        assert!(
            base::load::<Project>("project", "the-uuid").unwrap().is_some(),
            "an applied entity must have a base, or the next merge has no ancestor"
        );
    }

    /// Two entities claiming one filename is silent data loss: applying both
    /// writes one over the other. `plan_kind` never emits such a plan; this is
    /// the guard for one assembled anywhere else.
    #[test]
    fn a_plan_that_writes_two_entities_to_one_filename_is_refused_before_any_write() {
        let _ranch = testing::temp_ranch();
        let plan = MergePlan {
            incoming: vec![
                upsert("project", "my api", "name: my api\npath: /tmp/a\n", Some("one")),
                upsert("project", "my_api", "name: my_api\npath: /tmp/b\n", Some("two")),
            ],
            ..Default::default()
        };

        let why = format!("{:#}", apply(&plan, &[]).expect_err("one filename, two entities"));
        assert!(why.contains("two different entities"), "{}", why);
        assert!(
            config::load_projects().is_empty(),
            "the refusal must come before the first write"
        );
    }

    /// `base::accept` takes no locks and documents that the caller holds them for
    /// the whole batch. Taking one key twice in a thread is an unbreakable hang,
    /// because `fs2` locks are neither re-entrant nor timed — so a plan touching
    /// two names that share a lock key must still apply.
    #[test]
    fn a_plan_touching_two_kinds_and_a_rename_applies_without_locking_itself() {
        let _ranch = testing::temp_ranch();
        config::save_project(&mut project("old-name")).unwrap();

        let plan = MergePlan {
            incoming: vec![
                PlannedChange {
                    kind: "project".into(),
                    name: "new-name".into(),
                    id: Some("p1".into()),
                    previous_name: Some("old-name".into()),
                    change: Change::Upsert {
                        yaml: "name: new-name\npath: /tmp/x\nid: p1\n".into(),
                    },
                },
                upsert("barn", "pi", "name: pi\nid: b1\n", Some("b1")),
            ],
            ..Default::default()
        };

        assert_eq!(apply(&plan, &[]).unwrap(), Applied { written: 2, deleted: 0, based: 0 });
        let names: Vec<String> = config::load_projects().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["new-name"], "a rename must not leave the old file behind");
    }

    /// Nothing outgoing is written, and — the part that matters — no base is
    /// recorded for it. A base ahead of a push that never happened makes the next
    /// sync read a real local entity as already-synced and drop it silently.
    #[test]
    fn an_outgoing_change_is_neither_written_nor_given_a_base() {
        let _ranch = testing::temp_ranch();
        let plan = MergePlan {
            outgoing: vec![upsert(
                "project",
                "ours-only",
                "name: ours-only\npath: /tmp/o\nid: out-uuid\n",
                Some("out-uuid"),
            )],
            ..Default::default()
        };

        assert_eq!(apply(&plan, &[]).unwrap(), Applied::default());
        assert!(
            base::load::<Project>("project", "out-uuid").unwrap().is_none(),
            "a base for an unsent entity claims a sync that never happened"
        );
    }

    #[test]
    fn applying_a_deletion_removes_the_entity() {
        let _ranch = testing::temp_ranch();
        config::save_project(&mut project("doomed")).unwrap();

        let plan = MergePlan {
            incoming: vec![PlannedChange {
                kind: "project".into(),
                name: "doomed".into(),
                id: None,
                previous_name: None,
                change: Change::Delete,
            }],
            ..Default::default()
        };
        assert_eq!(apply(&plan, &[]).unwrap(), Applied { written: 0, deleted: 1, based: 0 });
        assert!(config::load_projects().is_empty());
    }
}
