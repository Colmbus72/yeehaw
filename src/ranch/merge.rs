//! The three-way merge.
//!
//! Pure functions over parsed structs. No I/O, no network, no process
//! spawning — everything this module needs is handed to it, and everything it
//! produces is a [`MergePlan`] for somebody else to apply. That is deliberate:
//! merge correctness is where a sync actually loses data, so it has to be
//! exhaustively testable without a second machine on the other end of an ssh.
//!
//! # Parsed structs, never YAML text
//!
//! A text-level merge would be shorter and would be wrong. `Project` carries
//! four `#[serde(rename)]` fields — `gradientSpread`, `gradientInverted`,
//! `issueProvider`, `wikiProvider` — `RanchHand.rh_type` and `Worm.worm_type`
//! both serialize as `type`, and `Trail` renames to kebab-case. A text merge
//! sees the wire spelling while the code that decides what to do with a field
//! sees the Rust spelling, so the four renamed `Project` fields would silently
//! never merge and nothing would fail: the entity would round-trip, the hash
//! would change, and one side's colour scheme would quietly win forever.
//!
//! So: merge the structs, and serialize once at the end.

#![allow(dead_code)] // Wired up in Slice E.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::ranch::wire::WireTombstone;
use crate::types::Identified;

// ============================================================================
// C1 — outcome types
// ============================================================================

/// Which machine holds the canonical set.
///
/// The Ranch House wins every collision the merge cannot resolve. That is a
/// property of the *machine*, not of which argument slot an entity arrived in,
/// so it is passed in rather than inferred — `merge(ours, theirs)` and
/// `merge(theirs, ours)` must agree, and they only can if the tie-break follows
/// the house.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// This machine.
    Local,
    /// The peer on the other end of the sync.
    Remote,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Local => Side::Remote,
            Side::Remote => Side::Local,
        }
    }
}

/// The common ancestor for one entity — the third leg of the three-way merge.
///
/// Three states, not two, and the third is the point. `base::load` returns
/// `Err` when a base file exists but will not parse, precisely so a corrupt
/// ancestor is never read as an absent one. If the sync loop threaded that
/// `Err` out through `?`, one unparseable file under `.ranch/base/` would block
/// every other entity on the ranch from syncing, and the user's only signal
/// would be a sync that had quietly stopped working.
///
/// Making it a *value* rather than an error is what isolates it: the caller is
/// forced to turn each `base::load` result into one of these — [`from_load`]
/// is the one-liner — and a failure lands against that entity, degrades that
/// entity to a two-way merge, and is carried into the plan as a [`Note`] the
/// user sees. Nothing else on the ranch notices.
///
/// [`from_load`]: Ancestor::from_load
#[derive(Debug, Clone, PartialEq)]
pub enum Ancestor<T> {
    /// A base snapshot exists. Merge three-way.
    Known(T),
    /// Never synced. No ancestor, so merge two-way — the normal state of
    /// everything created since the last sync.
    Absent,
    /// A base exists but could not be read. Merge two-way and say so.
    Unreadable(String),
}

impl<T> Ancestor<T> {
    /// Turns one `base::load` result into an ancestor.
    ///
    /// This is the entire corrupt-base isolation story, and it is one line at
    /// the call site:
    ///
    /// ```ignore
    /// let ancestor = Ancestor::from_load(base::load::<Project>("project", id));
    /// ```
    ///
    /// Written as a constructor rather than left to each caller because the
    /// tempting alternative — `base::load(..)?` inside the loop — is the exact
    /// shape that turns one bad file into a dead sync.
    pub fn from_load(loaded: Result<Option<T>>) -> Self {
        match loaded {
            Ok(Some(entity)) => Ancestor::Known(entity),
            Ok(None) => Ancestor::Absent,
            // `{:#}` renders the whole anyhow context chain, which is where
            // `base::load` puts the path and the "delete the file" remedy.
            Err(e) => Ancestor::Unreadable(format!("{:#}", e)),
        }
    }

    pub fn known(&self) -> Option<&T> {
        match self {
            Ancestor::Known(entity) => Some(entity),
            _ => None,
        }
    }

    pub fn unreadable(&self) -> Option<&str> {
        match self {
            Ancestor::Unreadable(why) => Some(why),
            _ => None,
        }
    }
}

/// What a plan does to one entity on one machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Write this entity. The payload is the merged struct serialized once, at
    /// the end — the merge itself never touched text.
    Upsert { yaml: String },
    /// Remove this entity: the other side deleted it and this side has not
    /// touched it since.
    Delete,
}

/// One entity's worth of work, on one machine.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedChange {
    pub kind: String,
    /// The name the entity will have *after* the change.
    pub name: String,
    pub id: Option<String>,
    /// Set when the merge renamed the entity, because the store keys files by
    /// name: applying this touches two filenames, and both have to be locked.
    pub previous_name: Option<String>,
    pub change: Change,
}

impl PlannedChange {
    /// The merged entity, parsed back out.
    ///
    /// The plan carries YAML because it is heterogeneous across five kinds and
    /// because the wire wants YAML anyway. This is how a caller that knows the
    /// kind gets its struct back.
    pub fn entity<T: DeserializeOwned>(&self) -> Result<T> {
        match &self.change {
            Change::Upsert { yaml } => Ok(serde_yaml::from_str(yaml)?),
            Change::Delete => {
                anyhow::bail!("{} {:?} is a deletion, not an entity", self.kind, self.name)
            }
        }
    }

    /// Every name this change touches on disk — the new one, and the old one
    /// when it is a rename.
    fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.previous_name.as_deref())
    }
}

/// One field two machines disagreed about.
///
/// Deliberately not a bare `Result`: the sync plan pane renders these, and
/// "conflict in project api" is not something a user can act on. The entity,
/// the field and *both* values are all here so the pane can show what was kept
/// and what was discarded, in the spelling the YAML file uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub kind: String,
    pub name: String,
    pub id: Option<String>,
    /// The serialized field name — `gradientSpread`, not `gradient_spread` —
    /// so it can be found in the file. Collection elements are named
    /// `wiki[Deploy]`.
    pub field: String,
    /// This machine's value, rendered.
    pub ours: String,
    /// The peer's value, rendered.
    pub theirs: String,
    /// Which side's value survived.
    pub winner: Side,
}

/// Something the merge could not do cleanly, carried into the plan rather than
/// swallowed.
///
/// Three things produce these today: a base that would not parse (that entity
/// merged two-way), a tombstone keyed by the `{kind}--{name}` fallback (which
/// cannot match any uuid, so the deletion cannot propagate), and two different
/// entities claiming one filename (neither is written — see
/// [`settle_name_collisions`]). All three are degradations the user is entitled
/// to know about, and none of them is an error.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub kind: String,
    pub name: String,
    pub id: Option<String>,
    pub reason: String,
}

/// Everything one sync would do, before anything is written.
///
/// `incoming` and `outgoing` are the two halves the user is shown and asked to
/// confirm; `conflicts` is what collided and who won; `notes` is what degraded.
/// Deletions are `incoming`/`outgoing` entries carrying [`Change::Delete`]
/// rather than a fifth list, because a deletion and a write compete for the
/// same lock and the caller must not have to remember to union two lists to
/// find that out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergePlan {
    /// Apply on this machine.
    pub incoming: Vec<PlannedChange>,
    /// Send to the peer.
    pub outgoing: Vec<PlannedChange>,
    pub conflicts: Vec<Conflict>,
    pub notes: Vec<Note>,
}

impl MergePlan {
    pub fn is_empty(&self) -> bool {
        self.incoming.is_empty() && self.outgoing.is_empty()
    }

    /// Folds another kind's plan into this one.
    pub fn absorb(&mut self, other: MergePlan) {
        self.incoming.extend(other.incoming);
        self.outgoing.extend(other.outgoing);
        self.conflicts.extend(other.conflicts);
        self.notes.extend(other.notes);
    }

    /// Every `(kind, name)` this plan will write on **this** machine, in the
    /// order the locks must be taken.
    ///
    /// `base::accept` takes no locks and documents that the caller must already
    /// hold them for the whole batch, acquired in sorted order over
    /// `store::lock_key` and deduped on the key — `fs2` locks are not
    /// re-entrant and have no timeout, so taking one key twice in a thread
    /// hangs forever with no error and no way out but a kill.
    ///
    /// So this does the sorting and the deduping itself rather than handing
    /// back a list and a warning. Sorting by name is the mistake it exists to
    /// prevent: `lock_key` collapses everything outside `[A-Za-z0-9_-]` to `_`,
    /// and `'X' < '_'`, so the two orders genuinely disagree.
    ///
    /// `outgoing` is included. An entity that only goes *out* still gets a base
    /// recorded for it on this machine when the sync is accepted, and a base
    /// write is a write.
    ///
    /// **The dedupe is lock hygiene, not collision handling.** Collapsing two
    /// entities that claim one key into a single lock is exactly what a
    /// multi-lock batch has to do, so this cannot also be the thing that
    /// *notices* the clash — the returned pairs have no room to say it. Noticing
    /// is [`plan_kind`]'s job, which refuses to emit such a plan at all, and
    /// [`MergePlan::lock_target_collisions`] is the after-the-fact check for a
    /// plan that came from somewhere else.
    pub fn lock_targets(&self) -> Vec<(String, String)> {
        let mut targets: Vec<(String, String, String)> = Vec::new();
        for change in self.incoming.iter().chain(self.outgoing.iter()) {
            for name in change.names() {
                targets.push((
                    change.kind.clone(),
                    crate::store::lock_key(name),
                    name.to_string(),
                ));
            }
        }
        targets.sort();
        targets.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        targets.into_iter().map(|(kind, _, name)| (kind, name)).collect()
    }

    /// The lock targets that **two different entities** both claim.
    ///
    /// Always empty for a plan [`plan_kind`] produced: it turns such a clash
    /// into a [`Conflict`] and emits neither change, because applying both
    /// writes one entity over the other on one machine and the reverse on the
    /// other, losing one entity on each and leaving the two machines disagreeing
    /// about what the name means.
    ///
    /// It exists anyway so that [`MergePlan::lock_targets`]' dedupe — mandatory,
    /// and silent by necessity — is not the only thing standing between a plan
    /// assembled somewhere else and that outcome.
    ///
    /// "Different entity" is `(name, id)`, not `id` alone: two names that share
    /// a lock key are two entities however their ids read, and an unstamped
    /// entity has no id to tell apart.
    pub fn lock_target_collisions(&self) -> Vec<(String, String)> {
        let mut claims: BTreeMap<(String, String), BTreeSet<(String, Option<String>)>> =
            BTreeMap::new();
        for change in self.incoming.iter().chain(self.outgoing.iter()) {
            claims
                .entry((change.kind.clone(), crate::store::lock_key(&change.name)))
                .or_default()
                .insert((change.name.clone(), change.id.clone()));
        }
        claims
            .into_iter()
            .filter(|(_, claimants)| claimants.len() > 1)
            .map(|((kind, _), claimants)| {
                let name = claimants.iter().next().expect("a non-empty set").0.clone();
                (kind, name)
            })
            .collect()
    }
}

// ============================================================================
// Value equality and rendering
// ============================================================================

/// Equality by value.
///
/// **Never use `==` on an entity in this module.** `Trail` and `TrailStep` both
/// carry hand-written `PartialEq` impls that compare only `name`, so `==` calls
/// two completely different trails equal — a merge built on it would decide
/// nothing had changed and drop every edit. Comparing serialized values sides
/// steps that, and works uniformly for the types that derive no `PartialEq` at
/// all (`IssueProviderConfig`, `WikiProviderConfig`, `Livestock`, `Critter`,
/// `Herd`).
///
/// A value that will not serialize compares unequal, which sends the field down
/// the conflict path rather than silently declaring a match.
fn same<T: Serialize>(a: &T, b: &T) -> bool {
    match (serde_yaml::to_value(a), serde_yaml::to_value(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// A field value as the plan pane shows it.
fn render<T: Serialize>(value: &T) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|e| format!("<unrenderable: {}>", e))
        .trim_end()
        .to_string()
}

// ============================================================================
// C2 — per-field three-way merge
// ============================================================================

/// Accumulates one entity's field decisions and the conflicts they produced.
pub struct Fields {
    kind: &'static str,
    name: String,
    id: Option<String>,
    house: Side,
    conflicts: Vec<Conflict>,
}

impl Fields {
    fn new(kind: &'static str, name: &str, id: Option<String>, house: Side) -> Self {
        Self { kind, name: name.to_string(), id, house, conflicts: Vec::new() }
    }

    /// Three-way merge of one field.
    ///
    /// - Equal on both sides: nothing to decide.
    /// - One side matches the base: the *other* side changed it, so take the
    ///   other side. This is the whole reason a base exists — without it there
    ///   is no way to tell "they changed it" from "we changed it", and the
    ///   merge can only pick a side and discard the other.
    /// - Both sides changed it, or there is no base to compare against: the
    ///   Ranch House wins, and the disagreement is recorded with both values.
    pub fn pick<T: Serialize + Clone>(
        &mut self,
        field: &str,
        base: Option<&T>,
        ours: &T,
        theirs: &T,
    ) -> T {
        if same(ours, theirs) {
            return ours.clone();
        }
        if let Some(base) = base {
            if same(ours, base) {
                return theirs.clone();
            }
            if same(theirs, base) {
                return ours.clone();
            }
        }
        self.conflict(field, render(ours), render(theirs), self.house);
        match self.house {
            Side::Local => ours.clone(),
            Side::Remote => theirs.clone(),
        }
    }

    fn conflict(&mut self, field: &str, ours: String, theirs: String, winner: Side) {
        self.conflicts.push(Conflict {
            kind: self.kind.to_string(),
            name: self.name.clone(),
            id: self.id.clone(),
            field: field.to_string(),
            ours,
            theirs,
            winner,
        });
    }

    pub fn house(&self) -> Side {
        self.house
    }
}

/// `(the Ranch House's, the peer's)`.
///
/// Everything that has to break a tie or inherit an order goes through here, so
/// there is one place where "which side is the house" is decided and no call
/// site can quietly answer it by argument position instead.
fn by_house<'a, T>(ours: &'a T, theirs: &'a T, house: Side) -> (&'a T, &'a T) {
    match house {
        Side::Local => (ours, theirs),
        Side::Remote => (theirs, ours),
    }
}

/// Whichever of the two the Ranch House holds.
fn house_pick<'a, T>(ours: &'a T, theirs: &'a T, house: Side) -> &'a T {
    by_house(ours, theirs, house).0
}

// ============================================================================
// C3 — unioned collections
// ============================================================================

/// Unions two collections keyed by `key`, three-way against `base`, in an
/// order the merge does not invent.
///
/// # Why the order has to be pinned at all
///
/// `canonical.rs` treats `Vec` order as content — correctly, because
/// `TrailStep`s and `WikiSection`s are order-significant. So if this machine's
/// union yields `[x, y]` and the peer's yields `[y, x]`, the two sides agree
/// under the merge's key model and disagree under the hash: each sees the other
/// as changed, ships, re-merges, and the sync never converges.
///
/// # Why it is inherited rather than sorted
///
/// Sorting by the union key is the obvious fix and it converges. It was
/// implemented first, and then run against a copy of a real ranch, where it
/// alphabetized a hand-authored wiki — `Architecture, Conventions, Commands,
/// Domain Context, Common Tasks, Gotchas` came back as `Architecture, Commands,
/// Common Tasks, Conventions, Domain Context, Gotchas` — and rewrote 12 of 17
/// projects that nobody had edited. A document whose order the design calls
/// content is not something a merge may re-sort; that is the same mistake as
/// blanket-sorting trail steps, one layer up.
///
/// So the order is **inherited, in three tiers**:
///
/// 1. keys the **base** had, in the base's order — the common ancestor is the
///    two machines' shared notion of what order this collection was in;
/// 2. then keys the **Ranch House** has, in the order it wrote them;
/// 3. then the peer's remaining additions, in the order *it* wrote them.
///
/// Every tier is a function of the three inputs plus which side is the house,
/// and both machines hold all four identically — so both compute the same order
/// and the result converges. What it is *not* is a function of the argument
/// slots: swap `ours` and `theirs` and swap `house`, and the answer is
/// unchanged. That is what `merge_is_commutative_given_the_same_base` pins, and
/// it is the convergence guarantee.
///
/// A sort by key closes the list as an unreachable backstop, so that a key
/// somehow reachable from none of the three inputs still cannot make the output
/// depend on hash iteration order.
///
/// # Union means union
///
/// An element present on one side and absent on the other is **kept**, whether
/// or not the base had it. A removal is expressed by a tombstone, never by an
/// absence — a peer that has simply never heard of a livestock is
/// indistinguishable from one that deleted it.
///
/// A key repeated within one side keeps its first occurrence, the same rule
/// every loader in this codebase already applies to duplicate names.
fn union_with<T, K, F, C>(
    base: Option<&[T]>,
    ours: &[T],
    theirs: &[T],
    key: F,
    house: Side,
    mut combine: C,
) -> Vec<T>
where
    T: Clone,
    K: Ord + Clone,
    F: Fn(&T) -> K,
    C: FnMut(&K, Option<&T>, &T, &T) -> T,
{
    let index = |items: &[T]| -> BTreeMap<K, T> {
        let mut map = BTreeMap::new();
        for item in items {
            map.entry(key(item)).or_insert_with(|| item.clone());
        }
        map
    };

    let ours_by_key = index(ours);
    let theirs_by_key = index(theirs);
    let base_by_key = base.map(index).unwrap_or_default();

    let (house_side, peer_side) = by_house(&ours, &theirs, house);
    let tiers: [&[T]; 3] = [base.unwrap_or(&[]), house_side, peer_side];

    let mut keys: Vec<K> = Vec::new();
    let mut seen: BTreeSet<K> = BTreeSet::new();
    for tier in tiers {
        for item in tier {
            let k = key(item);
            // A base-only key is not in the output: the element is gone from
            // both sides, which only the base remembers.
            if (ours_by_key.contains_key(&k) || theirs_by_key.contains_key(&k))
                && seen.insert(k.clone())
            {
                keys.push(k);
            }
        }
    }
    // Unreachable backstop: every surviving key came from `ours` or `theirs`,
    // both of which are tiers above. Sorted rather than arbitrary so that if it
    // ever is reached, it is still deterministic.
    let mut leftover: Vec<K> = ours_by_key
        .keys()
        .chain(theirs_by_key.keys())
        .filter(|k| !seen.contains(k))
        .cloned()
        .collect();
    leftover.sort();
    leftover.dedup();
    keys.extend(leftover);

    keys.into_iter()
        .map(|k| match (ours_by_key.get(&k), theirs_by_key.get(&k)) {
            (Some(ours), Some(theirs)) => combine(&k, base_by_key.get(&k), ours, theirs),
            (Some(only), None) | (None, Some(only)) => only.clone(),
            (None, None) => unreachable!("keys came from one of the two maps"),
        })
        .collect()
}

