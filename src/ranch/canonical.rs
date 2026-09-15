//! Canonical form and content hashing.
//!
//! A sync has to answer one question about every entity: *is this the same
//! entity the peer already has?* Byte comparison cannot answer it, and neither
//! can a hash of the file's bytes:
//!
//! - Almost nothing carries `skip_serializing_if` — the three `Identified`
//!   fields on every kind, and the six ranch fields Slice D added to `Barn`.
//!   Every *other* `Option` on `Barn`, `Project`, `Livestock` and `Critter`
//!   serializes as an explicit `null`, so a hand-written four-line barn file
//!   comes back from a round trip as fourteen lines with identical meaning.
//! - `Trail.env`, `TrailJob.env` and `TrailStep.env` are `HashMap`s. `std`'s
//!   `RandomState` reseeds per map instance, so two
//!   round trips of an unchanged trail iterate their keys in different orders
//!   and differ byte-for-byte. (Measured: two twelve-key maps built back to
//!   back agree on an order roughly never.)
//!
//! So the hash is taken over a *canonical form* instead: serialize to a
//! `serde_yaml::Value`, walk it, sort every mapping by key, drop mapping
//! entries whose value is null, drop the three identity keys at the top level,
//! render that deterministically, and SHA-256 the result.
//!
//! ## Why identity is excluded
//!
//! `Identified::stamp()` runs on *every* save and advances `updated_at`. If
//! that fed the hash, an entity re-saved with no edits would hash differently
//! from the copy the peer holds, every save would look like a change, and every
//! sync would ship the entire ranch. `id` and `created_at` come out for the
//! same reason from the other direction: on a first join the two machines minted
//! their uuids independently, so identity is exactly what has *not* got to
//! match for two records to be the same thing.
//!
//! ## Why nulls are dropped
//!
//! Two entities that parsed from different files but landed on the same struct
//! already serialize identically, so this is not about today's round trip. It is
//! about *tomorrow's field*: a peer on an older build renders a barn with no
//! `connectable` key at all, while this build renders `connectable: null`.
//! Dropping nulls keeps the two hashes equal, so a version skew does not present
//! as "every barn changed".
//!
//! Slice D's six fields took the belt-and-braces route instead and carry
//! `skip_serializing_if`, so this build renders no key either. That is not
//! redundant with the rule here, and one of the six proves it: an empty
//! `Vec` is not a null, so `addresses: []` survives canonicalization and would
//! have hashed differently from an older peer's absent key. Dropping nulls does
//! not cover empty collections — `tests::an_empty_address_list_hashes_the_same_as_a_peer_that_has_no_such_field`
//! is the measurement, and `Vec::is_empty` on the field is the fix. Teaching the
//! canonicalizer to drop empty sequences instead would change every existing
//! hash and would have to answer for `critters: []`, `livestock: []` and
//! `wiki: []` too.
//!
//! ## Identity is a special case of "machine-local bookkeeping is not content"
//!
//! The argument above is not really about identity. It is about *fields whose
//! value is a verdict about the machine holding the file*. Such a field differs
//! between two machines that hold the very same entity, and the sync has no way
//! to reconcile it: it is not a change either side made, so neither side may
//! win, and the merge's "changed on both → conflict" rule fires on it forever.
//!
//! Seven such fields exist beyond the identity three, and they are not the same
//! for every kind. Four predate Slice D:
//!
//! - **`Project.path`** — where the project's checkout lives on *this* machine.
//!   The design's Decisions section already rejects "syncing filesystem paths
//!   verbatim, which breaks the moment a Linux Pi joins", and the Layer 3 merge
//!   table lists no `path` among the project scalars. The Ranch House's
//!   `/Users/cam/Sites/api` and a Pi's `/home/cam/api` are two machines' answers
//!   to a local question, not a disagreement about the project: hashed, every
//!   project reads as changed on both sides on every sync, and the side that
//!   loses can no longer `expand_path(&project.path)` into anything that exists —
//!   `app.rs:815` and `app.rs:1011` both stop being able to open a session for
//!   it.
//! - **`Barn.connectable`** — a per-machine reachability verdict, written by
//!   discovery (`ranchhand_k8s`, `ranchhand_terraform`) and by
//!   `migrate::adopt_this_machine`. It is not decorative: `connect.rs` and
//!   `app.rs` both *hard refuse* a barn marked `Some(false)`. If it fed the
//!   hash, a Ranch House that cannot reach a barn would push `Some(false)` over
//!   a peer's `Some(true)` and the user would lose the ability to connect to a
//!   host they can demonstrably reach.
//! - **`RanchHand.last_sync`** — `Utc::now()`, written by
//!   `config::update_ranchhand_last_sync` on every ranch-hand sync. It differs
//!   on both sides essentially always, so if it fed the hash every ranch hand
//!   would be "changed on both" and conflict on every sync, forever.
//! - **`RanchHand.config`** — untyped YAML holding a `kubeconfig_path` or an S3
//!   reference. The merge design already calls it machine-local and gives the
//!   local value an unconditional win. A field the merge refuses to take from a
//!   peer must not be allowed to *say* the entity changed either: it would make
//!   every differently-configured ranch hand a permanent conflict, and the
//!   "remote changed, we did not" branch would clobber the local kubeconfig
//!   path with the peer's.
//!
//! Slice D added three more, all on `barn`, and all argued at the point of
//! classification rather than here — see the comments in [`SHAPES`]:
//!
//! - **`Barn.last_seen`** — `RanchHand.last_sync` under another name. Written on
//!   every reachability check, so two machines holding the same barn never agree.
//! - **`Barn.synced`** — whether *this* machine syncs the barn. Enrolment is a
//!   relationship, not a property of the barn, and a peer's `Some(false)` landing
//!   here would switch off a sync this machine is actively running.
//! - **`Barn.tunnel_port`** — a port forwarded on *this* machine. What is free on
//!   the iMac is taken on the Pi.
//!
//! The other three ranch fields — `brand`, `is_ranch_house`, `addresses` — are
//! content, and have to be: each exists in order to reach the other machine.
//!
//! So the not-content list is **per kind**, not flat: identity applies to all
//! five, `path` only to `project`, `connectable`/`last_seen`/`synced`/
//! `tunnel_port` only to `barn`, `last_sync` and `config` only to `ranchhand`.
//! See [`SHAPES`].
//!
//! ## Adding a field is a decision, not a default
//!
//! [`SHAPES`] lists *every* top-level key of every kind, split into content and
//! machine-local — not just the ones that get stripped. That redundancy is the
//! point: `every_top_level_field_is_classified` serializes one instance of each
//! kind and fails when a key appears in neither list, so a new field cannot join
//! the hash by default. It has to be classified on the way in.
//!
//! ### What Slice D found out about that guard
//!
//! The prediction here was that Slice D's six fields would trip the test. They
//! did not — **one of six did**, and only because it had not yet been given its
//! `skip_serializing_if`. The hole documented right below this paragraph was the
//! reason, and the two halves of this note had never been read against each
//! other: a field carrying `skip_serializing_if` renders no key, so a minimally
//! populated instance makes it invisible to *both* directions of the check.
//! Measured: with all six attributes in place and none of the six classified,
//! the whole suite was green, `last_seen` included.
//!
//! So `barn`'s keys now come from an exhaustively populated struct literal
//! (`tests::every_barn_field_set`) rather than from a terse YAML fixture. The
//! literal is the tripwire: a new field on `Barn` fails to compile there, and
//! the only way to satisfy it is to give the field a value — at which point the
//! classification check sees it. The verdicts the six reached, argued in
//! [`SHAPES`] rather than here, were the ones this note predicted.
//!
//! The hole that remains: the other four kinds are still read from YAML
//! fixtures, which is sound only because the identity three are still the only
//! fields they skip, and those are classified by name. Give any of them a
//! `skip_serializing_if` field and it needs the same treatment.
//!
//! ## The `source` audit
//!
//! `Barn`, `Livestock` and `Critter` each carry a `source: Option<String>`, and
//! all three are content. Read, not assumed:
//!
//! - `Livestock.source` and `Critter.source` are only ever `ranchhand:<name>`,
//!   written by `ranchhand_k8s` and `ranchhand_terraform` discovery. The tag
//!   names a ranch hand, which is itself one of the five synced kinds, so it
//!   resolves to the same entity on both machines: provenance, and shared.
//!   Neither is a top-level key in any case — both ride inside their parent and
//!   are hashed as ordinary nested content.
//! - `Barn.source` takes the same `ranchhand:<name>` values plus one other:
//!   `migrate::adopt_this_machine` writes `Some("self")` on the barn that *is*
//!   this machine. That single value is a per-machine assertion, and it is the
//!   reason this one was close. It is also read nowhere — the self-barn question
//!   is answered by `config.this_barn` through `config::barn_is_this_machine`,
//!   by name — so it is inert today, while excluding the whole field to
//!   neutralize it would also stop the ranch-hand provenance tag propagating,
//!   which is genuinely shared. Verdict: content. Slice D's `is_ranch_house` and
//!   `brand` are what should carry the meaning `"self"` is gesturing at, and
//!   `"self"` should stop being written once they land.
//!
//! ## Order is content here, so a unioned collection owes us an order
//!
//! Sequences keep their order (see [`render`]): `TrailStep`s in a job and
//! `WikiSection`s in a wiki are order-significant, and sorting them would make
//! two genuinely different trails hash alike.
//!
//! That is correct, and it puts an obligation on the merge. Slice C *unions*
//! `Project.livestock`, `Project.herds`, `Project.wiki`, `Barn.critters` and the
//! `Herd` member lists. Union is a set operation and has no inherent order, so
//! if A's union yields `[x, y]` and B's yields `[y, x]`, the two sides agree
//! under the merge's key model and disagree under this hash — each sees the
//! other as changed, on every sync, forever.
//!
//! **Any collection the merge unions must be emitted by the merge in a
//! deterministic order** (sort by the same key the union deduplicates on).
//! Do not fix this by blanket-sorting sequences here: that would break trail
//! steps, where order is the program.

