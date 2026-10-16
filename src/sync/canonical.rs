//! Canonical form and content hashing.
//!
//! A sync has to answer one question about every entity: *is this the same
//! entity the peer already has?* Byte comparison cannot answer it, and neither
//! can a hash of the file's bytes:
//!
//! - Almost nothing carries `skip_serializing_if` — only the three `Identified`
//!   fields on every kind. Every *other* `Option` on `Barn`, `Project`,
//!   `Livestock` and `Critter` serializes as an explicit `null`, so a
//!   hand-written four-line barn file comes back from a round trip as a dozen
//!   lines with identical meaning.
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
//! from the copy the peer holds and every save would look like a change. `id`
//! and `created_at` come out for the same reason from the other direction: two
//! machines that created the same record by hand minted their uuids
//! independently, so identity is exactly what has *not* got to match for two
//! records to be the same thing.
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
//! Dropping nulls does not cover empty collections, and deliberately so: teaching
//! the canonicalizer to drop empty sequences would change every existing hash and
//! would have to answer for `critters: []`, `livestock: []` and `wiki: []`. A new
//! `Vec` field that older peers lack wants `skip_serializing_if = "Vec::is_empty"`
//! instead.
//!
//! ## Identity is a special case of "machine-local bookkeeping is not content"
//!
//! The argument above is not really about identity. It is about *fields whose
//! value is a verdict about the machine holding the file*. Such a field differs
//! between two machines that hold the very same entity, and it is not a change
//! either side made, so a comparison that counted it would report a difference
//! that can never be resolved.
//!
//! Six such fields exist beyond the identity three, and they are not the same
//! for every kind:
//!
//! - **`Project.path`** — where the project's checkout lives on *this* machine.
//!   `/Users/cam/Sites/api` on one machine and `/home/cam/api` on a Pi are two
//!   machines' answers to a local question, not a disagreement about the
//!   project: hashed, every project reads as different on every comparison, and
//!   a copy that took the other side's path could no longer
//!   `expand_path(&project.path)` into anything that exists.
//! - **`Barn.connectable`** — a per-machine reachability verdict, written by
//!   discovery (`ranchhand_k8s`, `ranchhand_terraform`) and by
//!   `migrate::adopt_this_machine`. It is not decorative: `connect.rs` and
//!   `app.rs` both *hard refuse* a barn marked `Some(false)`. If it fed the
//!   hash, a machine that cannot reach a barn would offer its `Some(false)` over
//!   a peer's `Some(true)` and the user would lose the ability to connect to a
//!   host they can demonstrably reach.
//! - **`Barn.identity_file`** — a key path under *this* machine's `~/.ssh`.
//! - **`Worm.working_dir`** — a directory on *this* machine, as `Project.path`.
//! - **`RanchHand.last_sync`** — `Utc::now()`, written by
//!   `config::update_ranchhand_last_sync` on every ranch-hand sync. It differs
//!   on both sides essentially always.
//! - **`RanchHand.config`** — untyped YAML holding a `kubeconfig_path` or an S3
//!   reference, which only means something on the machine that wrote it.
//!
//! So the not-content list is **per kind**, not flat: identity applies to all
//! five, `path` only to `project`, `connectable` and `identity_file` only to
//! `barn`, `working_dir` only to `worm`, `last_sync` and `config` only to
//! `ranchhand`. See [`SHAPES`].
//!
//! ## Adding a field is a decision, not a default
//!
//! [`SHAPES`] lists *every* top-level key of every kind, split into content and
//! machine-local — not just the ones that get stripped. That redundancy is the
//! point: `every_top_level_field_is_classified` serializes one instance of each
//! kind and fails when a key appears in neither list, so a new field cannot join
//! the hash by default. It has to be classified on the way in.
//!
//! A field carrying `skip_serializing_if` renders no key when unset, so a
//! minimally populated instance makes it invisible to *both* directions of that
//! check. `barn`'s keys therefore come from an exhaustively populated struct
//! literal (`tests::every_barn_field_set`) rather than from a terse YAML
//! fixture: a new field on `Barn` fails to compile there, and the only way to
//! satisfy it is to give the field a value — at which point the classification
//! check sees it. The other four kinds are still read from YAML fixtures, which
//! is sound only because the identity three are the only fields they skip, and
//! those are classified by name.
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
//!   which is genuinely shared. Verdict: content.
//!
//! ## Order is content here
//!
//! Sequences keep their order (see [`render`]): `TrailStep`s in a job and
//! `WikiSection`s in a wiki are order-significant, and sorting them would make
//! two genuinely different trails hash alike. Do not blanket-sort sequences
//! here: that would break trail steps, where order is the program.