/// The default way to settle one collection element that exists on both sides:
/// three-way on the element as a whole.
///
/// Whole-element rather than field-wise on purpose. Three-way means an edit made
/// on the side that is *not* the Ranch House survives whenever the house did not
/// touch that element, which is strictly more preserving than "house wins"; what
/// it cannot do is merge two edits to *different* fields of one element, and
/// that is the trade accepted here rather than writing a field-wise merge for
/// every nested type.
///
/// The returned `bool` is "the house's copy replaced the peer's", and **every
/// caller owes the user a conflict when it is true**. The element the house
/// discarded was somebody's edit; nothing downstream of here can tell it
/// happened, and a plan pane that shows nothing is how the edit disappears.
/// [`element_disagreement`] turns it into a field name and two values.
fn three_way_element<T: Serialize + Clone>(
    base: Option<&T>,
    ours: &T,
    theirs: &T,
    house: Side,
) -> (T, bool) {
    if same(ours, theirs) {
        return (ours.clone(), false);
    }
    if let Some(base) = base {
        if same(ours, base) {
            return (theirs.clone(), false);
        }
        if same(theirs, base) {
            return (ours.clone(), false);
        }
    }
    (house_pick(ours, theirs, house).clone(), true)
}

/// How to describe an element the house overwrote: which key the two machines
/// disagree about, and the two values to show.
///
/// Returns `(field suffix, ours, theirs)`. The suffix is `".branch"` when exactly
/// one top-level key differs — the ordinary case, one machine moving a branch or
/// a service name — and empty when several do, because there is then no single
/// key to name and the whole record is the honest thing to show.
///
/// Worth the comparison because [`three_way_element`] settles an element whole
/// and so produces no field name of its own: `livestock[pi/worker]` with two
/// twelve-key records beside it sends the user hunting for the difference the
/// merge already knew.
///
/// A value that will not serialize falls back to the whole record, the same
/// direction [`same`] takes: no claim is better than a wrong one.
fn element_disagreement<T: Serialize>(ours: &T, theirs: &T) -> (String, String, String) {
    let whole = || (String::new(), render(ours), render(theirs));

    let (Ok(serde_yaml::Value::Mapping(a)), Ok(serde_yaml::Value::Mapping(b))) =
        (serde_yaml::to_value(ours), serde_yaml::to_value(theirs))
    else {
        return whole();
    };

    // Absent and explicitly null mean the same thing to every loader here, so a
    // key present on one side only still counts as that key differing.
    let null = serde_yaml::Value::Null;
    let mut keys: Vec<&serde_yaml::Value> = a.keys().chain(b.keys()).collect();
    keys.sort_by(|x, y| x.as_str().unwrap_or_default().cmp(y.as_str().unwrap_or_default()));
    keys.dedup();

    let differing: Vec<&serde_yaml::Value> = keys
        .into_iter()
        .filter(|k| a.get(k).unwrap_or(&null) != b.get(k).unwrap_or(&null))
        .collect();

    match differing.as_slice() {
        [key] => match key.as_str() {
            Some(name) => (
                format!(".{}", name),
                render(a.get(key).unwrap_or(&null)),
                render(b.get(key).unwrap_or(&null)),
            ),
            None => whole(),
        },
        _ => whole(),
    }
}

/// A pure union of a member list — deduped, no removals honoured, in inherited
/// order.
///
/// `house_side` first, in the order it wrote them, then `peer_side`'s additions
/// in the order *it* wrote them. Same reasoning as [`union_with`]: a member list
/// the user arranged is not the merge's to re-sort, and inheriting an order both
/// machines can compute is enough for convergence.
///
/// The parameters are named by role rather than by side so a caller cannot pass
/// `(ours, theirs)` and silently make the outcome depend on which machine ran
/// the sync.
fn union_members<T, K, F>(house_side: &[T], peer_side: &[T], key: F) -> Vec<T>
where
    T: Clone,
    K: Ord + Clone,
    F: Fn(&T) -> K,
{
    let mut seen: BTreeSet<K> = BTreeSet::new();
    let mut out: Vec<T> = Vec::new();
    for item in house_side.iter().chain(peer_side.iter()) {
        if seen.insert(key(item)) {
            out.push(item.clone());
        }
    }
    out
}

// ============================================================================
// The kinds
// ============================================================================

/// An entity kind the sync moves.
///
/// `KIND` is what `canonical::hash_entity` and `base::accept` are given, so it
/// is a constant on the type rather than a string threaded through every call —
/// the two cannot drift.
pub trait Mergeable: Serialize + DeserializeOwned + Clone + Identified {
    const KIND: &'static str;

    fn entity_name(&self) -> &str;

    /// Merges everything that is content. Identity is settled separately by
    /// [`settle_identity`], and machine-local fields are taken from `ours` here
    /// and then re-pointed per destination by [`Mergeable::take_machine_local_from`].
    fn merge_content(base: Option<&Self>, ours: &Self, theirs: &Self, fields: &mut Fields)
        -> Self;

    /// Copies this kind's machine-local fields out of `source`.
    ///
    /// A machine-local field is a verdict about the machine holding the file,
    /// not a property of the entity: `Barn.connectable` is a reachability
    /// result, `Project.path` is where the checkout lives here, `RanchHand.config`
    /// is a local kubeconfig path, `RanchHand.last_sync` is `Utc::now()` from the
    /// last discovery run. None of them is a change either side *made*, so
    /// neither side may win one, and shipping one over the wire actively breaks
    /// the receiver — a Ranch House that cannot reach a barn would push
    /// `connectable: Some(false)` onto a laptop that can, and `connect.rs` then
    /// hard-refuses a host the laptop can demonstrably reach; a Ranch House's
    /// `/Users/cam/Sites/api` landing on a Pi leaves `expand_path` there opening
    /// nothing.
    ///
    /// So the merged entity carries the machine-local fields *of the machine it
    /// is destined for*: the copy applied here keeps ours, the copy sent to the
    /// peer keeps theirs.
    ///
    /// Default is a no-op — `Worm` and `Trail` have none.
    fn take_machine_local_from(&mut self, _source: &Self) {}

    /// Blanks this kind's machine-local fields, for an entity the destination
    /// has never seen and therefore has no verdict about.
    ///
    /// Blank is not the same as "the sender's value". An unknown barn arriving
    /// with the sender's `connectable: Some(false)` would be unreachable here
    /// before anything on this machine had ever tried it; `None` means "not yet
    /// known", which is what is true.
    fn clear_machine_local(&mut self) {}
}

/// Merges two copies of one entity against their common ancestor.
///
/// The whole of the per-entity merge, and the only path [`plan_kind`] takes —
/// so a test that drives this is testing what a real sync runs, not a
/// reimplementation of it.
///
/// `base` is `None` for an entity that has never been synced *and* for one
/// whose base would not parse; both are two-way merges. The difference is
/// visible to the caller, not to the merge — see [`Ancestor`].
///
/// The returned entity carries **this machine's** machine-local fields. Use
/// [`Mergeable::take_machine_local_from`] to point them at the other machine
/// before shipping it there.
pub fn merge_entity<T: Mergeable>(
    base: Option<&T>,
    ours: &T,
    theirs: &T,
    house: Side,
) -> (T, Vec<Conflict>) {
    let mut fields = Fields::new(
        T::KIND,
        ours.entity_name(),
        ours.id().or_else(|| theirs.id()).map(|s| s.to_string()),
        house,
    );
    let mut merged = T::merge_content(base, ours, theirs, &mut fields);
    settle_identity(&mut merged, ours, theirs, house);
    (merged, fields.conflicts)
}

/// Identity is not content, so it is not merged field-wise.
///
/// - **`id`** comes from the Ranch House. On a first join the two machines
///   minted their uuids independently, so one of them has to be adopted, and
///   the design says the Ranch House wins collisions. Adopting it is what makes
///   every *subsequent* sync able to match by uuid.
/// - **`created_at`** is the earlier of the two. It is a fact about when the
///   entity came into being, and the earlier record is the truer one.
/// - **`updated_at`** is the later of the two. It feeds tombstone comparison,
///   where taking anything but the latest would let a stale timestamp authorize
///   a deletion of freshly edited content.
///
/// Timestamps are **parsed**, never string-compared: chrono's serde emits `Z`,
/// `'Z'` sorts above every digit, and a western offset puts a later instant at
/// a lower wall-clock hour.
fn settle_identity<T: Identified>(merged: &mut T, ours: &T, theirs: &T, house: Side) {
    // The uuid follows the Ranch House, never the argument slot — `by_house` is
    // the one place that answers which side that is. Taking `ours` here instead
    // would make the adoption depend on which machine ran the merge: the
    // joining machine would keep its own uuid and push it at the house, and the
    // two sides would each see the other as the one that has to change.
    //
    // The peer's uuid is used only when the house has none to give. Then it is
    // the only uuid in existence, and adopting it still leaves both sides
    // holding the same one.
    let (house_side, peer) = by_house(ours, theirs, house);
    if let Some(id) = house_side.id().or_else(|| peer.id()) {
        merged.set_id(id.to_string());
    }

    if let Some(created) = pick_time(ours.created_at(), theirs.created_at(), Ordering::Earliest) {
        merged.set_created_at(created);
    }
    if let Some(updated) = pick_time(ours.updated_at(), theirs.updated_at(), Ordering::Latest) {
        merged.set_updated_at(updated);
    }
}

enum Ordering {
    Earliest,
    Latest,
}

/// The earlier or later of two RFC 3339 timestamps, either of which may be
/// absent or unparseable.
///
/// An unparseable timestamp is not evidence of anything, so it loses to a
/// parseable one and is only returned when it is all there is.
fn pick_time(a: Option<&str>, b: Option<&str>, which: Ordering) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => match (parse_time(a), parse_time(b)) {
            (Some(ta), Some(tb)) => {
                let take_a = match which {
                    Ordering::Earliest => ta <= tb,
                    Ordering::Latest => ta >= tb,
                };
                Some(if take_a { a.to_string() } else { b.to_string() })
            }
            (Some(_), None) => Some(a.to_string()),
            (None, Some(_)) => Some(b.to_string()),
            (None, None) => Some(a.to_string()),
        },
        (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
        (None, None) => None,
    }
}

fn parse_time(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|dt| dt.with_timezone(&chrono::Utc))
}

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

/// `(barn, name)`, the design's livestock key.
///
/// `barn: None` means "this machine" on an unmigrated ranch and collapses onto
/// the empty string. Two livestock that differ only in `None` versus `Some("")`
/// are the same livestock in every part of this codebase, so collapsing them is
/// the behaviour that matches, and a `barn` literally named "" does not exist.
fn livestock_key(l: &crate::types::Livestock) -> (String, String) {
    (l.barn.clone().unwrap_or_default(), l.name.clone())
}

impl Mergeable for crate::types::Project {
    const KIND: &'static str = "project";

    fn entity_name(&self) -> &str {
        &self.name
    }

    fn merge_content(
        base: Option<&Self>,
        ours: &Self,
        theirs: &Self,
        f: &mut Fields,
    ) -> Self {
        use crate::types::Project;

        let house = f.house();

        // Field names are the *serialized* spellings, so a conflict the user
        // reads names something they can find in the file.
        Project {
            name: f.pick("name", base.map(|b| &b.name), &ours.name, &theirs.name),
            // Machine-local. Where the checkout lives here is not a property of
            // the project: the design rejected "syncing filesystem paths
            // verbatim, which breaks the moment a Linux Pi joins", and the Layer
            // 3 table lists no `path` among the project scalars. Merged, the
            // Ranch House's `/Users/cam/Sites/api` lands on a Pi and
            // `expand_path(&project.path)` there opens nothing — no session and
            // no trail. See `take_machine_local_from`.
            path: ours.path.clone(),
            summary: f.pick("summary", base.map(|b| &b.summary), &ours.summary, &theirs.summary),
            color: f.pick("color", base.map(|b| &b.color), &ours.color, &theirs.color),
            gradient_spread: f.pick(
                "gradientSpread",
                base.map(|b| &b.gradient_spread),
                &ours.gradient_spread,
                &theirs.gradient_spread,
            ),
            gradient_inverted: f.pick(
                "gradientInverted",
                base.map(|b| &b.gradient_inverted),
                &ours.gradient_inverted,
                &theirs.gradient_inverted,
            ),
            livestock: merge_livestock(base, ours, theirs, f),
            herds: merge_herds(base, ours, theirs, house),
            wiki: merge_wiki(base, ours, theirs, f),
            issue_provider: f.pick(
                "issueProvider",
                base.map(|b| &b.issue_provider),
                &ours.issue_provider,
                &theirs.issue_provider,
            ),
            wiki_provider: f.pick(
                "wikiProvider",
                base.map(|b| &b.wiki_provider),
                &ours.wiki_provider,
                &theirs.wiki_provider,
            ),
            id: ours.id.clone(),
            created_at: ours.created_at.clone(),
            updated_at: ours.updated_at.clone(),
        }
    }

    fn take_machine_local_from(&mut self, source: &Self) {
        self.path = source.path.clone();
    }

    fn clear_machine_local(&mut self) {
        // Blank, not the sender's path. A project this machine has never seen has
        // no checkout here, and `""` says exactly that — whereas the sender's
        // `/home/cam/api` would have the TUI offering to open a directory that
        // does not exist, which reads as a broken project rather than an
        // unconfigured one. Same reasoning as `Barn::clear_machine_local`.
        self.path = String::new();
    }
}

/// Livestock, keyed by `(barn, name)`, unioned, Ranch House wins an element both
/// machines edited — and says so.
///
/// The union half is the design's: a livestock is a codebase on a specific
/// machine, so one the peer does not list is **kept**, and removal takes an
/// explicit tombstone rather than an absence. That is what "never overwritten,
/// never conflicting" is about — the key *set*.
///
/// It is not a licence to discard a field edit invisibly. With both machines
/// having moved `branch` on the same livestock, the house's value replaces the
/// peer's; nothing downstream can tell, and the plan pane is the only place the
/// user could ever find out. So the same shape as [`merge_wiki`]: report it,
/// named `livestock[pi/worker].branch`.
fn merge_livestock(
    base: Option<&crate::types::Project>,
    ours: &crate::types::Project,
    theirs: &crate::types::Project,
    f: &mut Fields,
) -> Vec<crate::types::Livestock> {
    let house = f.house();
    let mut overwritten: Vec<(String, String, String)> = Vec::new();

    let merged = union_with(
        base.map(|b| b.livestock.as_slice()),
        &ours.livestock,
        &theirs.livestock,
        livestock_key,
        house,
        |(barn, name), base, ours, theirs| {
            let (winner, collided) = three_way_element(base, ours, theirs, house);
            if collided {
                let (which, ours, theirs) = element_disagreement(ours, theirs);
                overwritten.push((format!("livestock[{}/{}]{}", barn, name, which), ours, theirs));
            }
            winner
        },
    );

    for (field, ours, theirs) in overwritten {
        f.conflict(&field, ours, theirs, house);
    }
    merged
}

/// Wiki sections, keyed by title, unioned, Ranch House wins a same-title
/// collision.
///
/// Unlike livestock this *does* record a conflict. Two people editing the same
/// section of the same document is exactly the case where discarding one side
/// silently is unacceptable, and it is what the plan pane exists to show.
fn merge_wiki(
    base: Option<&crate::types::Project>,
    ours: &crate::types::Project,
    theirs: &crate::types::Project,
    f: &mut Fields,
) -> Vec<crate::types::WikiSection> {
    let house = f.house();
    let mut collisions: Vec<(String, String, String)> = Vec::new();

    let merged = union_with(
        base.map(|b| b.wiki.as_slice()),
        &ours.wiki,
        &theirs.wiki,
        |s| s.title.clone(),
        house,
        |title, base, ours, theirs| {
            let (winner, collided) = three_way_element(base, ours, theirs, house);
            if collided {
                collisions.push((
                    title.clone(),
                    ours.content.clone(),
                    theirs.content.clone(),
                ));
            }
            winner
        },
    );

    for (title, ours, theirs) in collisions {
        f.conflict(&format!("wiki[{}]", title), ours, theirs, house);
    }
    merged
}

/// Herds union by name, and a herd present on both sides unions its member
/// lists rather than picking a side — the design's rule, and the one that
/// cannot lose a membership.
///
/// Every herd that comes out is normalized, **including one only a single side
/// has**. That is not tidiness. A herd carried across verbatim keeps whatever
/// order and whatever duplicates its own machine happened to write, so the very
/// next sync — where both sides now hold it and the member lists actually get
/// unioned — produces a different list and reports the entity as changed again.
/// Measured: without this, `merge_is_idempotent_on_re_sync` fails at seed 147
/// and `a_union_s_order_depends_on_the_contents_and_nothing_else` fails outright.
fn merge_herds(
    base: Option<&crate::types::Project>,
    ours: &crate::types::Project,
    theirs: &crate::types::Project,
    house: Side,
) -> Vec<crate::types::Herd> {
    union_with(
        base.map(|b| b.herds.as_slice()),
        &ours.herds,
        &theirs.herds,
        |h| h.name.clone(),
        house,
        |_, _, ours, theirs| {
            let (h, p) = by_house(ours, theirs, house);
            crate::types::Herd {
                name: h.name.clone(),
                livestock: union_members(&h.livestock, &p.livestock, |l| l.clone()),
                critters: union_members(&h.critters, &p.critters, |c| {
                    (c.barn.clone(), c.critter.clone())
                }),
                connections: union_members(&h.connections, &p.connections, |c| {
                    (c.livestock.clone(), c.critter.clone(), c.barn.clone())
                }),
            }
        },
    )
    .iter()
    .map(normalized_herd)
    .collect()
}

