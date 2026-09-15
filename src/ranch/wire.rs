//! Newline-delimited JSON over stdio. One message per line.
//!
//! NDJSON rather than the sentinel framing `remote_grid` uses: JSON cannot
//! contain a raw newline, so there are no framing edge cases to get wrong, and
//! a transcript is readable when something goes sideways. Entity payloads ride
//! as YAML strings inside JSON fields — the same bytes the store would write.

use serde::{Deserialize, Serialize};

/// One entity as it appears in a manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// `"project" | "barn" | "worm" | "trail" | "ranchhand"`.
    pub kind: String,
    pub name: String,
    /// `None` for an entity that has never been stamped — most of a real ranch
    /// today, because nothing restamps a file until it is next written.
    pub id: Option<String>,
    pub updated_at: Option<String>,
    /// Hash of the *canonical* form, never of the file's bytes: most `Option`
    /// fields serialize as an explicit `null` and `Trail.env` is a `HashMap`,
    /// so byte equality cannot answer "did this change?".
    pub hash: String,
}

/// One deletion as it appears on the wire.
///
/// Deliberately *not* `crate::tombstones::Tombstone`, which it currently
/// mirrors field for field. That type is an on-disk record with its own reasons
/// to change — Task C5 already contemplates reworking how its `{kind}--{name}`
/// fallback id is reasoned about — and while it was embedded here directly,
/// every edit to it silently redefined the protocol: a renamed field breaks
/// decoding for every peer on an older build, an added one ships to the wire
/// without anybody choosing to.
///
/// The alternative considered was leaving the storage type in place under a
/// comment declaring it protocol-frozen. Rejected because the comment would sit
/// in `wire.rs` while the change happens in `tombstones.rs` — invisible at the
/// point of the edit. The `From` impls below destructure exhaustively instead,
/// so the same edit stops the build *here*, where [`super::PROTOCOL_VERSION`]
/// is the thing to reconsider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireTombstone {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub deleted_at: String,
}

impl From<crate::tombstones::Tombstone> for WireTombstone {
    fn from(t: crate::tombstones::Tombstone) -> Self {
        // The exhaustive destructure *is* the tripwire; do not replace it with
        // field access. A new field on the stored record fails to compile here,
        // which is the whole point of the type existing.
        let crate::tombstones::Tombstone { id, kind, name, deleted_at } = t;
        Self { id, kind, name, deleted_at }
    }
}