#![allow(dead_code)] // Consumed by the sync view.

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
        // `identity_file` is a key under this machine's `~/.ssh`: another
        // machine's path to its own key means nothing here.
        machine_local: &["connectable", "identity_file"],
        content: &[
            "name",
            "host",
            "user",
            "port",
            "critters",
            // Provenance, and shared. See "The `source` audit" in the
            // module docs — this is the one field on `Barn` where the verdict
            // was close.
            "source",
            "connection_type",
            "connection_config",
        ],
    },
    KindShape {
        kind: "project",
        // Where the checkout lives on *this* machine: `/Users/cam/Sites/api`
        // here and `/home/cam/api` on a Pi are two machines' answers to a local
        // question, not a disagreement about the project.
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
        // A directory on this machine, like `project.path`.
        machine_local: &["working_dir"],
        content: &["name", "command", "schedule", "type", "enabled", "project"],
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
    /// A field that renders no key when unset makes a minimally-populated
    /// instance invisible to
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

    /// Null versus absent, from both directions.
    ///
    /// The first half — two `Barn`s parsed from a terse and a verbose file —
    /// documents the property but cannot fail: both texts parse to the same
    /// struct, so both serialize identically whatever the canonicalizer does.
    /// The second half is the one with teeth, and it is also the real case:
    /// after a field is added to `Barn`, a peer on an older build renders a
    /// barn without that key while this build renders it as null.
    #[test]
    fn hash_ignores_explicit_null_versus_absent() {
        let terse: Barn = serde_yaml::from_str("name: pi\nhost: pi.local\n").unwrap();
        let verbose: Barn = serde_yaml::from_str(
            "name: pi\nhost: pi.local\nuser: null\nport: null\nidentity_file: null\n",
        )
        .unwrap();
        assert_eq!(hash_entity("barn", &terse).unwrap(), hash_entity("barn", &verbose).unwrap());

        // A peer that has never heard of `connectable` versus one that has it unset.
        let older: Value = serde_yaml::from_str("name: pi\nhost: pi.local\n").unwrap();
        let newer: Value =
            serde_yaml::from_str("name: pi\nhost: pi.local\nconnectable: null\n").unwrap();
        assert_eq!(
            hash_entity("barn", &older).unwrap(),
            hash_entity("barn", &newer).unwrap(),
            "an unset new field must not make every entity look changed to an older peer"
        );
    }

    /// The one that matters most. `Identified::stamp` runs on every save, so an
    /// entity re-saved with no edits carries a fresh `updated_at`. If identity
    /// fed the hash, every save would look like an edit and every entity would
    /// read as different from the peer's copy.
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

    // ---- machine-local bookkeeping is not content ---------------------------

    /// The field with teeth. `connect.rs:77` and `app.rs:1432` both *refuse* a
    /// barn marked `Some(false)`, and the mark is a verdict about the machine
    /// that made it. If it fed the hash it would sync, and a machine that
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

    /// `identity_file` is a key under *this* machine's `~/.ssh`. The Pi's
    /// `/home/pi/.ssh/id_ed25519` is no answer for the iMac, and hashed, it
    /// would flag every barn as different and push one machine's key path over
    /// the other's.
    #[test]
    fn a_barns_identity_file_is_not_content() {
        let mut here: Barn = parse(BARN_YAML);
        let mut there = here.clone();
        here.identity_file = Some("/Users/cam/.ssh/id_ed25519".into());
        there.identity_file = Some("/home/pi/.ssh/id_rsa".into());
        assert_eq!(
            hash_entity("barn", &here).unwrap(),
            hash_entity("barn", &there).unwrap(),
            "`identity_file` is a path on this machine, not content"
        );
    }

    /// A worm's `working_dir` is a directory on this machine, like a project's
    /// `path`.
    #[test]
    fn a_worms_working_dir_is_not_content() {
        let mut here: Worm = parse(WORM_YAML);
        let mut there = here.clone();
        here.working_dir = Some("/Users/cam/Sites/api".into());
        there.working_dir = Some("/home/cam/api".into());
        assert_eq!(
            hash_entity("worm", &here).unwrap(),
            hash_entity("worm", &there).unwrap(),
            "`working_dir` is a path on this machine, not content"
        );
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

    /// `RanchHand.config` holds this machine's kubeconfig path and S3
    /// references. Two machines configured differently hold the same ranch hand,
    /// and the difference must not read as the entity having changed.
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
    /// `/Users/cam/Sites/api` on one machine differs from a Pi's `/home/cam/api`,
    /// so hashed, every project would read as different, and a machine that took
    /// the other side's path would have `expand_path(&project.path)` open
    /// nothing.
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
    /// the re-stamp case is the one that would otherwise flag every project as
    /// different after every save.
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
    /// Without this a new field joins the hash by default, and a per-machine one
    /// would make every copy of the entity read as different. Serializing an instance catches them because an `Option` with
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
    /// check applies to it. With the terse `BARN_YAML` fixture this test once
    /// went green with several such fields unclassified and silently in the
    /// content hash.
    ///
    /// So `barn`'s keys come from [`every_barn_field_set`], where every field is
    /// populated and nothing can be skipped. The exhaustive struct literal in
    /// that helper **is** the tripwire, so a new field on `Barn` fails to compile there, and
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