/// One herd's member lists, deduped in place.
///
/// Order is left alone — that is [`union_members`]'s job and it inherits rather
/// than imposes. What this removes is duplicates, which a herd only one side has
/// can still carry: without it, the next sync (where both sides hold the herd
/// and the lists actually get unioned) dedupes them and reports the entity as
/// changed all over again. Measured: `merge_is_idempotent_on_re_sync` fails at
/// seed 147 without this.
///
/// Idempotent, which is what lets it be applied to both halves of the union
/// without the two-sided case getting a different answer from the one-sided one.
fn normalized_herd(herd: &crate::types::Herd) -> crate::types::Herd {
    crate::types::Herd {
        name: herd.name.clone(),
        livestock: union_members(&herd.livestock, &[], |l| l.clone()),
        critters: union_members(&herd.critters, &[], |c| (c.barn.clone(), c.critter.clone())),
        connections: union_members(&herd.connections, &[], |c| {
            (c.livestock.clone(), c.critter.clone(), c.barn.clone())
        }),
    }
}

// ---------------------------------------------------------------------------
// Barn
// ---------------------------------------------------------------------------

impl Mergeable for crate::types::Barn {
    const KIND: &'static str = "barn";

    fn entity_name(&self) -> &str {
        &self.name
    }

    fn merge_content(base: Option<&Self>, ours: &Self, theirs: &Self, f: &mut Fields) -> Self {
        use crate::types::Barn;

        Barn {
            name: f.pick("name", base.map(|b| &b.name), &ours.name, &theirs.name),
            // The design's one genuinely ambiguous case: same barn name, two
            // hosts. It resolves like any other field — house wins — and the
            // conflict is what the TUI prompts on.
            host: f.pick("host", base.map(|b| &b.host), &ours.host, &theirs.host),
            user: f.pick("user", base.map(|b| &b.user), &ours.user, &theirs.user),
            port: f.pick("port", base.map(|b| &b.port), &ours.port, &theirs.port),
            identity_file: f.pick(
                "identity_file",
                base.map(|b| &b.identity_file),
                &ours.identity_file,
                &theirs.identity_file,
            ),
            critters: merge_critters(base, ours, theirs, f),
            // Content: the barn's own ed25519 public half. It exists to be
            // distributed — the Ranch House writes it into every other barn's
            // `authorized_keys` — so a merge that kept `ours` would mean a
            // joining machine's brand never reaches the house and D3 never
            // works. The private half is not on this struct at all.
            brand: f.pick("brand", base.map(|b| &b.brand), &ours.brand, &theirs.brand),
            // Content: there is exactly one Ranch House per ranch and every
            // machine has to agree which barn it is, because the merge's own
            // tie-break is "the house wins". A per-machine answer here would be
            // two machines disagreeing about who arbitrates.
            is_ranch_house: f.pick(
                "is_ranch_house",
                base.map(|b| &b.is_ranch_house),
                &ours.is_ranch_house,
                &theirs.is_ranch_house,
            ),
            addresses: merge_addresses(ours, theirs, f),
            source: f.pick("source", base.map(|b| &b.source), &ours.source, &theirs.source),
            connection_type: f.pick(
                "connection_type",
                base.map(|b| &b.connection_type),
                &ours.connection_type,
                &theirs.connection_type,
            ),
            connection_config: f.pick(
                "connection_config",
                base.map(|b| &b.connection_config),
                &ours.connection_config,
                &theirs.connection_config,
            ),
            // Machine-local. Never merged, never a conflict — see
            // `take_machine_local_from`.
            connectable: ours.connectable,
            synced: ours.synced,
            tunnel_port: ours.tunnel_port,
            last_seen: ours.last_seen.clone(),
            id: ours.id.clone(),
            created_at: ours.created_at.clone(),
            updated_at: ours.updated_at.clone(),
        }
    }

    fn take_machine_local_from(&mut self, source: &Self) {
        self.connectable = source.connectable;
        self.synced = source.synced;
        self.tunnel_port = source.tunnel_port;
        self.last_seen = source.last_seen.clone();
    }

    fn clear_machine_local(&mut self) {
        self.connectable = None;
        self.synced = None;
        self.tunnel_port = None;
        self.last_seen = None;
    }
}

/// A barn's addresses, unioned — house's first, in its order, then the peer's
/// additions in theirs.
///
/// A union rather than a pick because the list is additive: two machines that
/// each learned a different address for the same barn are both right, and an
/// address that does not work costs a connect timeout, not a lost host. That is
/// what separates this from `connectable`, where a wrong value is a hard refusal.
///
/// No removals honoured, same as every other unioned collection here. And the
/// order is not incidental: `canonical::render` hashes sequence order, so a union
/// that came out differently on each side would make every barn read as changed
/// on both, forever — the obligation `canonical.rs`'s last section spells out for
/// exactly this case.
fn merge_addresses(
    ours: &crate::types::Barn,
    theirs: &crate::types::Barn,
    f: &mut Fields,
) -> Vec<String> {
    let (house, peer) = by_house(&ours.addresses, &theirs.addresses, f.house());
    union_members(house, peer, |a| a.clone())
}

/// Critters, keyed by name, unioned under their barn, Ranch House wins an
/// element both machines edited — and says so.
///
/// Same reasoning as [`merge_livestock`], and the same omission it closes: a
/// critter the house overwrote is an edit somebody made, and discarding it
/// without a word is the part the design's "union under their barn" never
/// licensed.
fn merge_critters(
    base: Option<&crate::types::Barn>,
    ours: &crate::types::Barn,
    theirs: &crate::types::Barn,
    f: &mut Fields,
) -> Vec<crate::types::Critter> {
    let house = f.house();
    // The barn's own name, so a conflict reads `critters[pi/nginx]` — the critter
    // name alone does not say which machine it runs on.
    let barn = house_pick(&ours.name, &theirs.name, house).clone();
    let mut overwritten: Vec<(String, String, String)> = Vec::new();

    let merged = union_with(
        base.map(|b| b.critters.as_slice()),
        &ours.critters,
        &theirs.critters,
        |c| c.name.clone(),
        house,
        |name, base, ours, theirs| {
            let (winner, collided) = three_way_element(base, ours, theirs, house);
            if collided {
                let (which, ours, theirs) = element_disagreement(ours, theirs);
                overwritten.push((format!("critters[{}/{}]{}", barn, name, which), ours, theirs));
            }
            winner
        },
    );

    for (field, ours, theirs) in overwritten {
        f.conflict(&field, ours, theirs, house);
    }
    merged
}

// ---------------------------------------------------------------------------
// Worm
// ---------------------------------------------------------------------------

impl Mergeable for crate::types::Worm {
    const KIND: &'static str = "worm";

    fn entity_name(&self) -> &str {
        &self.name
    }