impl From<WireTombstone> for crate::tombstones::Tombstone {
    fn from(t: WireTombstone) -> Self {
        let WireTombstone { id, kind, name, deleted_at } = t;
        Self { id, kind, name, deleted_at }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum Message {
    Hello { protocol: u32, barn: String, is_ranch_house: bool },
    /// A joining machine asking the Ranch House what it is to be called.
    ///
    /// `brand` is the machine's **identity** and `proposed` is only a
    /// suggestion. A hostname was never a ranch-wide name — two Raspberry Pis
    /// are both `pi`, and the second one to join would collide with the first on
    /// the roster — whereas the public half of a brand is unique per machine and
    /// already crosses the wire as `Barn.brand`. So the house, which owns the
    /// roster, is the naming authority, and this is the message that asks it.
    /// See [`super::assign_name`] for the policy it answers with.
    ClaimName { proposed: String, brand: String },
    /// The house's answer to [`Message::ClaimName`]: the name the joining
    /// machine is to adopt itself under. A refusal is an [`Message::Error`]
    /// carrying what is taken and what to do instead, not a name.
    NameAssigned { name: String },
    Manifest { entries: Vec<ManifestEntry>, tombstones: Vec<WireTombstone> },
    /// `"kind/id"`, or `"kind/name"` for an entity with no uuid yet.
    Want { keys: Vec<String> },
    /// One entity, in **either** direction.
    ///
    /// Sent by a `serve` answering a [`Message::Want`], and sent by a joining
    /// client pushing what it holds that the house does not. The payload is
    /// identical both ways — it is the bytes the store would write — so there is
    /// no second message type for the push, and no way for the two directions to
    /// drift apart in what an entity *is*.
    ///
    /// Deliberately **not individually answered** when it is a push. The batch
    /// is answered once, at its terminator, exactly as the house's own entity
    /// batch is terminated by [`Message::Done`] rather than acknowledged frame
    /// by frame: both sides are blocking writers on a pipe with no timeout, and
    /// a reply per entity is a round trip per entity for no information the
    /// batch answer does not already carry.
    ///
    /// What it cannot express, and therefore what a push cannot do: a rename
    /// (the receiver would have to delete the file the entity used to live in)
    /// and a deletion. See [`super::client::pushable`] for why that is the
    /// chosen boundary rather than an oversight.
    Entity { kind: String, name: String, yaml: String },
    /// The terminator of a **pushed** entity batch: "that is everything I am
    /// offering — apply what you can and tell me what landed".
    ///
    /// Separate from [`Message::Done`], which is a goodbye. A receiver must be
    /// able to tell "apply the batch" from "the session is over", because the
    /// two have opposite consequences for a peer that dies between the last
    /// entity and the next frame: a dropped `Commit` must write nothing.
    Commit,
    /// The answer to [`Message::Commit`]: the keys of the entities that were
    /// **actually written**, in [`Message::Want`]'s `kind/id`-or-`kind/name`
    /// spelling.
    ///
    /// The list is per entity rather than a yes/no for the batch, because there
    /// is no transaction over N independent file writes: a batch answer could
    /// only ever be a guess, and the half of that guess which says "all of it
    /// landed" when some of it did not is the drift direction `base.rs` calls
    /// unrecoverable. The sender records a sync base only for the keys named
    /// here.
    Applied { keys: Vec<String> },
    Done,
    Error { message: String },
    /// Any `msg` tag this build does not know — i.e. a message type some newer
    /// peer added.
    ///
    /// **Receive only.** `skip_serializing` makes encoding this an error rather
    /// than putting `{"msg":"unknown"}` on the wire, which is a message type no
    /// peer defines. `tag_of` recovers the actual tag for an error message,
    /// since `#[serde(other)]` matches only unit variants and so cannot carry it.
    ///
    /// The policy and its reasoning are pinned by
    /// `an_unknown_message_type_decodes_as_unknown_rather_than_as_corruption`.
    /// Every `match` on `Message` has to decide what to do with this arm, which
    /// is the point: tolerating it at the decode layer is not the same as
    /// ignoring it at the protocol layer.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl Message {
    /// The `msg` tag this variant carries on the wire.
    ///
    /// Kept in step with the `#[serde(rename_all = "snake_case")]` above by
    /// `every_label_matches_the_tag_on_the_wire`; the match is exhaustive, so a
    /// new variant has to name itself here before it compiles. Used where an
    /// error has to say *what* failed to cross — "sending manifest to the peer"
    /// beats "Broken pipe (os error 32)".
    pub fn label(&self) -> &'static str {
        match self {
            Message::Hello { .. } => "hello",
            Message::ClaimName { .. } => "claim_name",
            Message::NameAssigned { .. } => "name_assigned",
            Message::Manifest { .. } => "manifest",
            Message::Want { .. } => "want",
            Message::Entity { .. } => "entity",
            Message::Commit => "commit",
            Message::Applied { .. } => "applied",
            Message::Done => "done",
            Message::Error { .. } => "error",
            Message::Unknown => "unknown",
        }
    }
}

/// The `msg` tag on a line, without committing to a variant.
///
/// Exists for one job: naming the message type behind a
/// [`Message::Unknown`]. `#[serde(other)]` may only be applied to a unit
/// variant, so the tag it matched is thrown away by the decode — and an error
/// that cannot say *which* message type went unimplemented is the useless error
/// this whole policy was meant to replace.
///
/// `None` for anything that is not a JSON object carrying a string `msg`, so a
/// caller cannot accidentally report a tag for a line that never had one.
pub fn tag_of(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(value.get("msg")?.as_str()?.to_string())
}

/// One line, no trailing newline — the caller owns the delimiter.
///
/// Compact, not pretty: `to_string_pretty` would put a real newline inside the
/// message and split one message across several frames.
pub fn encode(msg: &Message) -> Result<String, serde_json::Error> {
    serde_json::to_string(msg)
}

/// One line back into a message.
///
/// Every failure — a truncated line, an object with no `msg` tag, a known tag
/// missing its required fields, a peer's shell banner that landed in the stream
/// — comes back as `Err`. A panic here would take down a sync mid-write; an
/// error can be reported over the wire.
///
/// The one thing that is *not* a failure is an unrecognized `msg` tag: that is a
/// newer peer rather than a broken stream, and it decodes as
/// [`Message::Unknown`]. See that variant for the policy.
pub fn decode(line: &str) -> Result<Message, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_round_trips() {
        let msgs = vec![
            Message::Hello { protocol: 1, barn: "imac".into(), is_ranch_house: false },
            Message::ClaimName {
                proposed: "macbook".into(),
                // A real brand, comment and all: the space-separated shape is
                // what `assign_name` compares on.
                brand: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBq macbook".into(),
            },
            Message::NameAssigned { name: "macbook".into() },
            Message::Want { keys: vec!["project/abc".into()] },
            Message::Entity {
                kind: "project".into(),
                name: "api".into(),
                // A payload containing the delimiter is the whole risk here.
                yaml: "name: api\nsummary: |\n  multi\n  line\n".into(),
            },
            Message::Commit,
            Message::Applied { keys: vec!["project/abc".into()] },
            Message::Done,
            Message::Error { message: "nope".into() },
        ];
        for m in msgs {
            let line = encode(&m).unwrap();
            assert!(!line.contains('\n'), "an encoded message must occupy one line: {:?}", line);
            assert_eq!(decode(&line).unwrap(), m, "round trip changed the message");
        }
    }