#![allow(dead_code)] // Wired up in Slice C/E.

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_yaml::Value;
use sha2::{Digest, Sha256};

/// The `Identified` fields, removed from the top level of every entity before
/// hashing whatever its kind. See the module docs: identity is not content.
const IDENTITY_KEYS: [&str; 3] = ["id", "created_at", "updated_at"];

/// Which of one kind's top-level keys are content and which are bookkeeping.
///
/// Both lists are exhaustive on purpose. `machine_local` is what the hash
/// strips; `content` earns its keep by making an unclassified field a test
/// failure rather than a silent addition to the hash. See the module docs.
pub struct KindShape {
    pub kind: &'static str,
    /// Stripped before hashing: a verdict about this machine, not about the
    /// entity. The identity three are stripped from every kind and are not
    /// repeated here.
    pub machine_local: &'static [&'static str],
    /// Hashed. Every remaining top-level key, listed so that a new one is
    /// conspicuous by its absence from both lists.
    pub content: &'static [&'static str],
}

/// The five kinds a sync moves, and what is content for each.
///
/// Keys are the *serialized* spellings, so the `#[serde(rename)]` fields appear
/// as `gradientSpread`, `issueProvider`, `type` and so on — a Rust field name
/// here would classify nothing.
pub const SHAPES: [KindShape; 5] = [
    KindShape {
        kind: "barn",
        machine_local: &[
            "connectable",
            // `Utc::now()` on every reachability check. This is
            // `RanchHand.last_sync` with a different name: two machines probe at
            // different moments, so they essentially never agree, and hashing it
            // makes every barn "changed on both" and therefore a conflict, on
            // every sync, forever. It is also a verdict about *this* machine's
            // reach, like `connectable` — "when did I last see it", not "when was
            // it last seen".
            "last_seen",
            // Whether *this* machine syncs this barn. The barn is not
            // intrinsically synced or not; enrolment is a relationship, and the
            // Ranch House having enrolled a barn says nothing about whether a
            // laptop has. Hashed, the two sides disagree and conflict forever on
            // a field neither is wrong about — and the merge's "they changed it,
            // we did not" branch would then push `Some(false)` onto a machine
            // that is actively syncing, switching its sync off. Same shape of
            // loss as `connectable`.
            "synced",
            // A local forwarded port. Whatever is free on the iMac is taken on
            // the Pi, so the value differs per machine by construction, and
            // adopting a peer's could collide with something already bound here.
            // The barn's own advertised reachability lives in `addresses`, which
            // is content — this is only how *this* machine tunnels to it.
            //
            // Re-open this if Slice E turns out to mean "the port the barn
            // listens on for a reverse tunnel" rather than "the local end of the
            // forward": that reading would make it a property of the barn.
            "tunnel_port",
        ],
        content: &[
            "name",
            "host",
            "user",
            "port",
            "identity_file",
            "critters",
            // Provenance, and shared. See "The `source` audit" in the
            // module docs — this is the one field on `Barn` where the verdict
            // was close.
            "source",
            "connection_type",
            "connection_config",
            // The barn's ed25519 *public* half, and the private half is not on
            // this struct at all. Its entire purpose is to circulate: the Ranch
            // House writes it into every other barn's `authorized_keys`. Strip it
            // from the hash and it never propagates, and Task D3 has nothing to
            // write. Identical on every machine that holds the record, so it
            // cannot conflict.
            "brand",
            // There is exactly one Ranch House per ranch and every machine has to
            // agree which barn it is — the merge's own tie-break is "the house
            // wins", so a per-machine answer would mean two machines disagreeing
            // about who arbitrates. It also has to travel: a third machine
            // joining via a peer learns who the house is only if this propagates.
            "is_ranch_house",
            // Where the barn can be reached. The closest call of the six, because
            // `Project.path` is machine-local for a superficially similar reason
            // — but a path is one machine's filesystem layout, whereas an address
            // is something the *barn* has. Which address works is per-machine;
            // the list is not.
            //
            // The risk runs the other way too and is bounded: an address one
            // machine can use and another cannot costs a connect timeout, not a
            // lost host, because this is a union rather than a scalar. Contrast
            // `connectable`, where a wrong value is a hard refusal — that
            // asymmetry is the whole reason the verdicts differ.
            //
            // Being unioned, it owes this hash a deterministic order; see the
            // module docs' last section. `merge::merge_addresses` pays that debt.
            "addresses",
        ],
    },
    KindShape {
        kind: "project",
        // Where the checkout lives on *this* machine. The design rejected
        // "syncing filesystem paths verbatim, which breaks the moment a Linux Pi
        // joins", and the Layer 3 table lists no `path` among the project
        // scalars — so the Ranch House's `/Users/cam/Sites/api` and a Pi's
        // `/home/cam/api` are two machines' answers to a local question, not a
        // disagreement about the project. Hashed, every project would read as
        // changed on both sides on every sync, and the side that lost could no
        // longer `expand_path(&project.path)` into anything that exists.
        machine_local: &["path"],
        content: &[
            "name",
            "summary",
            "color",
            "gradientSpread",
            "gradientInverted",
            "livestock",
            "herds",
            "wiki",
            "issueProvider",
            "wikiProvider",
        ],
    },
    KindShape {
        kind: "ranchhand",
        machine_local: &["config", "last_sync"],
        content: &["name", "project", "type", "sync_settings", "herd", "resource_mappings"],
    },
    KindShape {
        kind: "trail",
        machine_local: &[],
        content: &["name", "on", "env", "jobs"],
    },
    KindShape {
        kind: "worm",
        machine_local: &[],
        content: &["name", "command", "schedule", "type", "enabled", "project", "working_dir"],
    },
];