    fn merge_content(base: Option<&Self>, ours: &Self, theirs: &Self, f: &mut Fields) -> Self {
        use crate::types::Worm;

        Worm {
            name: f.pick("name", base.map(|b| &b.name), &ours.name, &theirs.name),
            command: f.pick("command", base.map(|b| &b.command), &ours.command, &theirs.command),
            schedule: f.pick(
                "schedule",
                base.map(|b| &b.schedule),
                &ours.schedule,
                &theirs.schedule,
            ),
            // Serialized as `type`; the Rust name is `worm_type`. Naming the
            // field by its Rust spelling in a conflict would send the user
            // looking for a key their file does not contain.
            worm_type: f.pick("type", base.map(|b| &b.worm_type), &ours.worm_type, &theirs.worm_type),
            enabled: f.pick("enabled", base.map(|b| &b.enabled), &ours.enabled, &theirs.enabled),
            project: f.pick("project", base.map(|b| &b.project), &ours.project, &theirs.project),
            working_dir: f.pick(
                "working_dir",
                base.map(|b| &b.working_dir),
                &ours.working_dir,
                &theirs.working_dir,
            ),
            id: ours.id.clone(),
            created_at: ours.created_at.clone(),
            updated_at: ours.updated_at.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Trail
// ---------------------------------------------------------------------------

impl Mergeable for crate::trails::Trail {
    const KIND: &'static str = "trail";

    fn entity_name(&self) -> &str {
        &self.name
    }

    fn merge_content(base: Option<&Self>, ours: &Self, theirs: &Self, f: &mut Fields) -> Self {
        use crate::trails::Trail;

        Trail {
            name: f.pick("name", base.map(|b| &b.name), &ours.name, &theirs.name),
            on: f.pick("on", base.map(|b| &b.on), &ours.on, &theirs.on),
            env: f.pick("env", base.map(|b| &b.env), &ours.env, &theirs.env),
            // `jobs` is settled whole rather than unioned per job name. A trail
            // is a program: steps within a job are ordered and a job's steps
            // reference each other's side effects, so half of one side's job
            // list beside half of the other's is not a trail anybody wrote.
            // The design says the Ranch House wins a trail collision; this is
            // that, with the base still allowed to resolve it when only one
            // side moved.
            jobs: f.pick("jobs", base.map(|b| &b.jobs), &ours.jobs, &theirs.jobs),
            id: ours.id.clone(),
            created_at: ours.created_at.clone(),
            updated_at: ours.updated_at.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// RanchHand
// ---------------------------------------------------------------------------

impl Mergeable for crate::types::RanchHand {
    const KIND: &'static str = "ranchhand";

    fn entity_name(&self) -> &str {
        &self.name
    }

    fn merge_content(base: Option<&Self>, ours: &Self, theirs: &Self, f: &mut Fields) -> Self {
        use crate::types::RanchHand;

        RanchHand {
            name: f.pick("name", base.map(|b| &b.name), &ours.name, &theirs.name),
            project: f.pick("project", base.map(|b| &b.project), &ours.project, &theirs.project),
            rh_type: f.pick("type", base.map(|b| &b.rh_type), &ours.rh_type, &theirs.rh_type),
            // Machine-local, both of them. `config` holds a kubeconfig path or
            // an S3 reference that resolves only on the machine that wrote it;
            // `last_sync` is `Utc::now()` from the last discovery run and so
            // differs on both sides essentially always. Merging either yields a
            // permanent conflict on every ranch hand, forever.
            config: ours.config.clone(),
            last_sync: ours.last_sync.clone(),
            sync_settings: f.pick(
                "sync_settings",
                base.map(|b| &b.sync_settings),
                &ours.sync_settings,
                &theirs.sync_settings,
            ),
            herd: f.pick("herd", base.map(|b| &b.herd), &ours.herd, &theirs.herd),
            resource_mappings: f.pick(
                "resource_mappings",
                base.map(|b| &b.resource_mappings),
                &ours.resource_mappings,
                &theirs.resource_mappings,
            ),
            id: ours.id.clone(),
            created_at: ours.created_at.clone(),
            updated_at: ours.updated_at.clone(),
        }
    }

    fn take_machine_local_from(&mut self, source: &Self) {
        self.config = source.config.clone();
        self.last_sync = source.last_sync.clone();
    }

    fn clear_machine_local(&mut self) {
        self.config = serde_yaml::Value::Null;
        self.last_sync = None;
    }
}

// ============================================================================
// C5 — tombstones
// ============================================================================

/// The tombstones for one kind, newest per id.
///
/// Keyed by uuid only. A tombstone recorded for an entity that was deleted
/// before it was ever stamped carries the `{kind}--{name}` fallback id from
/// `tombstones::record`, which no peer's uuid can equal — so such a deletion
/// **cannot propagate**, by construction. That is a real limitation of Phase 1's
/// fallback, not something this module can paper over by also matching on name:
/// name-matching a tombstone would let a deletion on one machine kill an
/// unrelated entity that merely shares a name on the other. It is surfaced as a
/// [`Note`] instead.
struct Tombstones<'a> {
    by_id: HashMap<&'a str, &'a WireTombstone>,
    /// Fallback-keyed stones, indexed by the name they name, so the plan can
    /// say why a deletion did not travel.
    fallback_by_name: HashMap<&'a str, &'a WireTombstone>,
}

impl<'a> Tombstones<'a> {
    fn for_kind(kind: &str, stones: &'a [WireTombstone]) -> Self {
        let mut by_id: HashMap<&str, &WireTombstone> = HashMap::new();
        let mut fallback_by_name: HashMap<&str, &WireTombstone> = HashMap::new();

        for stone in stones.iter().filter(|s| s.kind == kind) {
            let target = if is_fallback_key(stone) {
                &mut fallback_by_name
            } else {
                &mut by_id
            };
            let key: &str = if is_fallback_key(stone) { &stone.name } else { &stone.id };

            match target.get(key) {
                // Newest wins: an entity deleted, recreated and deleted again
                // is governed by the last deletion.
                Some(existing) if !newer(stone, existing) => {}
                _ => {
                    target.insert(key, stone);
                }
            }
        }

        Self { by_id, fallback_by_name }
    }

    fn covering(&self, id: Option<&str>) -> Option<&'a WireTombstone> {
        id.and_then(|id| self.by_id.get(id).copied())
    }

    fn fallback_for(&self, name: &str) -> Option<&'a WireTombstone> {
        self.fallback_by_name.get(name).copied()
    }
}

/// `tombstones::record` falls back to `{kind}--{name}` when the entity had no
/// uuid to record.
fn is_fallback_key(stone: &WireTombstone) -> bool {
    stone.id == format!("{}--{}", stone.kind, stone.name)
}

fn newer(a: &WireTombstone, b: &WireTombstone) -> bool {
    match (parse_time(&a.deleted_at), parse_time(&b.deleted_at)) {
        (Some(ta), Some(tb)) => ta > tb,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Whether `entity` has been touched since `stone` was recorded.
///
/// A tombstone deletes an entity only when the answer is no. Otherwise the
/// deletion is stale: somebody edited the entity after the other machine threw
/// it away, and applying the stone would discard that edit with no trace.
///
/// Timestamps are **parsed**. String comparison is wrong in both directions
/// here — chrono's serde emits a `Z` suffix and `'Z'` sorts above every digit,
/// so `...T12:00:00Z` compares greater than `...T23:00:00+00:00` — and getting
/// it wrong deletes live entities.
///
/// An `updated_at` that is absent or will not parse counts as **modified**, so
/// the deletion is blocked and reported. It is the conservative direction: a
/// deletion that fails to apply costs the user one keystroke, and a deletion
/// that should not have applied costs them their work.
fn modified_since<T: Identified>(entity: &T, stone: &WireTombstone) -> bool {
    let deleted = match parse_time(&stone.deleted_at) {
        Some(t) => t,
        // A stone whose own date is unreadable cannot authorize anything.
        None => return true,
    };
    match entity.updated_at().and_then(parse_time) {
        Some(updated) => updated > deleted,
        None => true,
    }
}

// ============================================================================
// The plan
// ============================================================================

/// Merges one kind and returns everything that would have to happen.
///
/// `ancestor_of` is handed an entity id and returns its base snapshot. It is a
/// closure rather than a base lookup done in here because this module does no
/// I/O — and because that is what keeps a corrupt base file to one entity: the
/// caller writes `Ancestor::from_load(base::load(kind, id))` and a failure
/// arrives as a value, against that entity, instead of as an error that aborts
/// the run.
///
/// Bases are looked up by **our** id only. The base directory records what
/// *this* machine last synced, so a peer-only entity has no ancestor here by
/// definition.
///
/// Each entity is settled into its own plan first, and the pieces are only
/// joined once [`settle_name_collisions`] has checked that no two of them claim
/// one filename. This is the last place that knows *why* two changes exist —
/// downstream there are only changes, and two changes that each look ordinary
/// are how a rename into an occupied name destroys an entity on both machines.
pub fn plan_kind<T: Mergeable>(
    ours: &[T],
    theirs: &[T],
    ancestor_of: impl Fn(&str) -> Ancestor<T>,
    our_tombstones: &[WireTombstone],
    their_tombstones: &[WireTombstone],
    house: Side,
) -> Result<MergePlan> {
    let ours_stones = Tombstones::for_kind(T::KIND, our_tombstones);
    let theirs_stones = Tombstones::for_kind(T::KIND, their_tombstones);

    let mut settled: Vec<Settled> = Vec::new();
    for (ours, theirs) in pair_up(ours, theirs) {
        let mut one = MergePlan::default();
        settle_one(&mut one, ours, theirs, &ancestor_of, &ours_stones, &theirs_stones, house)?;
        settled.push(Settled {
            plan: one,
            ours: holder_of(ours),
            theirs: holder_of(theirs),
        });
    }

    Ok(settle_name_collisions::<T>(settled, house))
}

/// One entity's worth of plan, plus what each machine calls that entity *today*.
///
/// The names matter because a filename clash is about who **holds** a name, not
/// about what the merge decided to write: in the case this exists for, the
/// entity whose copy is about to be overwritten contributes no change of its own
/// on the machine where the damage happens.
struct Settled {
    plan: MergePlan,
    /// `(lock key, description)` of this machine's copy, if it has one.
    ours: Option<(String, String)>,
    theirs: Option<(String, String)>,
}

/// How one machine's copy of an entity is identified in a collision report.
fn holder_of<T: Mergeable>(entity: Option<&T>) -> Option<(String, String)> {
    entity.map(|e| {
        let name = e.entity_name();
        (
            crate::store::lock_key(name),
            match e.id() {
                Some(id) => format!("{:?} ({})", name, id),
                None => format!("{:?} (never stamped)", name),
            },
        )
    })
}

/// Refuses any plan in which two different entities would be written to one
/// filename, and says so instead.
///
/// The shape this catches: we rename `api` to `web`, and the peer independently
/// has a different project already called `web`. The rename is ordinary and the
/// peer's project is ordinary, and each is planned without reference to the
/// other — our `web` is then replaced by theirs, theirs by ours, one project is
/// lost on each machine, and the two machines disagree about what `web` means
/// from then on. No field conflicted, so nothing else in this module would have
/// noticed.
///
/// **Keyed on `store::lock_key`, not the raw name.** The mapping is many-to-one,
/// so `"my api"` and `"my_api"` are two entity files sharing one lock file and
/// one tombstone file; `ranch::base::file_stem_for` reached the same verdict for
/// ids and refuses rather than accept the collision. A name clash is the certain
/// loss and a key clash the near miss, and the merge is not the layer to bet on
/// the difference.
///
/// Only the target name counts, never `previous_name`. A rename frees the name
/// it leaves, and two entities swapping names are a legitimate plan with an
/// ordering requirement rather than a clash — that is the applier's problem, not
/// a reason to refuse the sync.
///
/// An entity caught by this keeps the conflicts and notes it learned on the way
/// — they describe real disagreements — and loses only its writes. Both
/// machines keep what they have, the plan pane says which name is contested, and
/// the sync stays refused until the user renames one side. There is no
/// resolution the merge can apply: inventing a third name for somebody's
/// project is not a merge decision.
fn settle_name_collisions<T: Mergeable>(settled: Vec<Settled>, house: Side) -> MergePlan {
    // The kind is fixed for the whole call, so the lock key alone identifies a
    // filename.
    let mut claims: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, one) in settled.iter().enumerate() {
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for change in one.plan.incoming.iter().chain(one.plan.outgoing.iter()) {
            keys.insert(crate::store::lock_key(&change.name));
        }
        for key in keys {
            claims.entry(key).or_default().push(i);
        }
    }

    let mut blocked: BTreeSet<usize> = BTreeSet::new();
    let mut reported: Vec<(Conflict, Note)> = Vec::new();

    for (key, claimants) in &claims {
        if claimants.len() < 2 {
            continue;
        }
        // The name as it will be spelled on disk, taken from the claims
        // themselves so a lock-key clash reports a name that actually exists.
        let name = settled[claimants[0]]
            .plan
            .incoming
            .iter()
            .chain(settled[claimants[0]].plan.outgoing.iter())
            .find(|c| &crate::store::lock_key(&c.name) == key)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| key.clone());

        // Who holds the contested name on each machine. An entity whose copy is
        // about to be overwritten often plans no change of its own on the
        // machine where the damage happens, so this reads the holders rather
        // than the changes.
        let holders = |side: &dyn Fn(&Settled) -> Option<&(String, String)>| -> String {
            let held: Vec<&str> = claimants
                .iter()
                .filter_map(|i| side(&settled[*i]))
                .filter(|(held_key, _)| held_key == key)
                .map(|(_, description)| description.as_str())
                .collect();
            if held.is_empty() {
                "no entity of that name".to_string()
            } else {
                held.join(", ")
            }
        };

        let ours = holders(&|s: &Settled| s.ours.as_ref());
        let theirs = holders(&|s: &Settled| s.theirs.as_ref());

        reported.push((
            Conflict {
                kind: T::KIND.to_string(),
                name: name.clone(),
                // Two entities, so no single id can stand for the conflict; both
                // are named in `ours` and `theirs` instead.
                id: None,
                field: "<name>".to_string(),
                ours: ours.clone(),
                theirs: theirs.clone(),
                // `winner` names the house, whose copy of the name is the one to
                // keep — it is *not* a claim that anything was applied. Nothing
                // was: see the note alongside.
                winner: house,
            },
            Note {
                kind: T::KIND.to_string(),
                name: name.clone(),
                id: None,
                reason: format!(
                    "two different {kind}s would be written to the name {name:?} — {ours} here \
                     and {theirs} on the peer. Applying both would overwrite one with the other \
                     on this machine and the reverse on the peer, so neither was planned and \
                     both machines keep what they have. Rename one of them, then sync again",
                    kind = T::KIND,
                    name = name,
                    ours = ours,
                    theirs = theirs,
                ),
            },
        ));
        blocked.extend(claimants.iter().copied());
    }

    let mut plan = MergePlan::default();
    for (i, one) in settled.into_iter().enumerate() {
        if blocked.contains(&i) {
            // What it learned survives; only the writes that would collide go.
            plan.conflicts.extend(one.plan.conflicts);
            plan.notes.extend(one.plan.notes);
        } else {
            plan.absorb(one.plan);
        }
    }
    for (conflict, note) in reported {
        plan.conflicts.push(conflict);
        plan.notes.push(note);
    }
    plan
}

/// Pairs this machine's entities with the peer's.
///
/// **Uuid first, name second.** A joining machine's uuids were minted
/// independently of the Ranch House's and cannot match, so on a first join name
/// is the only thing that can align the two sets — once. Every sync after that
/// has uuids on both sides and matches on them, which is what makes a rename
/// survive.
///
/// A name is only allowed to pair entities the uuid pass left unmatched, so a
/// uuid match always beats a coincidental name match.
fn pair_up<'a, T: Mergeable>(
    ours: &'a [T],
    theirs: &'a [T],
) -> Vec<(Option<&'a T>, Option<&'a T>)> {
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for (i, entity) in theirs.iter().enumerate() {
        if let Some(id) = entity.id() {
            by_id.entry(id).or_insert(i);
        }
        by_name.entry(entity.entity_name()).or_insert(i);
    }

    let mut taken: HashSet<usize> = HashSet::new();
    let mut matched: Vec<Option<usize>> = vec![None; ours.len()];

    for (i, entity) in ours.iter().enumerate() {
        if let Some(&j) = entity.id().and_then(|id| by_id.get(id)) {
            if taken.insert(j) {
                matched[i] = Some(j);
            }
        }
    }
    for (i, entity) in ours.iter().enumerate() {
        if matched[i].is_some() {
            continue;
        }
        if let Some(&j) = by_name.get(entity.entity_name()) {
            if taken.insert(j) {
                matched[i] = Some(j);
            }
        }
    }

    let mut pairs: Vec<(Option<&T>, Option<&T>)> = Vec::new();
    for (i, entity) in ours.iter().enumerate() {
        pairs.push((Some(entity), matched[i].map(|j| &theirs[j])));
    }
    for (j, entity) in theirs.iter().enumerate() {
        if !taken.contains(&j) {
            pairs.push((None, Some(entity)));
        }
    }
    pairs
}

#[allow(clippy::too_many_arguments)]
fn settle_one<T: Mergeable>(
    plan: &mut MergePlan,
    ours: Option<&T>,
    theirs: Option<&T>,
    ancestor_of: &impl Fn(&str) -> Ancestor<T>,
    ours_stones: &Tombstones,
    theirs_stones: &Tombstones,
    house: Side,
) -> Result<()> {
    // A tombstone from the peer cancels our copy; ours cancels theirs. Both are
    // checked before merging, because there is nothing to merge into an entity
    // one side has thrown away.
    if let Some(entity) = ours {
        if let Some(stone) = theirs_stones.covering(entity.id()) {
            return apply_tombstone(plan, entity, stone, Side::Local, house);
        }
    }
    if let Some(entity) = theirs {
        if let Some(stone) = ours_stones.covering(entity.id()) {
            return apply_tombstone(plan, entity, stone, Side::Remote, house);
        }
    }

    // A fallback-keyed stone cannot match any uuid, so the deletion it records
    // cannot travel and the surviving copy is kept instead. Said in **both**
    // directions: the machine holding the stone is the one that needs telling
    // least, because it already knows what it deleted — it is the machine
    // receiving the resurrecting upsert that has no explanation for it.
    //
    // Only when the other side no longer has the entity. A stone names a name,
    // not an entity, so it goes on matching that name for as long as it lives; an
    // entity alive on both machines is a delete-then-recreate, and calling that a
    // deletion that failed to travel sends the user after a problem that is not
    // there.
    match (ours, theirs) {
        (Some(entity), None) => note_unpropagatable(plan, entity, theirs_stones, Side::Remote),
        (None, Some(entity)) => note_unpropagatable(plan, entity, ours_stones, Side::Local),
        _ => {}
    }

    match (ours, theirs) {
        (Some(ours), Some(theirs)) => {
            let ancestor = match ours.id() {
                Some(id) => ancestor_of(id),
                None => Ancestor::Absent,
            };
            if let Some(reason) = ancestor.unreadable() {
                plan.notes.push(Note {
                    kind: T::KIND.to_string(),
                    name: ours.entity_name().to_string(),
                    id: ours.id().map(|s| s.to_string()),
                    reason: format!(
                        "merged without a common ancestor, so a change made on both machines \
                         picked a side instead of merging: {}",
                        reason
                    ),
                });
            }

            let (merged, mut conflicts) =
                merge_entity(ancestor.known(), ours, theirs, house);
            plan.conflicts.append(&mut conflicts);

            let mut for_local = merged.clone();
            for_local.take_machine_local_from(ours);
            let mut for_remote = merged.clone();
            for_remote.take_machine_local_from(theirs);

            if differs(ours, &for_local)? {
                plan.incoming.push(upsert(&for_local, rename_of(ours, &for_local))?);
            }
            if differs(theirs, &for_remote)? {
                plan.outgoing.push(upsert(&for_remote, rename_of(theirs, &for_remote))?);
            }
        }
        (Some(ours), None) => {
            // The peer has never seen it, so it carries none of their local
            // verdicts about it.
            let mut for_remote = ours.clone();
            for_remote.clear_machine_local();
            plan.outgoing.push(upsert(&for_remote, None)?);
        }
        (None, Some(theirs)) => {
            let mut for_local = theirs.clone();
            for_local.clear_machine_local();
            plan.incoming.push(upsert(&for_local, None)?);
        }
        (None, None) => {}
    }

    Ok(())
}

/// Records that a fallback-keyed tombstone exists for this entity's name and
/// cannot reach it.
///
/// `deleter` is the machine whose tombstone it is, and it decides the whole
/// wording: the machine that did the deleting knows what it deleted, and the
/// *other* one is the one staring at an upsert with no reason attached. Both
/// directions are wired, because only one of them was and the unwired half is the
/// one that surprises the user.
fn note_unpropagatable<T: Mergeable>(
    plan: &mut MergePlan,
    entity: &T,
    stones: &Tombstones,
    deleter: Side,
) {
    let Some(stone) = stones.fallback_for(entity.entity_name()) else {
        return;
    };

    let outcome = match deleter {
        Side::Remote =>
            "so the deletion cannot propagate and the local copy is kept. Delete it here too \
             if that is what you meant",
        Side::Local =>
            "so the deletion cannot propagate: the peer still holds its copy, which is \
             arriving here as an upsert rather than being deleted there. Delete it on the \
             peer too if that is what you meant",
    };

    plan.notes.push(Note {
        kind: T::KIND.to_string(),
        name: entity.entity_name().to_string(),
        id: entity.id().map(|s| s.to_string()),
        reason: format!(
            "{} a {} named {:?} at {}, but recorded the deletion under the fallback key {:?} \
             because that entity had never been stamped with a uuid. A fallback key cannot \
             match any uuid, {}",
            match deleter {
                Side::Remote => "the peer deleted",
                Side::Local => "this machine deleted",
            },
            stone.kind,
            stone.name,
            stone.deleted_at,
            stone.id,
            outcome
        ),
    });
}

/// One side deleted it. The other side keeps it only if it has been touched
/// since — otherwise the deletion applies.
fn apply_tombstone<T: Mergeable>(
    plan: &mut MergePlan,
    entity: &T,
    stone: &WireTombstone,
    holder: Side,
    house: Side,
) -> Result<()> {
    if modified_since(entity, stone) {
        // The entity survives. Recorded as a conflict rather than resolved
        // silently: one machine believes this is deleted and the other has
        // edits on it, and only the user can say which they meant.
        plan.conflicts.push(Conflict {
            kind: T::KIND.to_string(),
            name: entity.entity_name().to_string(),
            id: entity.id().map(|s| s.to_string()),
            field: "<deleted>".to_string(),
            ours: match holder {
                Side::Local => format!("kept, last edited {:?}", entity.updated_at().unwrap_or("never")),
                Side::Remote => format!("deleted at {}", stone.deleted_at),
            },
            theirs: match holder {
                Side::Local => format!("deleted at {}", stone.deleted_at),
                Side::Remote => format!("kept, last edited {:?}", entity.updated_at().unwrap_or("never")),
            },
            winner: holder,
        });

        // The surviving copy goes back to the machine that deleted it, so both
        // machines hold the entity again and neither loses the edit.
        //
        // What this does *not* do is settle the argument. The tombstone stays
        // where it is — `tombstones` exposes `record`, `load_all` and `reap` and
        // nothing that retracts a stone — so it ships again on the next sync,
        // `modified_since` is still true, and this same conflict is reported
        // again. It recurs every sync until the user acts: delete the entity on
        // the machine that kept it, or let the stone age out of `reap`'s 90-day
        // window.
        //
        // Follow-up, deliberately not done here: a `tombstones::remove` called
        // when a stone has been resolved this way. That is a Phase 1 API
        // addition with its own locking and its own tests, and it does not
        // belong inside a pure merge — this module does no I/O.
        let mut restored = entity.clone();
        restored.clear_machine_local();
        let change = upsert(&restored, None)?;
        match holder {
            Side::Local => plan.outgoing.push(change),
            Side::Remote => plan.incoming.push(change),
        }
        let _ = house;
        return Ok(());
    }

    let change = PlannedChange {
        kind: T::KIND.to_string(),
        name: entity.entity_name().to_string(),
        id: entity.id().map(|s| s.to_string()),
        previous_name: None,
        change: Change::Delete,
    };
    match holder {
        Side::Local => plan.incoming.push(change),
        Side::Remote => plan.outgoing.push(change),
    }
    Ok(())
}

/// Whether a side has to be written at all.
///
/// Content is compared by canonical hash, which is the same comparison the
/// manifest exchange makes — so a re-stamp, a `null`-versus-absent difference
/// or a reordered `HashMap` does not count as a change and does not ship the
/// entity.
///
/// `id` is compared separately because it is deliberately *not* in the hash. On
/// a first join the local copy has to adopt the Ranch House's uuid, and that is
/// a write with no content change behind it — the one that makes every
/// subsequent sync able to match by uuid instead of by name.
fn differs<T: Mergeable>(existing: &T, merged: &T) -> Result<bool> {
    if existing.id() != merged.id() {
        return Ok(true);
    }
    Ok(crate::ranch::canonical::hash_entity(T::KIND, existing)?
        != crate::ranch::canonical::hash_entity(T::KIND, merged)?)
}

fn rename_of<T: Mergeable>(existing: &T, merged: &T) -> Option<String> {
    (existing.entity_name() != merged.entity_name())
        .then(|| existing.entity_name().to_string())
}

fn upsert<T: Mergeable>(entity: &T, previous_name: Option<String>) -> Result<PlannedChange> {
    Ok(PlannedChange {
        kind: T::KIND.to_string(),
        name: entity.entity_name().to_string(),
        id: entity.id().map(|s| s.to_string()),
        previous_name,
        change: Change::Upsert { yaml: serde_yaml::to_string(entity)? },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ranch::canonical::hash_entity;
    use crate::trails::Trail;
    use crate::types::{Barn, Critter, Herd, Livestock, Project, RanchHand, WikiSection, Worm};

    // ------------------------------------------------------------------
    // Fixtures
    // ------------------------------------------------------------------

    fn parse<T: DeserializeOwned>(yaml: &str) -> T {
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("fixture will not parse: {}\n{}", e, yaml))
    }

    /// A stamped project, as everything on a ranch is once `ranch init` has run.
    fn proj(name: &str) -> Project {
        parse(&format!(
            "name: {name}\npath: /src/{name}\nid: p-{name}\n\
             created_at: '2026-01-01T00:00:00+00:00'\nupdated_at: '2026-02-01T00:00:00+00:00'\n"
        ))
    }

    fn ls(barn: &str, name: &str) -> Livestock {
        parse(&format!("name: {name}\npath: /srv/{name}\nbarn: {barn}\n"))
    }

    fn wiki(title: &str, content: &str) -> WikiSection {
        WikiSection { title: title.into(), content: content.into() }
    }

    fn barn(name: &str) -> Barn {
        parse(&format!(
            "name: {name}\nhost: {name}.local\nid: b-{name}\n\
             created_at: '2026-01-01T00:00:00+00:00'\nupdated_at: '2026-02-01T00:00:00+00:00'\n"
        ))
    }

    fn critter(name: &str) -> Critter {
        parse(&format!("name: {name}\nservice: {name}.service\n"))
    }

    fn worm(name: &str) -> Worm {
        parse(&format!(
            "name: {name}\ncommand: echo {name}\nschedule: '0 0 * * *'\ntype: cron\nenabled: true\n\
             id: w-{name}\ncreated_at: '2026-01-01T00:00:00+00:00'\nupdated_at: '2026-02-01T00:00:00+00:00'\n"
        ))
    }

    fn ranchhand(name: &str) -> RanchHand {
        parse(&format!(
            "name: {name}\nproject: api\ntype: k8s\nconfig:\n  kubeconfig_path: /home/local/.kube/config\n\
             sync_settings:\n  auto_sync: false\n  interval_minutes: null\nherd: web\n\
             last_sync: '2026-03-01T00:00:00+00:00'\n\
             id: r-{name}\ncreated_at: '2026-01-01T00:00:00+00:00'\nupdated_at: '2026-02-01T00:00:00+00:00'\n"
        ))
    }

    fn trail(name: &str, step: &str) -> Trail {
        parse(&format!(
            "name: {name}\njobs:\n  build:\n    runs-on: native\n    steps:\n      - name: {step}\n        run: make {step}\n\
             id: t-{name}\ncreated_at: '2026-01-01T00:00:00+00:00'\nupdated_at: '2026-02-01T00:00:00+00:00'\n"
        ))
    }

    fn no_base<T>(_: &str) -> Ancestor<T> {
        Ancestor::Absent
    }

    fn base_of<T: Clone>(base: T) -> impl Fn(&str) -> Ancestor<T> {
        move |_| Ancestor::Known(base.clone())
    }

    fn tomb(kind: &str, name: &str, id: &str, at: &str) -> WireTombstone {
        WireTombstone {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            deleted_at: at.into(),
        }
    }

    /// The one incoming entity, or a panic naming what was actually planned.
    fn only_incoming<T: DeserializeOwned>(plan: &MergePlan) -> T {
        assert_eq!(plan.incoming.len(), 1, "expected exactly one incoming change: {:#?}", plan);
        plan.incoming[0].entity().unwrap()
    }

    fn only_outgoing<T: DeserializeOwned>(plan: &MergePlan) -> T {
        assert_eq!(plan.outgoing.len(), 1, "expected exactly one outgoing change: {:#?}", plan);
        plan.outgoing[0].entity().unwrap()
    }

    fn ls_keys(p: &Project) -> Vec<(String, String)> {
        p.livestock.iter().map(livestock_key).collect()
    }

    // ==================================================================
    // C1 — outcome types
    // ==================================================================

    /// The plan pane renders conflicts, so "there was a conflict in project
    /// api" is not enough: a user has to be able to see what was kept, what was
    /// thrown away, and which key in the file to go and look at.
    #[test]
    fn a_conflict_names_the_entity_the_field_and_both_values() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.summary = Some("ours".into());
        let mut theirs = base.clone();
        theirs.summary = Some("theirs".into());

        let (_, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

        assert_eq!(conflicts.len(), 1, "one field disagreed: {:#?}", conflicts);
        let c = &conflicts[0];
        assert_eq!(c.kind, "project");
        assert_eq!(c.name, "api");
        assert_eq!(c.id.as_deref(), Some("p-api"));
        assert_eq!(c.field, "summary");
        assert!(c.ours.contains("ours"), "our value must be inspectable: {:?}", c.ours);
        assert!(c.theirs.contains("theirs"), "their value must be inspectable: {:?}", c.theirs);
        assert_eq!(c.winner, Side::Local);
    }

    /// `base::accept` takes no locks and requires the caller to hold them for
    /// the whole batch, in sorted `store::lock_key` order and deduped on the
    /// key. Sorting by *name* is the mistake: `lock_key` collapses everything
    /// outside `[A-Za-z0-9_-]` to `_`, and `' ' < '-' < '_'`, so the two orders
    /// genuinely disagree. `fs2` locks are not re-entrant and have no timeout,
    /// so a missed dedupe is a hang with no error and no way out but a kill.
    #[test]
    fn lock_targets_come_back_in_lock_key_order_and_deduped_on_the_key() {
        let plan = MergePlan {
            incoming: vec![
                PlannedChange {
                    kind: "project".into(),
                    name: "a b".into(),
                    id: None,
                    previous_name: None,
                    change: Change::Delete,
                },
                PlannedChange {
                    kind: "project".into(),
                    name: "a-b".into(),
                    id: None,
                    previous_name: None,
                    change: Change::Delete,
                },
            ],
            // Same lock key as "a b" — one lock, and taking it twice hangs.
            outgoing: vec![PlannedChange {
                kind: "project".into(),
                name: "a_b".into(),
                id: None,
                previous_name: None,
                change: Change::Delete,
            }],
            ..Default::default()
        };

        // Guard: if these ever stop disagreeing the test is proving nothing.
        assert!(
            "a b" < "a-b" && crate::store::lock_key("a-b") < crate::store::lock_key("a b"),
            "the name order and the lock-key order must disagree for this to have teeth"
        );

        let targets = plan.lock_targets();
        assert_eq!(
            targets,
            vec![("project".to_string(), "a-b".to_string()), ("project".to_string(), "a b".to_string())],
            "targets must be in lock_key order, and the two names sharing a key must appear once"
        );
    }

    /// A rename touches two filenames, and both need locking — the old one is
    /// removed and the new one written.
    #[test]
    fn a_rename_locks_the_name_it_leaves_as_well_as_the_one_it_takes() {
        let plan = MergePlan {
            incoming: vec![PlannedChange {
                kind: "project".into(),
                name: "new".into(),
                id: None,
                previous_name: Some("old".into()),
                change: Change::Upsert { yaml: String::new() },
            }],
            ..Default::default()
        };
        assert_eq!(
            plan.lock_targets(),
            vec![("project".to_string(), "new".to_string()), ("project".to_string(), "old".to_string())]
        );
    }

    #[test]
    fn an_ancestor_distinguishes_absent_from_unreadable() {
        assert_eq!(Ancestor::from_load(Ok(Some(7))), Ancestor::Known(7));
        assert_eq!(Ancestor::<i32>::from_load(Ok(None)), Ancestor::Absent);
        let corrupt: Result<Option<i32>> =
            Err(anyhow::anyhow!("base at /x.yaml is unreadable. Delete the file"));
        match Ancestor::from_load(corrupt) {
            Ancestor::Unreadable(why) => {
                assert!(why.contains("Delete the file"), "the remedy must survive: {:?}", why)
            }
            other => panic!("a corrupt base must not read as absent: {:?}", other),
        }
    }

    /// `base::load` errors on a corrupt file precisely so it is never read as
    /// absent. If the sync loop threaded that error out through `?`, one bad
    /// file would stop every other entity on the ranch from syncing. It must
    /// cost exactly one entity its ancestor, and say so.
    #[test]
    fn one_unreadable_base_degrades_its_own_entity_and_nothing_else() {
        let mut ours_a = proj("api");
        ours_a.summary = Some("ours".into());
        let mut theirs_a = proj("api");
        theirs_a.summary = Some("theirs".into());

        let mut ours_b = proj("web");
        let mut theirs_b = proj("web");
        theirs_b.color = Some("#fff".into());
        ours_b.color = None;

        let plan = plan_kind(
            &[ours_a, ours_b],
            &[theirs_a, theirs_b],
            |id| {
                if id == "p-api" {
                    Ancestor::Unreadable("sync base at /x.yaml is unreadable".into())
                } else {
                    Ancestor::Known(proj("web"))
                }
            },
            &[],
            &[],
            Side::Local,
        )
        .unwrap();

        assert_eq!(plan.notes.len(), 1, "exactly one entity degraded: {:#?}", plan.notes);
        assert_eq!(plan.notes[0].name, "api");
        assert!(
            plan.notes[0].reason.contains("without a common ancestor"),
            "the note must say what was lost: {:?}",
            plan.notes[0].reason
        );

        // And `web`, whose base was fine, merged three-way: only the peer
        // changed `color`, so the peer's value is taken with no conflict.
        let web: Project = plan
            .incoming
            .iter()
            .find(|c| c.name == "web")
            .expect("web must still have been merged")
            .entity()
            .unwrap();
        assert_eq!(web.color.as_deref(), Some("#fff"));
        assert!(
            plan.conflicts.iter().all(|c| c.name != "web"),
            "the healthy entity must not inherit the broken one's degradation"
        );
    }

    // ==================================================================
    // C2 — project scalars, three-way per field
    // ==================================================================

    #[test]
    fn a_field_only_the_peer_changed_takes_the_peer_s_value() {
        let base = proj("api");
        let ours = base.clone();
        let mut theirs = base.clone();
        theirs.summary = Some("the peer wrote this".into());

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

        assert_eq!(merged.summary.as_deref(), Some("the peer wrote this"));
        assert!(conflicts.is_empty(), "one-sided change is not a conflict: {:#?}", conflicts);
    }

    #[test]
    fn a_field_only_we_changed_keeps_our_value_even_when_the_peer_is_the_ranch_house() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.summary = Some("we wrote this".into());
        let theirs = base.clone();

        // House is the peer, and it still must not clobber a change it did not
        // make. This is the entire reason a base exists.
        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Remote);

        assert_eq!(merged.summary.as_deref(), Some("we wrote this"));
        assert!(conflicts.is_empty(), "{:#?}", conflicts);
    }

    #[test]
    fn a_field_changed_on_both_sides_goes_to_the_ranch_house() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.color = Some("#ours".into());
        let mut theirs = base.clone();
        theirs.color = Some("#theirs".into());

        let (local_house, c1) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(local_house.color.as_deref(), Some("#ours"));
        assert_eq!(c1.len(), 1);
        assert_eq!(c1[0].winner, Side::Local);

        let (remote_house, c2) = merge_entity(Some(&base), &ours, &theirs, Side::Remote);
        assert_eq!(remote_house.color.as_deref(), Some("#theirs"));
        assert_eq!(c2.len(), 1);
        assert_eq!(c2[0].winner, Side::Remote);
    }