    #[test]
    fn a_payload_with_newlines_survives_the_wire() {
        let yaml = "a: 1\nb: |\n  line one\n  line two\n";
        let line = encode(&Message::Entity {
            kind: "project".into(), name: "x".into(), yaml: yaml.into(),
        }).unwrap();
        assert_eq!(line.matches('\n').count(), 0, "the payload's newlines must be escaped");
        match decode(&line).unwrap() {
            Message::Entity { yaml: got, .. } => assert_eq!(got, yaml, "payload was mangled"),
            other => panic!("decoded as the wrong variant: {:?}", other),
        }
    }

    fn a_stored_tombstone() -> crate::tombstones::Tombstone {
        crate::tombstones::Tombstone {
            id: "3f0c1e8a-5f2b-4a55-9a3d-6d1c2b7e4f01".into(),
            kind: "project".into(),
            name: "api".into(),
            deleted_at: "2026-09-09T12:00:00+00:00".into(),
        }
    }

    /// The protocol's shape, pinned as text. `Tombstone` is a storage record
    /// that will keep evolving; this asserts what a peer on another build
    /// actually has to parse, so a change to that record cannot quietly
    /// redefine it — it fails here, next to `PROTOCOL_VERSION`.
    #[test]
    fn a_tombstone_crosses_the_wire_in_the_shape_the_protocol_promises() {
        let line = encode(&Message::Manifest {
            entries: vec![],
            tombstones: vec![a_stored_tombstone().into()],
        })
        .unwrap();

        assert_eq!(
            line,
            r#"{"msg":"manifest","entries":[],"tombstones":[{"id":"3f0c1e8a-5f2b-4a55-9a3d-6d1c2b7e4f01","kind":"project","name":"api","deleted_at":"2026-09-09T12:00:00+00:00"}]}"#
        );
    }

    /// And back again, losslessly — a wire tombstone has to be usable as the
    /// stored record a merge will act on.
    #[test]
    fn a_tombstone_survives_the_round_trip_through_both_conversions() {
        let stored = a_stored_tombstone();
        let line = encode(&Message::Manifest {
            entries: vec![],
            tombstones: vec![stored.clone().into()],
        })
        .unwrap();

        match decode(&line).unwrap() {
            Message::Manifest { tombstones, .. } => {
                let back: crate::tombstones::Tombstone = tombstones[0].clone().into();
                assert_eq!(back, stored, "the wire lost or reshaped a tombstone");
            }
            other => panic!("decoded as the wrong variant: {:?}", other),
        }
    }

    /// `label` names a message for error text, so it has to be the name the
    /// peer sees, not a second vocabulary that drifts from the first.
    #[test]
    fn every_label_matches_the_tag_on_the_wire() {
        let msgs = vec![
            Message::Hello { protocol: 1, barn: "imac".into(), is_ranch_house: false },
            Message::ClaimName { proposed: "macbook".into(), brand: "ssh-ed25519 AAAA x".into() },
            Message::NameAssigned { name: "macbook".into() },
            Message::Manifest { entries: vec![], tombstones: vec![] },
            Message::Want { keys: vec![] },
            Message::Entity { kind: "project".into(), name: "api".into(), yaml: String::new() },
            Message::Commit,
            Message::Applied { keys: vec![] },
            Message::Done,
            Message::Error { message: "nope".into() },
        ];
        for m in msgs {
            let v: serde_json::Value = serde_json::from_str(&encode(&m).unwrap()).unwrap();
            assert_eq!(v["msg"], m.label(), "label disagrees with the wire tag for {:?}", m);
        }
    }