/// The shape registered for `kind`, or an error naming the kind.
fn shape_for(kind: &str) -> Result<&'static KindShape> {
    SHAPES.iter().find(|shape| shape.kind == kind).ok_or_else(|| {
        anyhow!(
            "no canonical shape is registered for entity kind {:?}, so there is no way to \
             tell its content from its machine-local bookkeeping; add it to \
             `canonical::SHAPES`",
            kind
        )
    })
}

/// The content hash of `entity` — lowercase hex SHA-256 of its canonical form.
///
/// Equal hashes mean "semantically the same entity", not "byte-identical
/// files". That is the only comparison a sync can safely make.
///
/// `kind` is one of the five in [`SHAPES`] and is what selects the not-content
/// list; an unregistered kind is an error rather than a guess, because guessing
/// means silently hashing a machine-local field. Every caller has a fixed kind
/// at the call site.
pub fn hash_entity<T: Serialize>(kind: &str, entity: &T) -> Result<String> {
    let form = canonical_form(kind, entity)?;
    let mut hasher = Sha256::new();
    hasher.update(form.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

/// The bytes [`hash_entity`] actually hashes.
///
/// Kept separate because a hash that disagrees between two machines is
/// otherwise undebuggable: with this, the two forms can be diffed directly and
/// the offending field named.
fn canonical_form<T: Serialize>(kind: &str, entity: &T) -> Result<String> {
    let shape = shape_for(kind)?;
    let value = serde_yaml::to_value(entity)
        .context("failed to serialize entity into a canonical form")?;
    let mut out = String::new();
    render_root(&value, shape, &mut out);
    Ok(out)
}

/// The top level is the only place not-content keys are stripped.
///
/// Deliberately not recursive: `RanchHand.config` is an untyped
/// `serde_yaml::Value` holding whatever kubeconfig or Terraform state the user
/// pointed at, and a k8s resource with its own `id` field is ordinary content.
/// The `Identified` fields and the per-kind machine-local fields exist exactly
/// once per entity, at the top.
fn render_root(value: &Value, shape: &KindShape, out: &mut String) {
    match value {
        Value::Mapping(map) => render_mapping(map, out, Some(shape)),
        other => render(other, out),
    }
}

/// True when `key` is bookkeeping rather than content for this kind.
fn is_not_content(shape: &KindShape, key: &str) -> bool {
    IDENTITY_KEYS.contains(&key) || shape.machine_local.contains(&key)
}

fn render(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push('~'),
        Value::Bool(b) => {
            out.push_str("b:");
            out.push(if *b { '1' } else { '0' });
        }
        // Numbers are rendered through their own `Display`, which distinguishes
        // `1` from `1.0` — `Project.gradient_spread` is an `f64` and an integer
        // literal in the file parses as one, so the two forms have to stay
        // distinguishable rather than be normalized together.
        Value::Number(n) => {
            out.push_str("n:");
            push_sized(out, &n.to_string());
        }
        Value::String(s) => {
            out.push_str("s:");
            push_sized(out, s);
        }
        // Sequence elements keep their order and their nulls. Order is content
        // for a `Vec` (wiki sections, steps in a trail job), and dropping a null
        // element would silently shorten the list.
        Value::Sequence(items) => {
            out.push('[');
            for item in items {
                render(item, out);
                out.push(';');
            }
            out.push(']');
        }
        Value::Mapping(map) => render_mapping(map, out, None),
        // Externally tagged enums land here. The tag is content.
        Value::Tagged(tagged) => {
            out.push('!');
            push_sized(out, &tagged.tag.to_string());
            render(&tagged.value, out);
        }
    }
}

/// `shape` is `Some` only for the top-level mapping of an entity — that is the
/// one place a key can mean identity or machine-local bookkeeping rather than
/// content.
fn render_mapping(map: &serde_yaml::Mapping, out: &mut String, shape: Option<&KindShape>) {
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(map.len());

    for (key, value) in map {
        // An absent key and a key explicitly set to null mean the same thing to
        // every loader in this codebase, so they must hash the same.
        if matches!(value, Value::Null) {
            continue;
        }
        if let (Some(shape), Value::String(name)) = (shape, key) {
            if is_not_content(shape, name.as_str()) {
                continue;
            }
        }

        let mut rendered_key = String::new();
        render(key, &mut rendered_key);
        let mut rendered_value = String::new();
        render(value, &mut rendered_value);
        pairs.push((rendered_key, rendered_value));
    }

    // The whole point. Rendered keys, not the raw `Value`s: a mapping can be
    // keyed by anything, and rendering first gives every key kind one total
    // order. Keys are unique within a mapping, so the sort is unambiguous.
    pairs.sort();

    out.push('{');
    for (key, value) in pairs {
        out.push_str(&key);
        out.push_str("=>");
        out.push_str(&value);
        out.push(';');
    }
    out.push('}');
}

/// Writes `s` length-prefixed, so no string can impersonate the structure
/// around it. Without this a project summary containing `;}` could render the
/// same as two shorter fields, and two different entities would share a hash.
fn push_sized(out: &mut String, s: &str) {
    out.push_str(&s.len().to_string());
    out.push(':');
    out.push_str(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trails::Trail;
    use crate::types::{Barn, Project, RanchHand, Worm};

    const BARN_YAML: &str = "name: pi\nhost: pi.local\n";
    const PROJECT_YAML: &str = "name: api\npath: /tmp/api\n";
    const RANCHHAND_YAML: &str = "name: rh\nproject: api\ntype: k8s\n\
         config:\n  kubeconfig_path: /tmp/kc\n\
         sync_settings:\n  auto_sync: false\n  interval_minutes: null\nherd: web\n";
    const TRAIL_YAML: &str = "name: t\njobs: {}\n";
    const WORM_YAML: &str =
        "name: w\ncommand: echo hi\nschedule: '0 0 * * *'\ntype: cron\nenabled: true\n";

    fn parse<T: serde::de::DeserializeOwned>(yaml: &str) -> T {
        serde_yaml::from_str(yaml).unwrap()
    }

    /// A `Barn` with **every** field populated, so that nothing is hidden by a
    /// `skip_serializing_if`.
    ///
    /// Six of `Barn`'s nineteen fields render no key when unset, which makes a
    /// minimally-populated instance invisible to
    /// `every_top_level_field_is_classified_as_content_or_not` — the one test
    /// standing between a new field and the content hash.
    ///
    /// The exhaustive struct literal is the tripwire. Do **not** replace it with
    /// `..Default::default()`, and do not satisfy the compiler by setting a new
    /// field to `None`: either one puts the field back out of the test's sight,
    /// which is the failure this helper exists to prevent. Give it a value.
    fn every_barn_field_set() -> Barn {
        Barn {
            name: "pi".into(),
            host: Some("pi.local".into()),
            user: Some("cam".into()),
            port: Some(22),
            identity_file: Some("~/.ssh/id_ed25519".into()),
            // Empty is fine for this one and only this one: `critters` has no
            // `skip_serializing_if`, so `critters: []` still renders the key.
            critters: vec![],
            source: Some("ranchhand:cluster".into()),
            connection_type: Some("kubernetes".into()),
            connection_config: Some(crate::types::K8sBarnConnectionConfig {
                context: "minikube".into(),
                node: "pi".into(),
            }),
            connectable: Some(true),
            synced: Some(true),
            brand: Some("ssh-ed25519 AAAAC3Nz... yeehaw-ranch-pi".into()),
            is_ranch_house: Some(false),
            tunnel_port: Some(2222),
            last_seen: Some("2026-09-10T12:00:00+00:00".into()),
            addresses: vec!["pi.local".into()],
            id: Some("3f0c1e8a-5f2b-4a55-9a3d-6d1c2b7e4f01".into()),
            created_at: Some("2026-08-01T10:00:00+00:00".into()),
            updated_at: Some("2026-09-01T10:00:00+00:00".into()),
        }
    }

    /// Deterministic by construction: the two mappings are built in opposite
    /// insertion orders, and `serde_yaml::Mapping` preserves insertion order.
    ///
    /// The obvious version of this test — two `Trail`s parsed from the same two
    /// keys in different orders — is a coin flip, because both parse into
    /// `HashMap`s and two `HashMap`s with the same two keys iterate in the same
    /// order about half the time. Measured: 105 of 200 pairs. Building the
    /// mappings by hand removes the luck.
    #[test]
    fn hash_ignores_map_ordering() {
        let keys: Vec<String> = (0..12).map(|i| format!("KEY_{}", i)).collect();

        let mut forward = serde_yaml::Mapping::new();
        for (i, key) in keys.iter().enumerate() {
            forward.insert(key.as_str().into(), format!("v{}", i).into());
        }
        let mut reverse = serde_yaml::Mapping::new();
        for (i, key) in keys.iter().enumerate().rev() {
            reverse.insert(key.as_str().into(), format!("v{}", i).into());
        }

        // Guard: if this ever fails the test above it is proving nothing.
        assert_ne!(
            forward.keys().collect::<Vec<_>>(),
            reverse.keys().collect::<Vec<_>>(),
            "the two mappings were built in the same order; the test has no teeth"
        );

        assert_eq!(
            hash_entity("trail", &Value::Mapping(forward)).unwrap(),
            hash_entity("trail", &Value::Mapping(reverse)).unwrap(),
            "key order is not content"
        );
    }

    /// The same property on the type that actually has the problem.
    ///
    /// `Trail.env` is a `HashMap`, so each parse of the same text produces a
    /// map with a fresh `RandomState` and a fresh iteration order. Twelve keys
    /// over thirty-two parses: two independently built twelve-key maps agreed
    /// on an order 0 times in 200 when measured, so an unsorted canonicalizer
    /// cannot survive this loop.
    #[test]
    fn a_trails_env_hashes_the_same_however_its_hashmap_iterates() {
        fn trail() -> crate::trails::Trail {
            let mut yaml = String::from("name: t\njobs: {}\nenv:\n");
            for i in 0..12 {
                yaml.push_str(&format!("  KEY_{}: 'v{}'\n", i, i));
            }
            serde_yaml::from_str(&yaml).unwrap()
        }

        let first = hash_entity("trail", &trail()).unwrap();
        for round in 0..32 {
            assert_eq!(
                hash_entity("trail", &trail()).unwrap(),
                first,
                "trail hash changed on round {} with no edit — env map ordering leaked in",
                round
            );
        }
    }

    /// Finding 2, from both directions.
    ///
    /// The first half — two `Barn`s parsed from a terse and a verbose file —
    /// documents the property but cannot fail: both texts parse to the same
    /// struct, so both serialize identically whatever the canonicalizer does.
    /// The second half is the one with teeth, and it is also the real case:
    /// after Slice D adds fields to `Barn`, a peer on an older build renders a
    /// barn without those keys while this build renders them as null.
    #[test]
    fn hash_ignores_explicit_null_versus_absent() {
        let terse: Barn = serde_yaml::from_str("name: pi\nhost: pi.local\n").unwrap();
        let verbose: Barn = serde_yaml::from_str(
            "name: pi\nhost: pi.local\nuser: null\nport: null\nidentity_file: null\n",
        )
        .unwrap();
        assert_eq!(hash_entity("barn", &terse).unwrap(), hash_entity("barn", &verbose).unwrap());

        // A peer that has never heard of `synced` versus one that has it unset.
        let older: Value = serde_yaml::from_str("name: pi\nhost: pi.local\n").unwrap();
        let newer: Value =
            serde_yaml::from_str("name: pi\nhost: pi.local\nsynced: null\nbrand: null\n").unwrap();
        assert_eq!(
            hash_entity("barn", &older).unwrap(),
            hash_entity("barn", &newer).unwrap(),
            "an unset new field must not make every entity look changed to an older peer"
        );
    }

    /// The same version-skew property as above, for the one Slice D field that
    /// is not an `Option` — and it does not follow from that test, because
    /// [`render_mapping`] drops *nulls*, not empty collections.
    ///
    /// A build without the ranch fields serializes a barn with no `addresses`
    /// key at all. This build, given `#[serde(default)]` alone, serializes the
    /// very same barn with `addresses: []` — one extra mapping entry, a
    /// different canonical form, a different hash. Every barn on the ranch then
    /// reads as changed to an older peer, which is precisely the failure
    /// `hash_ignores_explicit_null_versus_absent` exists to prevent and cannot
    /// catch here.
    ///
    /// Hashing the real `Barn` against a hand-built `Value` rather than two
    /// `Value`s is what gives this teeth: the `Value` is the older peer's output,
    /// and the struct is ours, so the comparison is the one that happens on the
    /// wire. Fixed by `skip_serializing_if = "Vec::is_empty"` on the field, not
    /// by teaching the canonicalizer to drop empty sequences — that would change
    /// every existing hash and would have to answer for `critters: []`,
    /// `livestock: []` and `wiki: []` as well.
    #[test]
    fn an_empty_address_list_hashes_the_same_as_a_peer_that_has_no_such_field() {
        // Every key a build *without* the ranch fields serializes for this barn,
        // nulls and empty collections included. Spelled out rather than reusing
        // the terse two-line fixture: `critters: []` is an empty sequence, which
        // survives canonicalization exactly like `addresses: []` would, so a
        // short fixture here would fail for a reason that has nothing to do with
        // the field under test.
        let older: Value = serde_yaml::from_str(
            "name: pi\nhost: pi.local\nuser: null\nport: null\nidentity_file: null\n\
             critters: []\nsource: null\nconnection_type: null\nconnection_config: null\n\
             connectable: null\n",
        )
        .unwrap();
        let ours: Barn = serde_yaml::from_str("name: pi\nhost: pi.local\n").unwrap();
        assert!(ours.addresses.is_empty(), "the fixture must leave the list unset");

        assert_eq!(
            hash_entity("barn", &older).unwrap(),
            hash_entity("barn", &ours).unwrap(),
            "a barn with no addresses must hash as it did before the field existed, or a \
             version-skewed sync reports every barn on the ranch as changed"
        );

        // And the field is not inert: a real address still has to move the hash,
        // or classifying it as content would mean nothing.
        let mut with_one = ours.clone();
        with_one.addresses = vec!["100.64.0.3".into()];
        assert_ne!(
            hash_entity("barn", &ours).unwrap(),
            hash_entity("barn", &with_one).unwrap(),
            "an address the peer does not have is a real difference"
        );
    }

    /// The one that matters most. Phase 1 stamps on every save, so an entity
    /// re-saved with no edits carries a fresh `updated_at`. If identity fed the
    /// hash, every save would look like an edit and every sync would ship the
    /// whole ranch.
    #[test]
    fn hash_ignores_the_identity_fields_but_notices_content() {
        let mut a: Project = serde_yaml::from_str("name: api\npath: /tmp\n").unwrap();
        let mut b = a.clone();
        a.id = Some("aaa".into());
        a.created_at = Some("2020-01-01T00:00:00+00:00".into());
        a.updated_at = Some("2026-01-01T00:00:00+00:00".into());
        b.id = Some("bbb".into());
        b.created_at = Some("2021-06-06T00:00:00+00:00".into());
        b.updated_at = Some("2027-01-01T00:00:00+00:00".into());
        assert_eq!(
            hash_entity("project", &a).unwrap(),
            hash_entity("project", &b).unwrap(),
            "identity is not content; a re-stamp must not look like an edit"
        );

        b.summary = Some("changed".into());
        assert_ne!(
            hash_entity("project", &a).unwrap(),
            hash_entity("project", &b).unwrap(),
            "a real content change must change the hash"
        );
    }

    /// Identity is stripped at the top level only. `id` appearing further down —
    /// inside a nested record, or inside a `RanchHand.config` blob holding a
    /// k8s resource — is ordinary content that two different resources differ
    /// on, and collapsing it would make them hash alike.
    #[test]
    fn a_nested_id_is_content_not_identity() {
        let one: Value =
            serde_yaml::from_str("name: api\npath: /tmp\nlivestock:\n  - name: web\n    id: aaa\n")
                .unwrap();
        let two: Value =
            serde_yaml::from_str("name: api\npath: /tmp\nlivestock:\n  - name: web\n    id: bbb\n")
                .unwrap();
        assert_ne!(
            hash_entity("project", &one).unwrap(),
            hash_entity("project", &two).unwrap(),
            "an id nested inside a record is content and must be hashed"
        );
    }

    /// A string is length-prefixed so it cannot impersonate the structure
    /// around it. Without that, one field holding the delimiter renders the
    /// same as two fields, and two different entities collide.
    #[test]
    fn a_value_containing_the_delimiters_cannot_forge_a_different_entity() {
        // Without the length prefix `{a}` here renders exactly as `{a, b}` below:
        // `{s:a=>s:1;s:b=>s:2;}`, because the value carries the pair separator
        // and a forged second pair inside itself.
        let sneaky: Value = serde_yaml::from_str("a: \"1;s:b=>s:2\"\n").unwrap();
        let honest: Value = serde_yaml::from_str("a: '1'\nb: '2'\n").unwrap();
        assert_ne!(
            hash_entity("project", &sneaky).unwrap(),
            hash_entity("project", &honest).unwrap(),
            "a field value must not be able to forge extra fields"
        );
    }

    // ---- C1: machine-local bookkeeping is not content ----------------------

    /// The field with teeth. `connect.rs:77` and `app.rs:1432` both *refuse* a
    /// barn marked `Some(false)`, and the mark is a verdict about the machine
    /// that made it. If it fed the hash it would sync, and a Ranch House that
    /// cannot reach a barn would push its `Some(false)` over a peer's
    /// `Some(true)` — leaving the user unable to connect to a host they can
    /// demonstrably reach.
    #[test]
    fn a_barns_reachability_verdict_is_not_content() {
        let mut here: Barn = parse(BARN_YAML);
        let mut there = here.clone();
        here.connectable = Some(true);
        there.connectable = Some(false);

        assert_eq!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "`connectable` is a per-machine verdict; syncing it costs the user a reachable host"
        );

        there.host = Some("pi2.local".into());
        assert_ne!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "a real content change to a barn must still move the hash"
        );
    }

    // ---- D1: the six ranch fields ------------------------------------------

    /// The one with teeth, and the reason the classification guard exists.
    ///
    /// `last_seen` is written on every reachability check, so two machines that
    /// hold the very same barn essentially never agree on it. As content it makes
    /// every barn read as edited on every sync — "changed on both sides", so a
    /// conflict, so a prompt, forever — which is exactly the
    /// `RanchHand.last_sync` failure one struct over. It is also a verdict about
    /// *this* machine's reach, like `connectable`: "when did I last see it", not
    /// "when was it last seen".
    #[test]
    fn a_barns_last_seen_is_not_content() {
        let mut here: Barn = parse(BARN_YAML);
        let mut there = here.clone();
        here.last_seen = Some("2026-09-10T09:00:00+00:00".into());
        there.last_seen = Some("2026-09-10T09:00:01+00:00".into());

        assert_eq!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "`last_seen` moves on every reachability check; hashing it conflicts every barn on \
             every sync — the `RanchHand.last_sync` bug again"
        );

        there.host = Some("pi2.local".into());
        assert_ne!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "a real content change to a barn must still move the hash"
        );
    }

    /// Enrolment is a relationship, not a property of the barn. The Ranch House
    /// having enrolled a barn says nothing about whether a laptop has, so the two
    /// sides disagree permanently on a field neither is wrong about — and the
    /// merge's "they changed it, we did not" branch would then push `Some(false)`
    /// onto a machine that is actively syncing, switching its sync off. Same
    /// shape of loss as `connectable`.
    #[test]
    fn whether_this_machine_syncs_a_barn_is_not_content() {
        let mut here: Barn = parse(BARN_YAML);
        let mut there = here.clone();
        here.synced = Some(true);
        there.synced = Some(false);

        assert_eq!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "`synced` is this machine's relationship to the barn, not the barn's own state"
        );
    }

    /// A local forwarded port: whatever is free on the iMac is taken on the Pi.
    /// The barn's own advertised reachability is `addresses`, which *is* content.
    #[test]
    fn a_barns_local_tunnel_port_is_not_content() {
        let mut here: Barn = parse(BARN_YAML);
        let mut there = here.clone();
        here.tunnel_port = Some(2222);
        there.tunnel_port = Some(2223);

        assert_eq!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "a port allocated on this machine cannot be a fact about the barn"
        );
    }

    /// The other direction, and just as load-bearing: these three have to reach
    /// the peer, so they must be in the hash or the sync never notices they
    /// differ and never ships them.
    ///
    /// - `brand` is the barn's ed25519 public half. Task D3 writes it into every
    ///   other barn's `authorized_keys`; a brand that does not propagate is a
    ///   managed block with nothing in it.
    /// - `is_ranch_house` has to travel or a third machine joining via a peer
    ///   never learns who arbitrates — and the merge's tie-break is "the house
    ///   wins".
    /// - `addresses` is how a joining machine reaches anything at all, which is
    ///   the whole goal of the slice.
    #[test]
    fn a_barns_brand_and_ranch_house_flag_and_addresses_are_content() {
        let base: Barn = parse(BARN_YAML);

        for (what, mutate) in [
            ("brand", (|b: &mut Barn| b.brand = Some("ssh-ed25519 AAAA".into())) as fn(&mut Barn)),
            ("is_ranch_house", |b: &mut Barn| b.is_ranch_house = Some(true)),
            ("addresses", |b: &mut Barn| b.addresses = vec!["100.64.0.3".into()]),
        ] {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(
                hash_entity("barn", &base).unwrap(),
                hash_entity("barn", &changed).unwrap(),
                "`{}` must be content: stripped from the hash it never looks different, so the \
                 sync never ships it and the feature it exists for never works",
                what
            );
        }
    }

    /// `config::update_ranchhand_last_sync` writes `Utc::now()` on every
    /// ranch-hand sync, so two machines essentially never agree on it. Feeding
    /// it to the hash makes every ranch hand "changed on both sides" and so a
    /// conflict, on every sync, forever.
    #[test]
    fn a_ranch_hands_last_sync_is_not_content() {
        let mut here: RanchHand = parse(RANCHHAND_YAML);
        let mut there = here.clone();
        here.last_sync = Some("2026-01-01T00:00:00+00:00".into());
        there.last_sync = Some("2027-09-09T12:34:56+00:00".into());

        assert_eq!(
            hash_entity("ranchhand", &here).unwrap(),
            hash_entity("ranchhand", &there).unwrap(),
            "`last_sync` differs on both sides always; hashing it conflicts every ranch hand"
        );

        there.herd = "workers".into();
        assert_ne!(
            hash_entity("ranchhand", &here).unwrap(),
            hash_entity("ranchhand", &there).unwrap(),
            "a real content change to a ranch hand must still move the hash"
        );
    }

    /// The merge gives `RanchHand.config` an unconditional local win — it holds
    /// this machine's kubeconfig path and S3 references. A field the merge
    /// refuses to take from a peer must not be able to *say* the entity changed
    /// either, or the "they changed it, we did not" branch clobbers the local
    /// path with the peer's.
    #[test]
    fn a_ranch_hands_machine_local_config_is_not_content() {
        let mut here: RanchHand = parse(RANCHHAND_YAML);
        let mut there = here.clone();
        here.config = serde_yaml::from_str("kubeconfig_path: /Users/cam/.kube/config").unwrap();
        there.config = serde_yaml::from_str("kubeconfig_path: /home/pi/.kube/config").unwrap();

        assert_eq!(
            hash_entity("ranchhand", &here).unwrap(),
            hash_entity("ranchhand", &there).unwrap(),
            "`config` is barn-scoped; the local value always wins and it must not signal a change"
        );
    }

    /// A project's `path` is where its checkout lives on *this* machine.
    ///
    /// The design rejected syncing filesystem paths verbatim — "which breaks the
    /// moment a Linux Pi joins" — and the Layer 3 table lists no `path` among the
    /// project scalars. Hashing it is the same mistake from the other side: the
    /// Ranch House's `/Users/cam/Sites/api` differs from a Pi's
    /// `/home/cam/api`, so every project would read as changed on both sides,
    /// conflict, and the losing machine's `expand_path(&project.path)` would then
    /// open nothing.
    #[test]
    fn a_projects_path_is_not_content() {
        let mut here: Project = parse(PROJECT_YAML);
        let mut there = here.clone();
        here.path = "/Users/cam/Sites/api".into();
        there.path = "/home/cam/api".into();

        assert_eq!(
            hash_entity("project", &here).unwrap(),
            hash_entity("project", &there).unwrap(),
            "`path` is where the checkout lives on this machine; syncing it costs the losing \
             machine the ability to open the project at all"
        );

        there.summary = Some("changed".into());
        assert_ne!(
            hash_entity("project", &here).unwrap(),
            hash_entity("project", &there).unwrap(),
            "a real content change to a project must still move the hash"
        );
    }

    /// Identity plus `path` is the whole of a project's not-content list — and
    /// the re-stamp case is the one that would otherwise ship the entire ranch on
    /// every save.
    #[test]
    fn a_projects_stamp_is_not_content_but_its_summary_is() {
        let mut here: Project = parse(PROJECT_YAML);
        let mut there = here.clone();
        here.id = Some("aaa".into());
        here.updated_at = Some("2026-01-01T00:00:00+00:00".into());
        there.id = Some("bbb".into());
        there.updated_at = Some("2027-01-01T00:00:00+00:00".into());
        assert_eq!(
            hash_entity("project", &here).unwrap(),
            hash_entity("project", &there).unwrap()
        );

        there.summary = Some("changed".into());
        assert_ne!(
            hash_entity("project", &here).unwrap(),
            hash_entity("project", &there).unwrap()
        );
    }

    #[test]
    fn a_trails_stamp_is_not_content_but_its_jobs_are() {
        let mut here: Trail = parse(TRAIL_YAML);
        let mut there = here.clone();
        here.updated_at = Some("2026-01-01T00:00:00+00:00".into());
        there.updated_at = Some("2027-01-01T00:00:00+00:00".into());
        assert_eq!(
            hash_entity("trail", &here).unwrap(),
            hash_entity("trail", &there).unwrap()
        );

        there = parse("name: t\njobs:\n  build:\n    steps:\n      - name: s\n        run: make\n");
        assert_ne!(
            hash_entity("trail", &here).unwrap(),
            hash_entity("trail", &there).unwrap()
        );
    }

    #[test]
    fn a_worms_stamp_is_not_content_but_its_schedule_is() {
        let mut here: Worm = parse(WORM_YAML);
        let mut there = here.clone();
        here.updated_at = Some("2026-01-01T00:00:00+00:00".into());
        there.updated_at = Some("2027-01-01T00:00:00+00:00".into());
        assert_eq!(
            hash_entity("worm", &here).unwrap(),
            hash_entity("worm", &there).unwrap()
        );

        there.schedule = "*/5 * * * *".into();
        assert_ne!(
            hash_entity("worm", &here).unwrap(),
            hash_entity("worm", &there).unwrap()
        );
    }

    /// The guard that makes the next field a decision instead of an accident.
    ///
    /// Slice D adds six fields to `Barn`; without this they would join the hash
    /// by default and `last_seen` alone would make every barn a permanent
    /// conflict. Serializing an instance catches them because an `Option` with
    /// no `skip_serializing_if` still renders its key as `null` — which is why
    /// this reads the keys *before* nulls are dropped.
    ///
    /// Checked in both directions: an unclassified key means someone added a
    /// field without deciding, and a classified key that never appears means a
    /// typo in the table, which silently classifies nothing.
    ///
    /// ## Why `barn` is built from a struct literal and the others are parsed
    ///
    /// Serializing a *minimal* instance cannot see a field carrying
    /// `skip_serializing_if` — it renders no key, so neither direction of the
    /// check applies to it. That was harmless while only the identity three had
    /// the attribute. Slice D gave six more to `Barn`, all of them
    /// `Option::is_none` or `Vec::is_empty`, and measured: with the terse
    /// `BARN_YAML` fixture this test went **green** with all six unclassified and
    /// silently in the content hash — including `last_seen`, which is written on
    /// every reachability check and would have made every barn a permanent
    /// conflict. One of six was caught, and only because `addresses` had not
    /// been given its attribute yet.
    ///
    /// So `barn`'s keys come from [`every_barn_field_set`], where every field is
    /// populated and nothing can be skipped. The exhaustive struct literal in
    /// that helper **is** the tripwire — the same device `wire::WireTombstone`'s
    /// `From` impls use — so a new field on `Barn` fails to compile there, and
    /// the fix is to give it a value, not to reach for `..Default::default()`.
    ///
    /// The other four kinds stay on their YAML fixtures because the identity
    /// three are still the only fields they skip, and those are classified by
    /// name. Give any of them a `skip_serializing_if` field and it needs the same
    /// treatment.
    #[test]
    fn every_top_level_field_is_classified_as_content_or_not() {
        fn keys_of_instance<T: Serialize>(entity: &T) -> Vec<String> {
            match serde_yaml::to_value(entity).unwrap() {
                Value::Mapping(map) => map
                    .keys()
                    .map(|k| k.as_str().expect("entity keys are strings").to_string())
                    .collect(),
                other => panic!("an entity must serialize as a mapping, got {:?}", other),
            }
        }
        fn keys_of<T: serde::de::DeserializeOwned + Serialize>(yaml: &str) -> Vec<String> {
            keys_of_instance(&parse::<T>(yaml))
        }

        let observed: Vec<(&str, Vec<String>)> = vec![
            ("barn", keys_of_instance(&every_barn_field_set())),
            ("project", keys_of::<Project>(PROJECT_YAML)),
            ("ranchhand", keys_of::<RanchHand>(RANCHHAND_YAML)),
            ("trail", keys_of::<Trail>(TRAIL_YAML)),
            ("worm", keys_of::<Worm>(WORM_YAML)),
        ];

        for (kind, keys) in &observed {
            let shape = shape_for(kind).unwrap();
            for key in keys {
                let classified = IDENTITY_KEYS.contains(&key.as_str())
                    || shape.machine_local.contains(&key.as_str())
                    || shape.content.contains(&key.as_str());
                assert!(
                    classified,
                    "`{}.{}` is in neither `content` nor `machine_local` in `canonical::SHAPES`, \
                     so it joined the content hash by default. Decide: does its value describe \
                     the entity (content) or the machine holding the file (machine_local)?",
                    kind, key
                );
            }

            for listed in shape.content.iter().chain(shape.machine_local.iter()) {
                assert!(
                    keys.iter().any(|k| k == listed) || IDENTITY_KEYS.contains(listed),
                    "`canonical::SHAPES` classifies `{}.{}`, but no such key is serialized — \
                     a misspelled entry classifies nothing",
                    kind, listed
                );
            }
        }
    }

    /// The shape table and the manifest's kind list are two spellings of the
    /// same set. A kind in one and not the other is a manifest that cannot be
    /// hashed, or a shape nothing reaches.
    #[test]
    fn the_shape_table_covers_exactly_the_kinds_a_sync_moves() {
        let mut shaped: Vec<&str> = SHAPES.iter().map(|s| s.kind).collect();
        shaped.sort_unstable();
        let mut synced: Vec<&str> = crate::ranch::manifest::KINDS.to_vec();
        synced.sort_unstable();
        assert_eq!(shaped, synced);
    }

    /// A kind with no registered shape has no known not-content list, so there
    /// is no safe hash to compute for it — guessing means silently hashing
    /// whatever machine-local field it turns out to carry.
    #[test]
    fn an_unregistered_kind_is_refused_rather_than_guessed() {
        let p: Project = parse(PROJECT_YAML);
        let err = hash_entity("livestock", &p).expect_err("only the five kinds have shapes");
        assert!(
            err.to_string().contains("livestock"),
            "the error must name the kind: {}",
            err
        );
    }
}