    /// With no ancestor there is no way to tell "they changed it" from "we
    /// changed it", so the merge can only pick a side — the Ranch House — and
    /// say so.
    #[test]
    fn with_no_base_a_disagreement_falls_to_the_ranch_house_and_is_reported() {
        let mut ours = proj("api");
        ours.summary = Some("ours".into());
        let mut theirs = proj("api");
        theirs.summary = Some("theirs".into());

        let (merged, conflicts) = merge_entity(None, &ours, &theirs, Side::Local);
        assert_eq!(merged.summary.as_deref(), Some("ours"));
        assert_eq!(conflicts.len(), 1, "{:#?}", conflicts);
    }

    /// The four `#[serde(rename)]` fields are the reason this merge runs on
    /// parsed structs. A text-level merge sees `gradientSpread` in the file
    /// while the code says `gradient_spread`, and these four would silently
    /// never merge — no error, no failed round trip, just one machine's theme
    /// winning forever.
    #[test]
    fn the_renamed_project_fields_merge_like_every_other_field() {
        let base: Project = parse(
            "name: api\npath: /src/api\nid: p-api\n\
             gradientSpread: 1.0\ngradientInverted: false\n\
             issueProvider:\n  type: none\nwikiProvider:\n  type: local\n",
        );

        let mut theirs = base.clone();
        theirs.gradient_spread = Some(2.5);
        theirs.gradient_inverted = Some(true);
        theirs.issue_provider = Some(crate::types::IssueProviderConfig::GitHub);
        theirs.wiki_provider = Some(crate::types::WikiProviderConfig::Linear {
            team_id: Some("T1".into()),
            team_name: None,
        });

        let (merged, conflicts) = merge_entity(Some(&base), &base, &theirs, Side::Local);

        assert!(conflicts.is_empty(), "{:#?}", conflicts);
        assert_eq!(merged.gradient_spread, Some(2.5), "gradientSpread did not merge");
        assert_eq!(merged.gradient_inverted, Some(true), "gradientInverted did not merge");
        assert_eq!(
            render(&merged.issue_provider),
            render(&theirs.issue_provider),
            "issueProvider did not merge"
        );
        assert_eq!(
            render(&merged.wiki_provider),
            render(&theirs.wiki_provider),
            "wikiProvider did not merge"
        );
    }

    /// A conflict has to name the key the user will search their file for, and
    /// their file says `gradientSpread`.
    #[test]
    fn a_conflict_names_a_renamed_field_the_way_the_file_spells_it() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.gradient_spread = Some(1.0);
        let mut theirs = base.clone();
        theirs.gradient_spread = Some(2.0);

        let (_, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(conflicts[0].field, "gradientSpread");
    }

    /// Phase 1 stamps `updated_at` on every save, so an entity re-saved with no
    /// edits differs byte-for-byte from the peer's copy. If that counted as a
    /// change, every sync would ship the whole ranch.
    #[test]
    fn an_entity_that_only_got_re_stamped_produces_no_work() {
        let base = proj("api");
        let ours = base.clone();
        let mut theirs = base.clone();
        theirs.updated_at = Some("2027-06-06T06:06:06+00:00".into());

        let plan =
            plan_kind(&[ours], &[theirs], base_of(base), &[], &[], Side::Local).unwrap();
        assert!(plan.is_empty(), "a re-stamp is not an edit: {:#?}", plan);
    }

    // ==================================================================
    // C3 — nested collections
    // ==================================================================

    #[test]
    fn livestock_union_keeps_what_each_machine_has() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.livestock = vec![ls("imac", "web")];
        let mut theirs = base.clone();
        theirs.livestock = vec![ls("pi", "worker")];

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