    /// DELIBERATE CHANGE, Slice D. This test used to assert
    /// `decode(r#"{"msg":"no_such_variant"}"#).is_err()`, which was a statement
    /// about the *unknown-variant policy* hiding inside a test about corruption
    /// — and the policy had never been chosen. It is now chosen (tolerate; see
    /// [`Message::Unknown`]) and pinned by
    /// `an_unknown_message_type_decodes_as_unknown_rather_than_as_corruption`,
    /// so that assertion moved there rather than silently inverting here.
    ///
    /// What is left is the part this test was always about, plus the two cases
    /// that prove tolerating an unknown *tag* did not turn into tolerating
    /// anything: an object with no tag at all, and a known tag whose required
    /// fields are missing.
    #[test]
    fn a_corrupt_line_is_an_error_not_a_panic() {
        assert!(decode("{not json").is_err(), "truncated JSON");
        assert!(decode("").is_err(), "an empty line is not a frame");
        assert!(decode("null").is_err(), "valid JSON that is not an object");
        assert!(decode("{}").is_err(), "no `msg` key is not an unknown message type");
        assert!(
            decode(r#"{"protocol":1,"barn":"pi","is_ranch_house":false}"#).is_err(),
            "a message body with no tag cannot be guessed at"
        );
        assert!(
            decode(r#"{"msg":"hello"}"#).is_err(),
            "a known tag missing its required fields is corruption, not a new message type"
        );
    }

    // ---- D0: the unknown-variant policy ----------------------------------

    /// THE POLICY, chosen in Slice D: **tolerate**. An unrecognized `msg` tag
    /// decodes as [`Message::Unknown`] instead of failing.
    ///
    /// Why tolerate rather than fatal:
    ///
    /// - `PROTOCOL_VERSION` is bumped only for changes that are *not*
    ///   backwards compatible, and [`super::check_protocol`] is the gate for
    ///   those. Adding a whole new message type that an older peer can simply
    ///   decline is the textbook *compatible* change — so if an unknown tag
    ///   were fatal, every new message type would force a version bump and a
    ///   ranch-wide flag day, which is the thing `MIN_COMPATIBLE_PROTOCOL`
    ///   exists to avoid.
    /// - It is the only way to tell the two cases apart. Today an unknown
    ///   variant and a truncated frame arrive as the same `Err`, so the best
    ///   error available is "not a protocol message". With this, the receiver
    ///   can say *which* message type it does not implement, which is the
    ///   difference between a user upgrading the right machine and filing a bug.
    /// - The stream stays in sync. A fatal decode kills the session mid-read,
    ///   so the refusal cannot even be *sent*; a tolerated one leaves a
    ///   well-formed stream on which `Message::Error` still fits.
    ///
    /// Tolerate at the *decode* layer is not "ignore" at the protocol layer.
    /// `serve` answers an `Unknown` with `Message::Error` naming the tag — it
    /// never proceeds as though the message had not happened. What the variant
    /// buys is that a future handler *can* decide to skip a message type
    /// designed as optional, at the point where that information exists.
    #[test]
    fn an_unknown_message_type_decodes_as_unknown_rather_than_as_corruption() {
        assert_eq!(
            decode(r#"{"msg":"no_such_variant"}"#).unwrap(),
            Message::Unknown,
            "a tag this build does not know is a newer peer, not a corrupt line"
        );
        assert_eq!(
            decode(r#"{"msg":"lease","ttl":30,"holder":"imac"}"#).unwrap(),
            Message::Unknown,
            "an unknown tag's payload is discarded without being parsed"
        );
    }

    /// `Unknown` is a shape for *receiving*. Serializing it would put
    /// `{"msg":"unknown"}` on the wire — a message no peer defines, sent by a
    /// build that by definition does not know what it is relaying.
    #[test]
    fn the_unknown_variant_is_receive_only_and_cannot_be_put_on_the_wire() {
        assert!(
            encode(&Message::Unknown).is_err(),
            "encoding `Unknown` would invent a message type; it must be refused"
        );
    }

    /// `#[serde(other)]` only matches unit variants, so it cannot capture the
    /// tag it matched — and an error that cannot name the message type is the
    /// error we already had. This is how the name is recovered.
    #[test]
    fn an_unknown_tag_can_still_be_named_back_to_the_peer() {
        assert_eq!(tag_of(r#"{"msg":"lease","ttl":30}"#).as_deref(), Some("lease"));
        assert_eq!(tag_of(r#"{"msg":"done"}"#).as_deref(), Some("done"));
        assert_eq!(tag_of("{not json"), None, "a non-JSON line has no tag to name");
        assert_eq!(tag_of("{}"), None, "an object with no `msg` key has no tag to name");
        assert_eq!(tag_of(r#"{"msg":7}"#), None, "a non-string tag is not a name");
    }
}