        assert_eq!(
            ls_keys(&merged),
            vec![("imac".to_string(), "web".to_string()), ("pi".to_string(), "worker".to_string())]
        );
        assert!(conflicts.is_empty(), "livestock can never conflict: {:#?}", conflicts);
    }

    /// The key is `(barn, name)` because a livestock is a codebase *on a
    /// specific machine*. Two barns both running `web` are two livestock.
    #[test]
    fn the_same_livestock_name_on_two_barns_is_two_livestock() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.livestock = vec![ls("imac", "web")];
        let mut theirs = base.clone();
        theirs.livestock = vec![ls("pi", "web")];

        let (merged, _) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(merged.livestock.len(), 2, "the barn is part of the key");
    }

    /// Union means union. A livestock the peer no longer lists is *kept* —
    /// removal takes an explicit tombstone from its owning barn, never a
    /// silent absence, because a peer that has simply never heard of a
    /// livestock is indistinguishable from one that deleted it.
    #[test]
    fn a_livestock_missing_from_the_peer_is_kept_not_deleted() {
        let mut base = proj("api");
        base.livestock = vec![ls("imac", "web"), ls("pi", "worker")];
        let ours = base.clone();
        let mut theirs = base.clone();
        theirs.livestock = vec![ls("imac", "web")];

        let (merged, _) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(merged.livestock.len(), 2, "union never drops an element");
    }

    /// Both machines editing the same livestock cannot be reported as a
    /// conflict — the design says so — but the edit made by whichever side is
    /// *not* the Ranch House must still survive when the house did not touch
    /// it, which is what three-way on the element buys.
    #[test]
    fn a_livestock_edited_only_by_the_non_house_side_keeps_that_edit() {
        let mut base = proj("api");
        base.livestock = vec![ls("pi", "worker")];
        let ours = base.clone();
        let mut theirs = base.clone();
        theirs.livestock[0].branch = Some("release".into());

        // House is us, and the peer's edit still survives.
        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(merged.livestock[0].branch.as_deref(), Some("release"));
        assert!(conflicts.is_empty(), "only one side moved: {:#?}", conflicts);
    }

    /// The other half, which the design does *not* excuse.
    ///
    /// "Never overwritten, never conflicting" is about the **key set** — a
    /// livestock is never deleted because the peer has not heard of it. It is not
    /// a licence to discard a field edit invisibly: with both machines having
    /// moved `branch`, one of the two edits is thrown away, and the plan pane is
    /// the only place the user could ever find out.
    #[test]
    fn a_livestock_edited_on_both_machines_says_whose_edit_was_discarded() {
        let mut base = proj("api");
        base.livestock = vec![ls("pi", "worker")];
        let mut ours = base.clone();
        ours.livestock[0].branch = Some("ours".into());
        let mut theirs = base.clone();
        theirs.livestock[0].branch = Some("theirs".into());

        // The peer is the Ranch House, so its edit is the one that survives.
        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Remote);

        assert_eq!(merged.livestock[0].branch.as_deref(), Some("theirs"));
        assert_eq!(
            conflicts.len(),
            1,
            "an overwritten edit that is never reported is an edit the user loses \
             silently: {:#?}",
            conflicts
        );
        assert_eq!(conflicts[0].field, "livestock[pi/worker].branch");
        assert_eq!(conflicts[0].ours, "ours");
        assert_eq!(conflicts[0].theirs, "theirs");
        assert_eq!(conflicts[0].winner, Side::Remote);
    }

    /// Two keys moved at once leaves nothing to name, so the whole record is
    /// shown rather than one key guessed at.
    #[test]
    fn an_element_edited_in_two_places_at_once_shows_the_whole_record() {
        let mut base = proj("api");
        base.livestock = vec![ls("pi", "worker")];
        let mut ours = base.clone();
        ours.livestock[0].branch = Some("ours".into());
        ours.livestock[0].repo = Some("ours-repo".into());
        let mut theirs = base.clone();
        theirs.livestock[0].branch = Some("theirs".into());
        theirs.livestock[0].repo = Some("theirs-repo".into());

        let (_, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(conflicts.len(), 1, "{:#?}", conflicts);
        assert_eq!(conflicts[0].field, "livestock[pi/worker]");
        assert!(conflicts[0].ours.contains("ours-repo"), "{:?}", conflicts[0].ours);
        assert!(conflicts[0].theirs.contains("theirs-repo"), "{:?}", conflicts[0].theirs);
    }

    #[test]
    fn critters_union_under_their_barn() {
        let base = barn("pi");
        let mut ours = base.clone();
        ours.critters = vec![critter("nginx")];
        let mut theirs = base.clone();
        theirs.critters = vec![critter("redis")];

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        let names: Vec<&str> = merged.critters.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["nginx", "redis"]);
        assert!(conflicts.is_empty(), "{:#?}", conflicts);
    }

    /// Critters union under their barn, but an element both machines edited is
    /// the same problem livestock has: the house's copy replaces the peer's, and
    /// saying nothing about it is how the edit disappears.
    #[test]
    fn a_critter_edited_on_both_machines_says_whose_edit_was_discarded() {
        let mut base = barn("pi");
        base.critters = vec![critter("nginx")];
        let mut ours = base.clone();
        ours.critters[0].service = "nginx-ours.service".into();
        let mut theirs = base.clone();
        theirs.critters[0].service = "nginx-theirs.service".into();

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

        assert_eq!(merged.critters[0].service, "nginx-ours.service", "the house wins");
        assert_eq!(conflicts.len(), 1, "{:#?}", conflicts);
        assert_eq!(conflicts[0].field, "critters[pi/nginx].service");
        assert_eq!(conflicts[0].ours, "nginx-ours.service");
        assert_eq!(conflicts[0].theirs, "nginx-theirs.service");
        assert_eq!(conflicts[0].winner, Side::Local);
    }

    #[test]
    fn herd_member_lists_union() {
        let mut base = proj("api");
        base.herds = vec![Herd {
            name: "web".into(),
            livestock: vec!["shared".into()],
            critters: vec![],
            connections: vec![],
        }];

        let mut ours = base.clone();
        ours.herds[0].livestock.push("ours".into());
        ours.herds[0].critters.push(crate::types::HerdCritterRef {
            barn: "imac".into(),
            critter: "nginx".into(),
        });

        let mut theirs = base.clone();
        theirs.herds[0].livestock.push("theirs".into());
        theirs.herds[0].critters.push(crate::types::HerdCritterRef {
            barn: "pi".into(),
            critter: "redis".into(),
        });

        let (merged, _) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

        assert_eq!(merged.herds.len(), 1);
        // Inherited, not sorted: the Ranch House's list in the order it wrote
        // it, then the peer's addition.
        assert_eq!(merged.herds[0].livestock, vec!["shared", "ours", "theirs"]);
        assert_eq!(
            merged.herds[0]
                .critters
                .iter()
                .map(|c| (c.barn.as_str(), c.critter.as_str()))
                .collect::<Vec<_>>(),
            vec![("imac", "nginx"), ("pi", "redis")]
        );
    }

    #[test]
    fn wiki_sections_union_by_title() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.wiki = vec![wiki("Architecture", "ours")];
        let mut theirs = base.clone();
        theirs.wiki = vec![wiki("Deploy", "theirs")];

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(
            merged.wiki.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            vec!["Architecture", "Deploy"]
        );
        assert!(conflicts.is_empty(), "{:#?}", conflicts);
    }

    /// Two people editing the same section of the same document is exactly the
    /// case that must not resolve silently.
    #[test]
    fn a_same_title_wiki_collision_goes_to_the_ranch_house_and_is_reported() {
        let mut base = proj("api");
        base.wiki = vec![wiki("Deploy", "original")];
        let mut ours = base.clone();
        ours.wiki[0].content = "ours".into();
        let mut theirs = base.clone();
        theirs.wiki[0].content = "theirs".into();

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Remote);

        assert_eq!(merged.wiki[0].content, "theirs", "the Ranch House wins the title");
        assert_eq!(conflicts.len(), 1, "{:#?}", conflicts);
        assert_eq!(conflicts[0].field, "wiki[Deploy]");
        assert_eq!(conflicts[0].ours, "ours");
        assert_eq!(conflicts[0].theirs, "theirs");
        assert_eq!(conflicts[0].winner, Side::Remote);
    }

    /// Wiki sections go through the collection path, not the scalar one, so
    /// "only one side moved" has to be answered there too — otherwise the
    /// Ranch House silently overwrites every section the other machine edited.
    #[test]
    fn a_wiki_section_edited_only_by_the_non_house_side_keeps_that_edit() {
        let mut base = proj("api");
        base.wiki = vec![wiki("Deploy", "original")];
        let mut ours = base.clone();
        ours.wiki[0].content = "we rewrote this".into();
        let theirs = base.clone();

        // The peer is the Ranch House and did not touch the section.
        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Remote);
        assert_eq!(merged.wiki[0].content, "we rewrote this");
        assert!(conflicts.is_empty(), "only one side moved: {:#?}", conflicts);
    }

    /// `canonical.rs` treats `Vec` order as content — correctly, since trail
    /// steps and wiki sections are order-significant. So a union that emitted
    /// its elements in whatever order they arrived would make two machines that
    /// *agree* on the contents hash differently, each see the other as changed,
    /// ship, re-merge, and never converge.
    #[test]
    fn a_union_emits_the_same_order_whichever_order_it_went_in() {
        let base = proj("api");

        let mut a = base.clone();
        a.livestock = vec![ls("pi", "worker"), ls("imac", "web")];
        a.wiki = vec![wiki("Deploy", "d"), wiki("Architecture", "a")];

        let mut b = base.clone();
        b.livestock = vec![ls("imac", "web"), ls("pi", "worker")];
        b.wiki = vec![wiki("Architecture", "a"), wiki("Deploy", "d")];

        // Guard: the two inputs really are in opposite orders.
        assert_ne!(ls_keys(&a), ls_keys(&b), "the inputs must disagree on order");

        let (from_a, _) = merge_entity(Some(&base), &a, &b, Side::Local);
        let (from_b, _) = merge_entity(Some(&base), &b, &a, Side::Remote);

        assert_eq!(
            hash_entity("project", &from_a).unwrap(),
            hash_entity("project", &from_b).unwrap(),
            "two machines that agree on the contents must agree on the hash, or sync never converges"
        );
    }

    /// The order is **inherited**, in three tiers: the base's order first, then
    /// the Ranch House's, then the peer's additions. It is not sorted.
    ///
    /// Measured against a copy of a real ranch: sorting by the union key turned
    /// a hand-authored wiki — `Architecture, Conventions, Commands, Domain
    /// Context, Common Tasks, Gotchas` — into alphabetical order and rewrote 12
    /// of 17 projects nobody had touched. `canonical.rs` refuses to sort
    /// sequences for exactly this reason; the merge must not undo that one layer
    /// up.
    #[test]
    fn a_union_keeps_the_order_the_collection_already_had() {
        let mut base = proj("api");
        base.wiki = vec![
            wiki("Architecture", "a"),
            wiki("Conventions", "c"),
            wiki("Commands", "m"),
        ];

        // The Ranch House appends a section; so does the peer. Neither touched
        // the three that were already there.
        let mut house = base.clone();
        house.wiki.push(wiki("Zebra", "house addition"));
        let mut peer = base.clone();
        peer.wiki.push(wiki("Aardvark", "peer addition"));

        let (merged, _) = merge_entity(Some(&base), &house, &peer, Side::Local);

        assert_eq!(
            merged.wiki.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            vec!["Architecture", "Conventions", "Commands", "Zebra", "Aardvark"],
            "the ancestor's order, then the house's addition, then the peer's — \
             alphabetical order would have put Aardvark first and reshuffled the document"
        );

        // And the peer, merging the same three inputs, agrees — which is what
        // convergence requires.
        let (from_peer, _) = merge_entity(Some(&base), &peer, &house, Side::Remote);
        assert_eq!(
            hash_entity("project", &merged).unwrap(),
            hash_entity("project", &from_peer).unwrap(),
            "both machines must compute the same order"
        );
    }

    /// The ancestor's order is the *first* tier, not a fallback, and this is the
    /// case that needs it: an element the Ranch House no longer lists but the
    /// peer still does is kept by the union, and it belongs where it was — not
    /// appended after everything the house has, as if the peer had just written
    /// it.
    #[test]
    fn an_element_the_house_dropped_keeps_its_place_from_the_ancestor() {
        let mut base = proj("api");
        base.wiki = vec![wiki("Intro", "i"), wiki("Middle", "m"), wiki("End", "e")];

        let mut house = base.clone();
        house.wiki.retain(|s| s.title != "Middle");
        let peer = base.clone();

        let (merged, _) = merge_entity(Some(&base), &house, &peer, Side::Local);
        assert_eq!(
            merged.wiki.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            vec!["Intro", "Middle", "End"],
            "the kept section must go back where the ancestor had it"
        );
    }

    /// The same thing, end to end, in the shape the design actually warns
    /// about: **both machines merge**, each from its own point of view, and
    /// they have to land on the same answer.
    ///
    /// A merge whose output order is merely *deterministic* — local order
    /// first, peer-only entries appended — passes a one-sided test, because
    /// only one machine computes and it ships its answer to the other. It fails
    /// here, which is the case that matters: two machines that agree on the
    /// contents each see the other as changed, ship, re-merge, and never
    /// converge. The order has to be a function of the contents.
    #[test]
    fn two_machines_that_disagree_only_about_order_settle_in_one_round() {
        let base = proj("api");
        let mut on_a = base.clone();
        on_a.livestock = vec![ls("pi", "worker"), ls("imac", "web")];
        let mut on_b = base.clone();
        on_b.livestock = vec![ls("imac", "web"), ls("pi", "worker")];

        // A is the Ranch House. A syncs (A is local); B syncs (A is the peer).
        let (a_merged, _) = merge_entity(Some(&base), &on_a, &on_b, Side::Local);
        let (b_merged, _) = merge_entity(Some(&base), &on_b, &on_a, Side::Remote);

        assert_eq!(
            hash_entity("project", &a_merged).unwrap(),
            hash_entity("project", &b_merged).unwrap(),
            "the two machines merged to different content:\nA: {:?}\nB: {:?}",
            ls_keys(&a_merged),
            ls_keys(&b_merged)
        );

        // And with both settled there, the next sync has nothing to do.
        let second = plan_kind(
            &[a_merged.clone()],
            &[b_merged],
            base_of(a_merged),
            &[],
            &[],
            Side::Local,
        )
        .unwrap();
        assert!(second.is_empty(), "the second sync must have nothing to do: {:#?}", second);
    }

    // ==================================================================
    // C4 — the remaining kinds
    // ==================================================================

    /// `Barn.connectable` is a reachability *verdict about this machine*, not a
    /// property of the barn. `connect.rs` hard-refuses a barn marked
    /// `Some(false)`, so a Ranch House that cannot reach a barn pushing its
    /// verdict onto a laptop that can takes away the laptop's ability to
    /// connect to a host it can demonstrably reach.
    #[test]
    fn barn_connectable_never_crosses_the_wire_in_either_direction() {
        let mut base = barn("pi");
        base.connectable = None;

        let mut ours = base.clone();
        ours.connectable = Some(true);
        ours.user = Some("cam".into());

        let mut theirs = base.clone();
        theirs.connectable = Some(false);

        let plan = plan_kind(&[ours], &[theirs], base_of(base), &[], &[], Side::Local).unwrap();

        // We changed `user`, so the peer needs it — and must not receive our
        // reachability verdict along with it.
        let sent: Barn = only_outgoing(&plan);
        assert_eq!(sent.user.as_deref(), Some("cam"));
        assert_eq!(
            sent.connectable,
            Some(false),
            "the copy sent to the peer keeps the peer's own verdict"
        );
        assert!(
            plan.conflicts.is_empty(),
            "a machine-local field is not a change either side made: {:#?}",
            plan.conflicts
        );
    }

    /// `Project.path` is where the checkout lives on *this* machine.
    ///
    /// The design rejected "syncing filesystem paths verbatim, which breaks the
    /// moment a Linux Pi joins", and the Layer 3 table lists no `path` among the
    /// project scalars. Merged three-way it would hand the Ranch House's
    /// `/Users/cam/Sites/api` to a Pi, after which `expand_path(&project.path)`
    /// on the Pi opens nothing — no session and no trail.
    #[test]
    fn a_project_path_never_crosses_the_wire_in_either_direction() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.path = "/Users/cam/Sites/api".into();
        let mut theirs = base.clone();
        theirs.path = "/home/cam/api".into();

        let plan =
            plan_kind(&[ours], &[theirs], base_of(base), &[], &[], Side::Local).unwrap();

        assert!(
            plan.is_empty(),
            "two machines keeping a project in different places have nothing to sync \
             about it: {:#?}",
            plan
        );
        assert!(
            plan.conflicts.is_empty(),
            "a machine-local field is not a change either side made: {:#?}",
            plan.conflicts
        );
    }

    /// And a second field moving does not drag the path along with it.
    #[test]
    fn a_project_that_does_need_syncing_still_leaves_its_path_alone() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.path = "/Users/cam/Sites/api".into();
        ours.summary = Some("ours".into());
        let mut theirs = base.clone();
        theirs.path = "/home/cam/api".into();

        let plan =
            plan_kind(&[ours], &[theirs], base_of(base), &[], &[], Side::Local).unwrap();

        let sent: Project = only_outgoing(&plan);
        assert_eq!(sent.summary.as_deref(), Some("ours"), "the summary travels");
        assert_eq!(sent.path, "/home/cam/api", "the peer keeps its own checkout path");
        assert!(plan.incoming.is_empty(), "nothing for us to apply: {:#?}", plan);
    }

    /// A project this machine has never seen has no local checkout, and the
    /// sender's path is not one — the same reasoning as `Barn.connectable`.
    /// Inheriting it would leave the TUI offering to open a directory that does
    /// not exist here, which reads as a broken project rather than an unconfigured
    /// one.
    #[test]
    fn a_project_arriving_for_the_first_time_brings_no_checkout_path_with_it() {
        let mut theirs = proj("api");
        theirs.path = "/home/cam/api".into();

        let plan = plan_kind(&[], &[theirs], no_base, &[], &[], Side::Remote).unwrap();
        let arrived: Project = only_incoming(&plan);
        assert_eq!(
            arrived.path, "",
            "the sender's checkout path is not this machine's, and claiming it is makes the \
             project look broken instead of unconfigured"
        );
    }

    /// The narrow version, and the one nothing pinned.
    ///
    /// Every other machine-local test moves a second, real field as well, so all
    /// of them would still pass if a machine-local difference *by itself* planned
    /// a write: there would be a write to plan regardless. The cost of that is a
    /// sync that ships the entity every time and never converges, because the
    /// field it is shipping is one neither side is allowed to win.
    #[test]
    fn a_machine_local_difference_on_its_own_is_no_work_at_all() {
        let base = barn("pi");
        let mut ours = base.clone();
        ours.connectable = Some(true);
        let mut theirs = base.clone();
        theirs.connectable = Some(false);

        let plan = plan_kind(&[ours], &[theirs], base_of(base), &[], &[], Side::Local).unwrap();
        assert!(plan.is_empty(), "a reachability verdict is not work: {:#?}", plan);
    }

    #[test]
    fn a_barn_arriving_for_the_first_time_brings_no_reachability_verdict_with_it() {
        let mut theirs = barn("pi");
        theirs.connectable = Some(false);

        let plan = plan_kind(&[], &[theirs], no_base, &[], &[], Side::Remote).unwrap();
        let arrived: Barn = only_incoming(&plan);
        assert_eq!(
            arrived.connectable, None,
            "the peer's verdict about a barn this machine has never tried is not our verdict, \
             and `Some(false)` would refuse the connection before we ever attempted it"
        );
    }

    /// `config` holds a kubeconfig path that resolves only on the machine that
    /// wrote it; `last_sync` is `Utc::now()` from the last discovery run and so
    /// differs on both sides essentially always. Merging either yields a
    /// permanent conflict on every ranch hand, forever.
    #[test]
    fn ranchhand_config_and_last_sync_stay_on_the_machine_that_wrote_them() {
        let base = ranchhand("prod");
        let mut ours = base.clone();
        ours.config = parse("kubeconfig_path: /home/local/.kube/config\n");
        ours.last_sync = Some("2026-09-09T10:00:00+00:00".into());
        ours.herd = "api".into();

        let mut theirs = base.clone();
        theirs.config = parse("kubeconfig_path: /home/peer/.kube/config\n");
        theirs.last_sync = Some("2026-09-09T11:11:11+00:00".into());

        let plan = plan_kind(&[ours.clone()], &[theirs.clone()], base_of(base), &[], &[], Side::Local)
            .unwrap();

        assert!(
            plan.conflicts.is_empty(),
            "machine-local fields must not conflict: {:#?}",
            plan.conflicts
        );
        let sent: RanchHand = only_outgoing(&plan);
        assert_eq!(sent.herd, "api", "the definition travels");
        assert_eq!(
            render(&sent.config),
            render(&theirs.config),
            "the peer keeps its own kubeconfig path"
        );
        assert_eq!(sent.last_sync, theirs.last_sync, "the peer keeps its own last_sync");
    }

    // ==================================================================
    // D1 — the six ranch fields
    // ==================================================================

    /// `brand` and `is_ranch_house` are content, so they have to *travel*; the
    /// three machine-local ones must not. The two halves are one test because
    /// the failure mode is getting the split wrong, and either direction alone
    /// would pass with the split inverted for the other.
    #[test]
    fn a_barns_brand_travels_while_its_local_bookkeeping_stays_home() {
        let base = barn("pi");

        let mut ours = base.clone();
        ours.synced = Some(true);
        ours.tunnel_port = Some(2222);
        ours.last_seen = Some("2026-09-10T09:00:00+00:00".into());

        let mut theirs = base.clone();
        theirs.brand = Some("ssh-ed25519 PEERKEY yeehaw-ranch-pi".into());
        theirs.is_ranch_house = Some(true);
        theirs.synced = Some(false);
        theirs.tunnel_port = Some(9999);
        theirs.last_seen = Some("2026-09-10T11:11:11+00:00".into());

        let plan =
            plan_kind(&[ours.clone()], &[theirs.clone()], base_of(base), &[], &[], Side::Remote)
                .unwrap();

        assert!(
            plan.conflicts.is_empty(),
            "machine-local fields must not conflict: {:#?}",
            plan.conflicts
        );

        let arrived: Barn = only_incoming(&plan);
        assert_eq!(
            arrived.brand, theirs.brand,
            "a brand that does not arrive is an `authorized_keys` block with nothing in it"
        );
        assert_eq!(
            arrived.is_ranch_house,
            Some(true),
            "a machine that cannot learn who the house is cannot use the house-wins tie-break"
        );
        assert_eq!(arrived.synced, ours.synced, "our sync relationship is ours");
        assert_eq!(arrived.tunnel_port, ours.tunnel_port, "our forwarded port is ours");
        assert_eq!(
            arrived.last_seen, ours.last_seen,
            "their clock must not overwrite when *we* last reached the barn"
        );
    }

    /// A barn this machine has never tried arrives with no local bookkeeping at
    /// all — the same reasoning as `connectable`, extended to the three Slice D
    /// fields. `last_seen` from the peer would claim we had reached a host we
    /// have never contacted; `synced: Some(true)` would claim an enrolment this
    /// machine never made.
    #[test]
    fn a_new_barn_arrives_with_none_of_the_peers_local_bookkeeping() {
        let mut theirs = barn("pi");
        theirs.brand = Some("ssh-ed25519 PEERKEY".into());
        theirs.synced = Some(true);
        theirs.tunnel_port = Some(2222);
        theirs.last_seen = Some("2026-09-10T11:11:11+00:00".into());

        let plan = plan_kind(&[], &[theirs], no_base, &[], &[], Side::Remote).unwrap();
        let arrived: Barn = only_incoming(&plan);

        assert!(arrived.brand.is_some(), "the brand is content and must still arrive");
        assert_eq!(arrived.synced, None, "we have not enrolled a barn we just heard of");
        assert_eq!(arrived.tunnel_port, None, "we have allocated no port for it");
        assert_eq!(arrived.last_seen, None, "we have never reached it, so we never saw it");
    }

    /// Addresses union rather than pick, and the order is the house's first.
    ///
    /// Order is not cosmetic: `canonical::render` hashes sequence order, so if
    /// the two sides unioned into different orders each would read the other as
    /// changed on every sync, forever. Asserting the exact vector is what pins
    /// that — `assert_eq` on a sorted copy would let a nondeterministic order
    /// through.
    #[test]
    fn a_barns_addresses_union_in_an_order_both_machines_compute_alike() {
        let base = barn("pi");
        let mut ours = base.clone();
        ours.addresses = vec!["10.0.0.2".into(), "pi.local".into()];
        let mut theirs = base.clone();
        theirs.addresses = vec!["100.64.0.3".into(), "pi.local".into()];

        // Run it from both sides with the same house, which is the thing that has
        // to agree: `Side` names who the house is, not who is running the sync.
        let (house_local, _) =
            merge_entity(Some(&base), &ours, &theirs, Side::Local);
        let (house_local_mirrored, _) =
            merge_entity(Some(&base), &theirs, &ours, Side::Remote);

        assert_eq!(
            house_local.addresses,
            vec!["10.0.0.2".to_string(), "pi.local".to_string(), "100.64.0.3".to_string()],
            "the house's list first in its own order, then the peer's additions"
        );
        assert_eq!(
            house_local.addresses, house_local_mirrored.addresses,
            "both machines must compute the same order or each sees the other as changed forever"
        );

        // And the union really is a union: neither side's address was dropped.
        for want in ["10.0.0.2", "pi.local", "100.64.0.3"] {
            assert!(
                house_local.addresses.iter().any(|a| a == want),
                "{} was lost: {:?}",
                want,
                house_local.addresses
            );
        }
    }

    #[test]
    fn worm_fields_merge_three_way_including_the_one_serialized_as_type() {
        let base = worm("nightly");
        let mut ours = base.clone();
        ours.schedule = "30 2 * * *".into();
        let mut theirs = base.clone();
        theirs.worm_type = "claude".into();

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert_eq!(merged.schedule, "30 2 * * *", "our change survived");
        assert_eq!(merged.worm_type, "claude", "their change survived");
        assert!(conflicts.is_empty(), "{:#?}", conflicts);

        let mut both = base.clone();
        both.worm_type = "shell-other".into();
        let (_, clash) = merge_entity(Some(&base), &both, &theirs, Side::Local);
        assert_eq!(clash[0].field, "type", "the conflict must name the key the file uses");
    }

    /// Everything in this module rests on [`same`], so it must not inherit
    /// `HashMap`'s iteration order.
    ///
    /// `Trail.env`, `TrailJob.env` and `TrailStep.env` are all
    /// `Option<HashMap<..>>`, and `RandomState` reseeds per map instance — two
    /// twelve-key maps built back to back agree on an order roughly half the
    /// time. If `same` were order-sensitive, an unchanged trail would *randomly*
    /// report as changed, conflict against itself, and the merge would be
    /// nondeterministic from one run to the next.
    ///
    /// 200 rounds because one round is a coin flip. The guard inside asserts the
    /// orders genuinely differ at least once, so a future `serde_yaml` that
    /// switched to a sorted map cannot make this pass vacuously.
    #[test]
    fn value_equality_ignores_hashmap_iteration_order() {
        let mut orders_differed = false;

        for round in 0..200 {
            let keys: Vec<String> = (0..12).map(|i| format!("KEY_{}", i)).collect();

            let mut forward = std::collections::HashMap::new();
            for (i, k) in keys.iter().enumerate() {
                forward.insert(k.clone(), format!("v{}", i));
            }
            let mut backward = std::collections::HashMap::new();
            for (i, k) in keys.iter().enumerate().rev() {
                backward.insert(k.clone(), format!("v{}", i));
            }

            if forward.keys().collect::<Vec<_>>() != backward.keys().collect::<Vec<_>>() {
                orders_differed = true;
            }

            let mut ours = trail("deploy", "build");
            ours.env = Some(forward);
            let mut theirs = trail("deploy", "build");
            theirs.env = Some(backward);

            assert!(
                same(&ours.env, &theirs.env),
                "round {} — two maps with identical contents compared unequal",
                round
            );

            let (merged, conflicts) = merge_entity(Some(&ours), &ours, &theirs, Side::Local);
            assert!(conflicts.is_empty(), "round {} — {:#?}", round, conflicts);
            assert_eq!(
                hash_entity("trail", &merged).unwrap(),
                hash_entity("trail", &ours).unwrap(),
                "round {} — the merge moved an unchanged trail",
                round
            );
        }

        assert!(
            orders_differed,
            "the two maps never iterated differently, so this test proved nothing"
        );
    }

    /// `Trail` and `TrailStep` carry hand-written `PartialEq` impls that compare
    /// **only `name`**. A merge built on `==` would call two completely
    /// different trails equal, decide nothing had changed, and drop every edit
    /// silently.
    #[test]
    fn two_trails_with_one_name_and_different_steps_are_not_the_same_trail() {
        let ours = trail("deploy", "build");
        let theirs = trail("deploy", "ship");

        // The trap, pinned: this is what `==` says.
        assert_eq!(ours, theirs, "PartialEq for Trail compares only the name");

        let base = ours.clone();
        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
        assert!(
            conflicts.is_empty(),
            "only the peer moved, so there is nothing to conflict: {:#?}",
            conflicts
        );
        assert_eq!(
            render(&merged.jobs),
            render(&theirs.jobs),
            "the peer's steps must have been taken; `==` would have reported no change"
        );
    }

    #[test]
    fn a_trail_collision_goes_to_the_ranch_house() {
        let base = trail("deploy", "build");
        let ours = trail("deploy", "ours");
        let theirs = trail("deploy", "theirs");

        let (merged, conflicts) = merge_entity(Some(&base), &ours, &theirs, Side::Remote);
        assert_eq!(render(&merged.jobs), render(&theirs.jobs));
        assert_eq!(conflicts.len(), 1, "{:#?}", conflicts);
        assert_eq!(conflicts[0].field, "jobs");
    }

    // ==================================================================
    // Matching: uuid first, name once
    // ==================================================================

    #[test]
    fn a_renamed_entity_is_matched_by_its_uuid_not_by_its_name() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.name = "api-v2".into();
        let theirs = base.clone();

        let plan = plan_kind(&[ours], &[theirs], base_of(base), &[], &[], Side::Local).unwrap();

        assert!(plan.incoming.is_empty(), "nothing to do here: {:#?}", plan.incoming);
        let sent: Project = only_outgoing(&plan);
        assert_eq!(sent.name, "api-v2", "the rename travels");
        assert_eq!(
            plan.outgoing[0].previous_name.as_deref(),
            Some("api"),
            "the peer has to unlink the old filename, so it has to be told the old name"
        );
    }

    /// The data-loss case, and the one both halves of the plan hide from each
    /// other.
    ///
    /// We renamed `api` to `web`; the peer independently has a *different*
    /// project already called `web`. Each change is unremarkable alone — a
    /// rename the peer needs, and a project we have never seen — and together
    /// they overwrite each other: our `web` is replaced by theirs, theirs by
    /// ours, one project is lost on each machine, and the two machines end up
    /// disagreeing about what `web` is.
    ///
    /// Neither change may be applied, and the clash has to be said out loud:
    /// only the user can decide which project keeps the name.
    #[test]
    fn a_rename_into_a_name_the_peer_gave_something_else_is_a_conflict() {
        let base = proj("api");
        // We renamed api -> web.
        let mut ours = base.clone();
        ours.name = "web".into();
        // The peer still knows that one as api, and has its own, different, web.
        let theirs_same = base.clone();
        let mut theirs_other = proj("other");
        theirs_other.name = "web".into();

        let plan = plan_kind(
            &[ours],
            &[theirs_same, theirs_other],
            base_of(base),
            &[],
            &[],
            Side::Local,
        )
        .unwrap();

        assert!(
            plan.incoming.is_empty() && plan.outgoing.is_empty(),
            "two entities cannot both be written to one name; neither change may be \
             applied: {:#?}",
            plan
        );
        assert_eq!(plan.conflicts.len(), 1, "the clash has to be reported: {:#?}", plan);
        let c = &plan.conflicts[0];
        assert_eq!(c.kind, "project");
        assert_eq!(c.name, "web");
        assert_eq!(c.field, "<name>");
        assert!(c.ours.contains("p-api"), "our entity has to be identifiable: {:?}", c.ours);
        assert!(
            c.theirs.contains("p-other"),
            "and so does theirs: {:?}",
            c.theirs
        );
        assert_eq!(
            plan.notes.len(),
            1,
            "a conflict nobody can act on is not enough; the remedy has to be stated: {:#?}",
            plan.notes
        );
        assert!(
            plan.notes[0].reason.to_lowercase().contains("rename one of them"),
            "the note must say what to do: {:?}",
            plan.notes[0].reason
        );
    }

    /// `store::lock_key` collapses everything outside `[A-Za-z0-9_-]`, so
    /// `"my api"` and `"my_api"` are two names sharing one lock file and one
    /// tombstone file. Detecting the clash on the raw name would let that pair
    /// through.
    #[test]
    fn two_names_that_share_a_lock_key_clash_as_surely_as_one_name() {
        // Guard: if these ever stop sharing a key the test is proving nothing.
        assert_eq!(crate::store::lock_key("my api"), crate::store::lock_key("my_api"));

        let mut ours = proj("api");
        ours.name = "my api".into();
        let mut theirs_other = proj("other");
        theirs_other.name = "my_api".into();

        let plan = plan_kind(
            &[ours],
            &[proj("api"), theirs_other],
            base_of(proj("api")),
            &[],
            &[],
            Side::Local,
        )
        .unwrap();

        assert!(
            plan.incoming.is_empty() && plan.outgoing.is_empty(),
            "two names sharing a lock key still race: {:#?}",
            plan
        );
        assert_eq!(plan.conflicts.len(), 1, "{:#?}", plan);
    }

    /// The benign half of the same shape, with a second peer entity in the plan
    /// so that "more than one change" is not what triggers the refusal.
    #[test]
    fn a_rename_into_a_free_name_still_travels() {
        let base = proj("api");
        let mut ours = base.clone();
        ours.name = "web".into();

        let plan = plan_kind(
            &[ours],
            &[base.clone(), proj("docs")],
            base_of(base),
            &[],
            &[],
            Side::Local,
        )
        .unwrap();

        assert!(plan.conflicts.is_empty(), "nothing collided: {:#?}", plan.conflicts);
        assert_eq!(plan.outgoing.len(), 1, "{:#?}", plan);
        assert_eq!(plan.outgoing[0].name, "web");
        assert_eq!(plan.outgoing[0].previous_name.as_deref(), Some("api"));
        assert_eq!(plan.incoming.len(), 1, "{:#?}", plan);
        assert_eq!(plan.incoming[0].name, "docs");
    }

    /// `lock_targets` has to dedupe on the lock key — `fs2` locks are not
    /// re-entrant and taking one twice in a thread hangs forever — so it cannot
    /// be the thing that *notices* two entities claiming one key. This is the
    /// signal it cannot carry, so that a hand-built plan reaching the applier
    /// is not relying on the dedupe to be safe.
    #[test]
    fn a_plan_can_be_asked_which_of_its_targets_two_entities_both_claim() {
        let one = PlannedChange {
            kind: "project".into(),
            name: "web".into(),
            id: Some("p-api".into()),
            previous_name: None,
            change: Change::Upsert { yaml: String::new() },
        };
        // The same entity, written on both machines: one lock, not a clash.
        let same_entity = PlannedChange { ..one.clone() };
        let other_entity = PlannedChange { id: Some("p-other".into()), ..one.clone() };

        let settled = MergePlan {
            incoming: vec![one.clone()],
            outgoing: vec![same_entity],
            ..Default::default()
        };
        assert!(
            settled.lock_target_collisions().is_empty(),
            "one entity written on both machines shares a name with itself: {:#?}",
            settled.lock_target_collisions()
        );

        let clashing =
            MergePlan { incoming: vec![one, other_entity], ..Default::default() };
        assert_eq!(
            clashing.lock_target_collisions(),
            vec![("project".to_string(), "web".to_string())],
        );
        assert_eq!(
            clashing.lock_targets(),
            vec![("project".to_string(), "web".to_string())],
            "the dedupe still has to happen, which is exactly why it cannot be the signal"
        );
    }

    /// A joining machine's uuids were minted independently and cannot match the
    /// Ranch House's, so name is the only thing that can align the two sets —
    /// once. Afterwards both sides carry the house's uuid and match on it.
    #[test]
    fn a_first_join_matches_by_name_and_adopts_the_ranch_house_uuid() {
        let mut ours = proj("api");
        ours.id = Some("minted-here".into());
        let mut theirs = proj("api");
        theirs.id = Some("minted-there".into());

        let plan = plan_kind(&[ours], &[theirs], no_base, &[], &[], Side::Remote).unwrap();
        let adopted: Project = only_incoming(&plan);
        assert_eq!(
            adopted.id.as_deref(),
            Some("minted-there"),
            "the Ranch House's uuid is the one that survives a first join"
        );
    }

    #[test]
    fn an_entity_only_one_side_has_travels_to_the_other() {
        let ours = proj("api");
        let theirs = proj("web");

        let plan = plan_kind(&[ours], &[theirs], no_base, &[], &[], Side::Local).unwrap();
        assert_eq!(plan.incoming.len(), 1);
        assert_eq!(plan.incoming[0].name, "web");
        assert_eq!(plan.outgoing.len(), 1);
        assert_eq!(plan.outgoing[0].name, "api");
    }

    /// Identity is not merged field-wise. `created_at` is a fact and the earlier
    /// record is the truer one; `updated_at` feeds tombstone comparison, where
    /// anything but the latest would let a stale timestamp authorize deleting
    /// freshly edited content. Both are **parsed**, so an offset cannot invert
    /// the comparison.
    #[test]
    fn identity_takes_the_earliest_creation_and_the_latest_edit() {
        let mut ours = proj("api");
        ours.created_at = Some("2026-01-05T00:00:00+00:00".into());
        // 14:00 UTC — later than the peer's 12:00Z despite the lower hour.
        ours.updated_at = Some("2026-02-01T09:00:00-05:00".into());

        let mut theirs = proj("api");
        theirs.created_at = Some("2026-01-01T00:00:00+00:00".into());
        theirs.updated_at = Some("2026-02-01T12:00:00Z".into());

        let (merged, _) = merge_entity(Some(&proj("api")), &ours, &theirs, Side::Local);
        assert_eq!(merged.created_at.as_deref(), Some("2026-01-01T00:00:00+00:00"));
        assert_eq!(merged.updated_at.as_deref(), Some("2026-02-01T09:00:00-05:00"));
    }

    // ==================================================================
    // C5 — tombstones
    // ==================================================================

    #[test]
    fn a_peer_tombstone_deletes_an_entity_nobody_has_touched_since() {
        let ours = proj("api"); // updated_at 2026-02-01
        let stone = tomb("project", "api", "p-api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[ours], &[], no_base, &[], &[stone], Side::Local).unwrap();
        assert_eq!(plan.incoming.len(), 1, "{:#?}", plan);
        assert_eq!(plan.incoming[0].change, Change::Delete);
        assert_eq!(plan.incoming[0].name, "api");
    }

    /// The whole point of the rule. A tombstone that predates the edit is
    /// stale, and applying it would throw away work with no trace.
    #[test]
    fn a_tombstone_does_not_delete_an_entity_edited_after_the_deletion() {
        let mut ours = proj("api");
        ours.updated_at = Some("2026-04-01T00:00:00+00:00".into());
        let stone = tomb("project", "api", "p-api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[ours], &[], no_base, &[], &[stone], Side::Local).unwrap();

        assert!(
            plan.incoming.iter().all(|c| c.change != Change::Delete),
            "a concurrent edit must not be silently discarded: {:#?}",
            plan
        );
        assert_eq!(plan.conflicts.len(), 1, "and the user has to be told: {:#?}", plan);
        assert_eq!(plan.conflicts[0].field, "<deleted>");
        assert_eq!(plan.outgoing.len(), 1, "the survivor goes back to the machine that deleted it");
    }

    /// RFC 3339 is not lexically ordered, and both ways it breaks are reachable
    /// with timestamps this codebase actually writes.
    ///
    /// - A **western offset** puts a later instant at a lower wall-clock hour:
    ///   `09:00-05:00` is 14:00 UTC, two hours *after* a `12:00Z` deletion, and
    ///   string-compares below it.
    /// - **`'Z'` sorts above every digit** (0x5A against 0x2E), so a
    ///   fractional-second stamp — which is exactly what
    ///   `Utc::now().to_rfc3339()` emits, since `now()` has nanoseconds —
    ///   string-compares below the whole second it comes after, when the
    ///   deletion was serialized through chrono's serde impl.
    ///
    /// Both cases are an entity edited *after* the deletion, so both must
    /// refuse to delete. String comparison deletes both.
    #[test]
    fn tombstone_times_are_parsed_not_string_compared() {
        let cases = [
            ("2026-03-01T09:00:00-05:00", "2026-03-01T12:00:00+00:00"),
            ("2026-03-01T12:00:00.500+00:00", "2026-03-01T12:00:00Z"),
        ];

        for (edited, deleted) in cases {
            // Guard: the string order really does invert the chronological one,
            // or this test is proving nothing.
            assert!(
                edited < deleted,
                "{:?} must string-compare below {:?} for this case to have teeth",
                edited,
                deleted
            );
            assert!(
                parse_time(edited).unwrap() > parse_time(deleted).unwrap(),
                "{:?} really is the later instant",
                edited
            );

            let mut ours = proj("api");
            ours.updated_at = Some(edited.into());
            let stone = tomb("project", "api", "p-api", deleted);

            let plan = plan_kind(&[ours], &[], no_base, &[], &[stone], Side::Local).unwrap();
            assert!(
                plan.incoming.iter().all(|c| c.change != Change::Delete),
                "{:?} was edited after the {:?} deletion, so the deletion is stale: {:#?}",
                edited,
                deleted,
                plan
            );
        }
    }

    /// A known limitation of Phase 1's fallback key, pinned rather than
    /// pretended away: `tombstones::record` writes `{kind}--{name}` when the
    /// deleted entity had never been stamped, and no uuid can equal that. Such
    /// a deletion cannot travel. Name-matching it instead would let a deletion
    /// on one machine kill an unrelated entity that merely shares a name.
    #[test]
    fn a_fallback_keyed_tombstone_cannot_propagate_and_says_so() {
        let ours = proj("api");
        let stone = tomb("project", "api", "project--api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[ours], &[], no_base, &[], &[stone], Side::Local).unwrap();

        assert!(
            plan.incoming.iter().all(|c| c.change != Change::Delete),
            "a fallback key cannot match a uuid: {:#?}",
            plan
        );
        assert_eq!(plan.notes.len(), 1, "the limitation has to be visible: {:#?}", plan.notes);
        assert!(
            plan.notes[0].reason.contains("cannot propagate"),
            "{:?}",
            plan.notes[0].reason
        );
        assert!(
            plan.notes[0].reason.contains("the peer deleted"),
            "the note has to say which machine threw it away: {:?}",
            plan.notes[0].reason
        );
    }

    /// The mirror, which emitted nothing at all.
    ///
    /// *We* deleted an entity that had never been stamped, so our stone carries
    /// the `{kind}--{name}` fallback key and cannot reach the peer's copy. That
    /// copy therefore arrives as an ordinary incoming upsert — resurrecting what
    /// the user just deleted — and without a note the plan pane offers no reason
    /// for it at all.
    #[test]
    fn our_own_fallback_keyed_tombstone_explains_the_upsert_that_undoes_it() {
        let theirs = proj("api");
        let stone = tomb("project", "api", "project--api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[], &[theirs], no_base, &[stone], &[], Side::Local).unwrap();

        assert_eq!(
            plan.incoming.len(),
            1,
            "the peer's copy still arrives, which is the whole problem: {:#?}",
            plan
        );
        assert_eq!(plan.incoming[0].name, "api");
        assert_eq!(plan.notes.len(), 1, "and it needs a reason: {:#?}", plan.notes);
        assert!(
            plan.notes[0].reason.contains("cannot propagate"),
            "{:?}",
            plan.notes[0].reason
        );
        assert!(
            plan.notes[0].reason.contains("this machine deleted"),
            "the note has to say which machine threw it away: {:?}",
            plan.notes[0].reason
        );
    }

    /// A fallback-keyed stone names a *name*, not an entity, so it goes on
    /// matching that name for as long as it lives. An entity alive on both
    /// machines is not being kept against anybody's wishes — the peer deleted an
    /// older, unstamped thing of that name and made a new one — and telling the
    /// user a deletion could not propagate sends them looking for a problem that
    /// is not there.
    #[test]
    fn a_stone_for_a_name_alive_on_both_machines_is_not_worth_mentioning() {
        let stone = tomb("project", "api", "project--api", "2026-03-01T00:00:00+00:00");

        let plan =
            plan_kind(&[proj("api")], &[proj("api")], no_base, &[], &[stone], Side::Local)
                .unwrap();

        assert!(plan.is_empty(), "nothing to do: {:#?}", plan);
        assert!(
            plan.notes.is_empty(),
            "the entity both machines still hold is a delete-then-recreate, not a deletion \
             that failed to travel: {:#?}",
            plan.notes
        );
    }

    #[test]
    fn our_tombstone_deletes_the_peer_s_copy() {
        let theirs = proj("api");
        let stone = tomb("project", "api", "p-api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[], &[theirs], no_base, &[stone], &[], Side::Local).unwrap();
        assert_eq!(plan.outgoing.len(), 1, "{:#?}", plan);
        assert_eq!(plan.outgoing[0].change, Change::Delete);
        assert!(plan.incoming.is_empty(), "we must not re-create what we deleted: {:#?}", plan);
    }

    #[test]
    fn a_tombstone_for_another_kind_is_not_ours_to_act_on() {
        let ours = proj("api");
        let stone = tomb("worm", "api", "p-api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[ours], &[], no_base, &[], &[stone], Side::Local).unwrap();
        assert!(plan.incoming.iter().all(|c| c.change != Change::Delete), "{:#?}", plan);
    }

    /// An entity deleted, recreated and deleted again has several stones, and
    /// only the last one describes the ranch. Directory order is not date
    /// order, so both arrival orders are exercised — "whichever came first"
    /// happens to be right half the time, which is not a rule.
    #[test]
    fn the_newest_tombstone_for_an_id_is_the_one_that_governs() {
        let newest = tomb("project", "api", "p-api", "2026-05-01T00:00:00+00:00");
        let oldest = tomb("project", "api", "p-api", "2026-03-01T00:00:00+00:00");

        for stones in [vec![newest.clone(), oldest.clone()], vec![oldest, newest]] {
            let mut ours = proj("api");
            // Edited in April: after the March stone, before the May one.
            ours.updated_at = Some("2026-04-01T00:00:00+00:00".into());

            let plan = plan_kind(&[ours], &[], no_base, &[], &stones, Side::Local).unwrap();
            assert_eq!(
                plan.incoming.first().map(|c| &c.change),
                Some(&Change::Delete),
                "the May deletion post-dates the April edit, whichever order the stones \
                 arrived in: {:#?}",
                plan
            );
        }
    }

    /// An `updated_at` that is absent or unreadable is not evidence the entity
    /// is older than the deletion, and the conservative direction is the one
    /// that does not lose work.
    #[test]
    fn an_entity_with_no_timestamp_is_not_deleted_on_a_guess() {
        let mut ours = proj("api");
        ours.updated_at = None;
        let stone = tomb("project", "api", "p-api", "2026-03-01T00:00:00+00:00");

        let plan = plan_kind(&[ours], &[], no_base, &[], &[stone], Side::Local).unwrap();
        assert!(plan.incoming.iter().all(|c| c.change != Change::Delete), "{:#?}", plan);
        assert_eq!(plan.conflicts.len(), 1, "and it is reported: {:#?}", plan);
    }

    // ==================================================================
    // C6 — properties
    // ==================================================================

    /// A tiny xorshift, so the generators below explore rather than replay.
    ///
    /// Hand-rolled because this crate has no property-testing dependency and
    /// adding one for four tests is not worth the supply chain. The shape it
    /// gives up is shrinking: a failure reports the seed, and the seed
    /// reproduces the case exactly, which is what a debugging session needs.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            // Any non-zero state; xorshift is dead at zero.
            Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn flip(&mut self) -> bool {
            self.next() & 1 == 1
        }
    }

    const BARNS: [&str; 4] = ["imac", "pi", "mbp", "vps"];
    const STOCK: [&str; 4] = ["web", "worker", "api", "cron"];
    const TITLES: [&str; 4] = ["Architecture", "Deploy", "Gotchas", "Commands"];
    const WORDS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

    /// A project drawn from a space wide enough for the properties to mean
    /// something.
    ///
    /// Counting the branches: `summary` and `color` are 5 values each,
    /// `gradientSpread` 4, `gradientInverted` 3, the livestock set is any of
    /// 2^16 subsets of `BARNS × STOCK` with 4 branch values apiece, the wiki is
    /// any of 2^4 subsets of `TITLES` with 4 contents each, and herds are 2^4
    /// subsets with 2^4 member sets. That is well past 10^20 distinct projects
    /// before the two independent mutations that follow.
    fn random_project(rng: &mut Rng) -> Project {
        let mut p = proj("api");
        p.summary = pick_opt(rng, &WORDS);
        p.color = pick_opt(rng, &WORDS);
        p.gradient_spread = if rng.flip() { Some(rng.below(4) as f64) } else { None };
        p.gradient_inverted = match rng.below(3) {
            0 => None,
            1 => Some(true),
            _ => Some(false),
        };

        for b in BARNS {
            for s in STOCK {
                if rng.flip() {
                    let mut l = ls(b, s);
                    l.branch = Some(WORDS[rng.below(WORDS.len())].to_string());
                    p.livestock.push(l);
                }
            }
        }
        for t in TITLES {
            if rng.flip() {
                p.wiki.push(wiki(t, WORDS[rng.below(WORDS.len())]));
            }
        }
        for w in WORDS {
            if rng.flip() {
                p.herds.push(Herd {
                    name: w.to_string(),
                    livestock: STOCK.iter().filter(|_| rng.flip()).map(|s| s.to_string()).collect(),
                    critters: vec![],
                    connections: vec![],
                });
            }
        }
        p
    }

    fn pick_opt(rng: &mut Rng, from: &[&str]) -> Option<String> {
        let i = rng.below(from.len() + 1);
        from.get(i).map(|s| s.to_string())
    }

    /// A few independent edits, the way one machine's week looks.
    fn mutate(rng: &mut Rng, p: &Project) -> Project {
        let mut p = p.clone();
        for _ in 0..rng.below(5) {
            match rng.below(6) {
                0 => p.summary = pick_opt(rng, &WORDS),
                1 => p.color = pick_opt(rng, &WORDS),
                2 => {
                    let mut l = ls(BARNS[rng.below(4)], STOCK[rng.below(4)]);
                    l.branch = Some(WORDS[rng.below(WORDS.len())].to_string());
                    let key = livestock_key(&l);
                    p.livestock.retain(|x| livestock_key(x) != key);
                    p.livestock.push(l);
                }
                3 => {
                    if !p.livestock.is_empty() {
                        let i = rng.below(p.livestock.len());
                        p.livestock.remove(i);
                    }
                }
                4 => {
                    let t = TITLES[rng.below(TITLES.len())];
                    let c = WORDS[rng.below(WORDS.len())].to_string();
                    match p.wiki.iter_mut().find(|s| s.title == t) {
                        Some(s) => s.content = c,
                        None => p.wiki.push(wiki(t, &c)),
                    }
                }
                _ => {
                    let n = WORDS[rng.below(WORDS.len())].to_string();
                    let member = STOCK[rng.below(STOCK.len())].to_string();
                    match p.herds.iter_mut().find(|h| h.name == n) {
                        Some(h) => h.livestock.push(member),
                        None => p.herds.push(Herd {
                            name: n,
                            livestock: vec![member],
                            critters: vec![],
                            connections: vec![],
                        }),
                    }
                }
            }
        }
        p
    }

    /// The same project with every unioned collection in a different order.
    ///
    /// Collection order is an *input* to the merge, not something it invents:
    /// `union_with` inherits the order from the base, the house and the peer in
    /// that sequence. The generators above build their collections in a fixed
    /// nested-loop order, so the properties would otherwise only ever see one
    /// arrangement of them — and order is precisely the dimension where an
    /// inherited answer is most likely to have been taken from the argument slot
    /// instead of from the inputs.
    ///
    /// Both machines hold the same three files, so shuffling once and handing the
    /// result to *both* merges is what a real sync does. Shuffling one side only
    /// would change the inputs between the two calls and prove nothing.
    fn shuffled(rng: &mut Rng, p: &Project) -> Project {
        let mut p = p.clone();
        let swap = |len: usize, rng: &mut Rng| -> Vec<(usize, usize)> {
            (0..len).map(|i| (i, rng.below(len.max(1)))).collect()
        };
        for (i, j) in swap(p.livestock.len(), rng) {
            p.livestock.swap(i, j);
        }
        for (i, j) in swap(p.wiki.len(), rng) {
            p.wiki.swap(i, j);
        }
        for (i, j) in swap(p.herds.len(), rng) {
            p.herds.swap(i, j);
        }
        for h in &mut p.herds {
            let len = h.livestock.len();
            for (i, j) in swap(len, rng) {
                h.livestock.swap(i, j);
            }
        }
        p
    }

    const ROUNDS: u64 = 400;

    /// **This is the convergence guarantee.** Both machines merge the same three
    /// inputs and name the same machine as the Ranch House, so both must compute
    /// the same bytes — every tie-break and every inherited order has to be a
    /// function of the inputs and of *which side is the house*, never of which
    /// argument slot an entity arrived in. If any of them followed the slot, the
    /// two machines would each see the other as changed, ship, re-merge, and the
    /// sync would never settle.
    #[test]
    fn merge_is_commutative_given_the_same_base() {
        // Teeth guard for the shuffling below: if it never actually reorders
        // anything then the order coverage it is there to provide is imaginary.
        let mut shuffle_reordered_something = false;

        for seed in 0..ROUNDS {
            let rng = &mut Rng::new(seed);

            let generated = random_project(rng);
            let base = shuffled(rng, &generated);
            shuffle_reordered_something |= render(&base) != render(&generated);

            let our_week = mutate(rng, &base);
            let mut ours = shuffled(rng, &our_week);
            let their_week = mutate(rng, &base);
            let mut theirs = shuffled(rng, &their_week);

            // Identity has to differ, or the assertions on it below have no
            // teeth. On a first join the two machines minted their uuids
            // independently and stamped at different moments, which is the whole
            // case `settle_identity` exists for.
            ours.id = Some("minted-here".into());
            theirs.id = Some("minted-there".into());
            ours.created_at = Some("2026-01-05T00:00:00+00:00".into());
            theirs.created_at = Some("2026-01-01T00:00:00+00:00".into());
            // 14:00 UTC, so the later instant is also the lower wall-clock hour.
            ours.updated_at = Some("2026-02-01T09:00:00-05:00".into());
            theirs.updated_at = Some("2026-02-01T12:00:00Z".into());

            // Both calls name the *same* machine as the Ranch House.
            let (left, lc) = merge_entity(Some(&base), &ours, &theirs, Side::Local);
            let (right, rc) = merge_entity(Some(&base), &theirs, &ours, Side::Remote);

            assert_eq!(
                hash_entity("project", &left).unwrap(),
                hash_entity("project", &right).unwrap(),
                "seed {} — which side ran the merge changed the answer\nleft: {:#?}\nright: {:#?}",
                seed,
                left,
                right
            );
            assert_eq!(lc.len(), rc.len(), "seed {} — conflicts disagreed", seed);

            // The hash above strips all three of these, so it is blind to exactly
            // the thing `settle_identity` decides — and the uuid is the one field
            // whose whole job is to follow the house rather than the argument
            // slot. Take it from the slot instead and the joining machine keeps
            // its own uuid and pushes it at the house, each side seeing the other
            // as the one that has to change.
            assert_eq!(left.id, right.id, "seed {} — the uuid followed the argument slot", seed);
            assert_eq!(
                left.created_at, right.created_at,
                "seed {} — created_at followed the argument slot",
                seed
            );
            assert_eq!(
                left.updated_at, right.updated_at,
                "seed {} — updated_at followed the argument slot",
                seed
            );
        }

        assert!(
            shuffle_reordered_something,
            "the shuffle never reordered a collection, so this ran {} rounds of one arrangement \
             and the order coverage is imaginary",
            ROUNDS
        );
    }

    /// Once both machines hold the merged state, a re-sync must have nothing to
    /// do. Anything else is a sync that ships the ranch every time.
    #[test]
    fn merge_is_idempotent_on_re_sync() {
        for seed in 0..ROUNDS {
            let rng = &mut Rng::new(seed ^ 0xA5A5);
            let base = random_project(rng);
            let ours = mutate(rng, &base);
            let theirs = mutate(rng, &base);

            let (merged, _) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

            let plan = plan_kind(
                &[merged.clone()],
                &[merged.clone()],
                base_of(merged.clone()),
                &[],
                &[],
                Side::Local,
            )
            .unwrap();
            assert!(plan.is_empty(), "seed {} — a settled ranch re-synced: {:#?}", seed, plan);

            let (again, conflicts) = merge_entity(Some(&merged), &merged, &merged, Side::Local);
            assert_eq!(
                hash_entity("project", &again).unwrap(),
                hash_entity("project", &merged).unwrap(),
                "seed {} — merging a merged entity moved it",
                seed
            );
            assert!(conflicts.is_empty(), "seed {} — {:#?}", seed, conflicts);
        }
    }

    /// A livestock is a codebase on a specific machine. Losing one is losing
    /// the record of something that is actually running.
    #[test]
    fn merge_never_loses_a_livestock() {
        for seed in 0..ROUNDS {
            let rng = &mut Rng::new(seed ^ 0x5A5A);
            let base = random_project(rng);
            let ours = mutate(rng, &base);
            let theirs = mutate(rng, &base);

            let (merged, _) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

            let kept: HashSet<(String, String)> = ls_keys(&merged).into_iter().collect();
            for key in ls_keys(&ours).into_iter().chain(ls_keys(&theirs)) {
                assert!(
                    kept.contains(&key),
                    "seed {} — livestock {:?} was dropped by the merge",
                    seed,
                    key
                );
            }
        }
    }

    /// Under inherited ordering the *union invariant* is what a property test
    /// can still assert about order: every key either side holds comes out
    /// exactly once, and nothing else does.
    ///
    /// The convergence half of the old shuffle-everything property moved to
    /// `merge_is_commutative_given_the_same_base`, which is where it belongs:
    /// convergence means two machines computing the same answer, and inherited
    /// order is a function of the inputs plus which side is the house — all four
    /// of which both machines hold identically. It is deliberately *not* a
    /// function of the argument slots, and that is exactly what commutativity
    /// pins.
    #[test]
    fn a_union_emits_every_key_exactly_once() {
        for seed in 0..ROUNDS {
            let rng = &mut Rng::new(seed ^ 0x1234);
            let base = random_project(rng);
            let ours = mutate(rng, &base);
            let theirs = mutate(rng, &base);

            let (merged, _) = merge_entity(Some(&base), &ours, &theirs, Side::Local);

            let mut expected: Vec<(String, String)> =
                ls_keys(&ours).into_iter().chain(ls_keys(&theirs)).collect();
            expected.sort();
            expected.dedup();

            let mut got = ls_keys(&merged);
            assert_eq!(
                got.len(),
                {
                    let mut d = got.clone();
                    d.sort();
                    d.dedup();
                    d.len()
                },
                "seed {} — a key came out twice: {:?}",
                seed,
                got
            );
            got.sort();
            assert_eq!(got, expected, "seed {} — the key set changed", seed);

            let mut wiki: Vec<&String> = merged.wiki.iter().map(|s| &s.title).collect();
            let before = wiki.len();
            wiki.sort();
            wiki.dedup();
            assert_eq!(before, wiki.len(), "seed {} — a wiki title came out twice", seed);

            let mut herds: Vec<&String> = merged.herds.iter().map(|h| &h.name).collect();
            let before = herds.len();
            herds.sort();
            herds.dedup();
            assert_eq!(before, herds.len(), "seed {} — a herd came out twice", seed);
        }
    }
}
