//! The Ranch: configuration shared between barns.

pub mod base;
pub mod brand;
pub mod canonical;
pub mod client;
pub mod manifest;
pub mod merge;
pub mod transport;
pub mod wire;

use anyhow::{Context, Result};

use crate::types::Barn;

/// The wire protocol this build speaks. Bumped when a change is not backwards
/// compatible, so a peer can refuse rather than misread.
pub const PROTOCOL_VERSION: u32 = 1;

/// The oldest peer protocol this build can still read.
///
/// Raising this is itself the breaking change: it says "this build has stopped
/// being able to parse what a peer on version N sends". It is `1` today because
/// there has only ever been one version.
pub const MIN_COMPATIBLE_PROTOCOL: u32 = 1;

/// Whether a peer declaring `peer` can be talked to, and if not, why — in words
/// meant for [`wire::Message::Error`], which is where the refusal goes.
///
/// # The policy: a closed range, and the newer side downgrades
///
/// Accepted iff `MIN_COMPATIBLE_PROTOCOL <= peer <= PROTOCOL_VERSION`.
///
/// **Not equality.** Equality makes every bump a flag day across the whole
/// ranch: `brew upgrade` on the iMac and the Pi stops syncing until someone
/// walks over to it. [`MIN_COMPATIBLE_PROTOCOL`] exists precisely so that a
/// bump which is still *readable* by older peers costs nothing.
///
/// **Not "accept anything newer and hope".** [`PROTOCOL_VERSION`]'s own
/// contract is that it is bumped "when a change is not backwards compatible".
/// A peer above our version is therefore, by that definition, able to put bytes
/// on the wire this build cannot parse — a renamed or newly-required field on
/// an existing variant breaks decoding no matter how tolerant we are about
/// unknown *variants* (see [`wire::Message::Unknown`]). Accepting it trades a
/// clean, quotable refusal at the handshake for an undecodable line in the
/// middle of an apply, which is the worst possible moment: by then locks are
/// held and entities have been written.
///
/// **So the upper bound stays at our own version, and the downgrade is the
/// newer build's job.** The newer build is the only side holding both specs, so
/// it is the only side that *can* downgrade; it declares in `Hello.protocol`
/// the version it intends to speak for this session rather than the highest it
/// knows. The older side's refusal is what forces that discipline — if an older
/// peer accepted a higher number, implementing the downgrade would be optional
/// and nobody would do it.
///
/// Note the asymmetry that falls out: against a version-2 peer it is the
/// *version-1* side that refuses, and a shipped build can never be taught
/// otherwise. That is the cost, and it is why the bound is documented here
/// rather than discovered later — a version bump has to ship its downgrade in
/// the same release, not the one after.
pub fn check_protocol(peer: u32) -> Result<(), String> {
    if peer < MIN_COMPATIBLE_PROTOCOL {
        return Err(format!(
            "this build speaks ranch protocol {} and can read back to {}; the peer declared {}, \
             which is older than anything it knows how to parse. Upgrade the peer.",
            PROTOCOL_VERSION, MIN_COMPATIBLE_PROTOCOL, peer
        ));
    }
    if peer > PROTOCOL_VERSION {
        return Err(format!(
            "this build speaks ranch protocol {} and the peer declared {}. The newer side is the \
             one that can speak both, so it has to ask for {} explicitly; this side cannot parse \
             {}. Upgrade this machine.",
            PROTOCOL_VERSION, peer, PROTOCOL_VERSION, peer
        ));
    }
    Ok(())
}

/// What `yeehaw ranch <verb>` was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum RanchCommand {
    Init { name: Option<String> },
    /// `name` is what this machine should be called on the ranch it is joining,
    /// for the same reason `init` takes one: a join adopts this machine as a
    /// barn, and `hostname -s` is frequently not what anybody wants typed at a
    /// shell. `None` keeps the bare `join <target>` form, which takes the
    /// hostname — and, on an already-adopted machine, the name it already has.
    Join { target: String, name: Option<String> },
    Serve,
    Status,
    Usage,
}

/// Reads the verb out of a full `argv`, so `args[0]` is the binary and
/// `args[1]` is `ranch`.
///
/// Anything unrecognized — including a `join` with nothing to join to — comes
/// back as `Usage` rather than an error. Parsing does not print and does not
/// exit; the caller owns both, which is what makes this testable.
pub fn parse_args(args: &[String]) -> RanchCommand {
    match args.get(2).map(|s| s.as_str()) {
        Some("init") => RanchCommand::Init { name: args.get(3).cloned() },
        Some("join") => match (args.get(3), proposed_name(args.get(4..).unwrap_or(&[]))) {
            (Some(t), Some(name)) => RanchCommand::Join { target: t.clone(), name },
            // A `--as` with nothing after it is not a name, and guessing one
            // would enroll this machine under something nobody typed.
            _ => RanchCommand::Usage,
        },
        Some("serve") => RanchCommand::Serve,
        Some("status") => RanchCommand::Status,
        _ => RanchCommand::Usage,
    }
}

/// The name a `join` proposes for this machine, from the arguments after the
/// target.
///
/// Both spellings, because both get typed: `join pi --as macbook` and the bare
/// positional `join pi macbook`. `None` (the outer one) means the arguments are
/// not usable as written — a `--as` with no value — which the caller turns into
/// usage rather than a guess.
///
/// Unrecognized flags are stepped over rather than claimed as the name: a future
/// flag on this verb must not silently become the machine's identity.
fn proposed_name(rest: &[String]) -> Option<Option<String>> {
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--as" => return rest.next().cloned().map(Some),
            flag if flag.starts_with('-') => continue,
            name => return Some(Some(name.to_string())),
        }
    }
    Some(None)
}

/// Speaks the sync protocol on stdio, for a peer that launched us as
/// `ssh <ranchhouse> yeehaw ranch serve`.
///
/// Everything written to stdout here is protocol. Diagnostics go to stderr,
/// which ssh keeps separate from the data stream — a single stray `println!`
/// anywhere on this path leaves the peer unable to parse our first message with
/// no way to say why.
///
/// We are the side that was launched, so we speak first and only then read. See
/// [`serve_session`] for what each frame is answered with and why — the one rule
/// that matters is that every frame gets an answer, because a client blocked on
/// a reply has no timeout to rescue it.
///
/// `selftest` greets and returns instead of waiting on stdin, so the purity of
/// this path can be checked without a peer on the other end.
pub fn serve(selftest: bool) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();

    if selftest {
        // Greet and go: no peer means nothing to read, and the point of the
        // flag is to check this path's *purity*, not its conversation.
        return send(&mut out, &greeting());
    }

    // `StdinLock` is already a `BufRead`, so no extra buffering is wanted here:
    // a second buffer over the same fd is a place for a frame to sit unread.
    serve_session(std::io::stdin().lock(), out)
}

/// The `Hello` this build opens every session with.
fn greeting() -> wire::Message {
    wire::Message::Hello {
        protocol: PROTOCOL_VERSION,
        // Empty until enrollment names this machine. A peer reads it as "not yet
        // adopted" rather than as a barn called "".
        barn: crate::config::this_barn_name().unwrap_or_default(),
        // Read as of Task D4, which is also what writes it. A joining client
        // refuses a peer that is not the house, so answering a hardcoded `false`
        // here — which is what this did while nothing could set the field —
        // would make every join refuse the machine it was pointed at.
        is_ranch_house: this_machine_is_ranch_house(),
    }
}

/// This machine's own barn record, straight off the disk.
///
/// `config::this_barn_name()` names it and the file is read directly, rather
/// than going through `config::load_barns()`: that loader injects a synthetic
/// `local` barn and drops a real `barns/local.yaml`, and neither belongs in an
/// answer to "what does this machine's own record say".
///
/// `None` for a machine that has never been adopted, which is every machine
/// before `ranch init` or `ranch join` — so callers get "unknown", not `false`
/// dressed up as an answer.
pub fn this_machine_barn() -> Option<Barn> {
    let name = crate::config::this_barn_name()?;
    let path = crate::config::barns_dir().join(format!("{}.yaml", name));
    let content = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&content).ok()
}

/// Whether this machine is the Ranch House.
pub fn this_machine_is_ranch_house() -> bool {
    this_machine_barn().and_then(|b| b.is_ranch_house) == Some(true)
}

/// Writes one message as one frame and flushes it.
///
/// The flush is not hygiene. The peer blocks on our newline, and a buffered
/// stdout that is never flushed is a hang rather than a slow reply.
fn send<W: std::io::Write>(out: &mut W, msg: &wire::Message) -> anyhow::Result<()> {
    use anyhow::Context;
    writeln!(out, "{}", wire::encode(msg)?)
        .and_then(|()| out.flush())
        .with_context(|| format!("writing a {} message to the peer", msg.label()))
}

/// One session: greet, then answer whatever the peer sends until it hangs up.
///
/// Split out from [`serve`] so it can be driven over a pair of in-memory
/// buffers. Nothing about the protocol needs a real pipe, and a test that needs
/// one is a test that does not get written.
///
/// Every frame gets an answer. The rule is not politeness, it is that a request
/// without a reply is a client blocked on a read with no timeout anywhere in
/// this codebase to break it — so "not implemented" has to be a *message*, which
/// is what gives [`wire::Message::Error`] its first real sender.
///
/// Which answers end the session, and why:
///
/// - **An incompatible `Hello`** — refuse and stop. We greeted before reading,
///   so the refusal cannot come first; it has to be a message rather than a
///   silent close, or the client cannot tell "incompatible" from "ssh died".
/// - **A `ClaimName`** — answer with the name [`assign_name`] gives it, or with
///   the refusal that function returns, and keep reading either way. Only the
///   house answers at all: the roster is the namespace, and a machine holding a
///   stale copy of it cannot promise a name is free. Nothing is written here —
///   the joining machine is the one that adopts, and the house learns the new
///   barn from the push later in the same session. Its *brand* still arrives a
///   sync late: `join_with` records that on the joiner's own barn record after
///   the plan has been built, so the copy that crosses the wire here predates
///   it. Harmless in the direction that matters — a joiner reaches the house
///   through `push_brand`, not through the roster.
/// - **A `Manifest`** — answer with ours. This is the manifest *exchange*: the
///   client cannot ask for what it has not been told about, and the peer's list
///   is also how it learns which of our entities it is missing entirely.
/// - **A `Want`** — send one `Entity` per key we can resolve, then `Done`, and
///   **keep reading**. The `Done` is the batch terminator: without one the client
///   cannot tell "that was the last entity" from "the next one is slow", and
///   there is no timeout anywhere in this module to rescue it. The session used
///   to end here, on the grounds that "a joining client reads and applies, it
///   does not push, so there is nothing left for it to say". It does push now,
///   and the push can only be computed from the entities this batch delivers —
///   so the terminator ends the *batch* and nothing more. A key that resolves to
///   nothing is skipped rather than refused: it means the entity was deleted
///   between the manifest and the want, and failing the whole session over one
///   racing deletion is worse than the client seeing it next sync.
/// - **An `Entity`** — held, and **not answered**. This is the push direction,
///   and it is the one kind of frame that does not get its own reply: the batch
///   is answered once, at its terminator, exactly as our own entity batch above
///   is. Holding is not writing, so nothing is checked here — see `Commit`.
/// - **A `Commit`** — apply everything held since the last one and answer
///   `Applied` with the keys that actually landed, or `Error` if the batch was
///   refused outright. This is the first thing `serve` has ever been allowed to
///   write, and `client::accept_pushed` owns every rule about it: that only the
///   Ranch House may accept a push at all, the locks, the collision refusal, the
///   one-time backup, and this machine's own bases. The session continues —
///   a push is not a goodbye.
/// - **`Done`** — say `Done` back and stop. A goodbye is not a failure.
/// - **`Error`** — the peer has given up. Stop, and answer *nothing*: both sides
///   answer an unhandled message with `Error`, so replying here is a volley that
///   ends when a pipe fills.
/// - **An undecodable line** — complain and stop. The peer believes it sent a
///   frame we could not read, so the stream is out of step and nothing further
///   down it can be trusted. Saying so while the stream is still writable is the
///   whole difference from a close.
/// - **Anything else, including [`wire::Message::Unknown`]** — `Error` naming
///   it, and *keep reading*. The stream is still in sync and the peer is
///   well-formed; giving up is its decision to make, not ours.
///
/// Exits `Ok` in every one of those cases. A refused session is a protocol
/// outcome the peer can already read off the wire, not a crash — and a non-zero
/// exit would put it in the same bucket as `Permission denied (publickey)` for
/// `transport::PeerExit`, which is the one distinction that struct exists to
/// draw. Same reasoning as `PROBE_CMD`'s `exit 0`: the status means only "did we
/// reach a shell", and the findings ride on stdout.
fn serve_session<R: std::io::BufRead, W: std::io::Write>(
    mut input: R,
    mut out: W,
) -> anyhow::Result<()> {
    send(&mut out, &greeting())?;

    // Loaded at most once, and only if the peer asks for something that needs
    // it. A session that only greets must not walk the entity directories, and
    // a session that asks twice must not walk them twice — the manifest it was
    // given has to describe the entities it is then served.
    let mut ranch: Option<client::Ranch> = None;

    // Entities the peer has pushed but not yet committed. Held rather than
    // written one by one: the batch has to be collision-checked and locked
    // whole, which cannot be done until its last member has arrived.
    let mut pushed: Vec<client::PushedEntity> = Vec::new();

    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(()); // the peer hung up
        }
        let frame = line.trim_end_matches(['\n', '\r']);
        // Not a frame: `writeln!` never emits one, and a blank line carries no
        // payload, so skipping it cannot desynchronize anything.
        if frame.trim().is_empty() {
            continue;
        }

        match wire::decode(frame) {
            Ok(wire::Message::Hello { protocol, .. }) => {
                if let Err(why) = check_protocol(protocol) {
                    send(&mut out, &wire::Message::Error { message: why })?;
                    return Ok(());
                }
            }
            Ok(wire::Message::ClaimName { proposed, brand }) => {
                // Only the house answers this, and the refusal is not a
                // formality: the roster *is* the namespace, and a machine that is
                // not the house holds at best a stale copy of it. Handing out a
                // name from that copy is how two machines end up with one name.
                if !this_machine_is_ranch_house() {
                    send(
                        &mut out,
                        &wire::Message::Error {
                            message: "this machine is not the Ranch House, so it cannot say what \
                                      another machine is called. Names come from the house's \
                                      roster"
                                .to_string(),
                        },
                    )?;
                    continue;
                }
                // Straight off the disk, like every other reader of the roster
                // here: `config::load_barns()` injects a synthetic `local` that
                // would make `local` look taken by something, and drops a real
                // `barns/local.yaml`.
                let roster = manifest::barns_from_disk().items;
                match assign_name(&proposed, &brand, &roster) {
                    Ok(name) => send(&mut out, &wire::Message::NameAssigned { name })?,
                    // Answered and then *kept reading*, like any other refusal
                    // the stream survives: nothing has been written on either
                    // side, the frame boundary is intact, and giving up is the
                    // client's decision to make.
                    Err(why) => send(&mut out, &wire::Message::Error { message: why })?,
                }
            }
            Ok(wire::Message::Manifest { .. }) => {
                // The peer's own list is read and dropped on purpose in this
                // slice: the joining side builds the plan and applies its half,
                // so nothing here acts on what it holds. It is still *answered*,
                // which is the only part the client blocks on.
                let held = match local_ranch(&mut ranch) {
                    Ok(held) => held,
                    Err(why) => {
                        send(&mut out, &wire::Message::Error { message: why })?;
                        return Ok(());
                    }
                };
                match client::manifest_of(held) {
                    Ok(msg) => send(&mut out, &msg)?,
                    Err(e) => {
                        send(
                            &mut out,
                            &wire::Message::Error {
                                message: format!(
                                    "this machine could not describe its own ranch: {:#}",
                                    e
                                ),
                            },
                        )?;
                        return Ok(());
                    }
                }
            }
            Ok(wire::Message::Want { keys }) => {
                let held = match local_ranch(&mut ranch) {
                    Ok(held) => held,
                    Err(why) => {
                        send(&mut out, &wire::Message::Error { message: why })?;
                        return Ok(());
                    }
                };
                for key in &keys {
                    if let Some(msg) = client::entity_for_key(held, key) {
                        send(&mut out, &msg)?;
                    }
                }
                // The batch terminator, and only that. See the doc comment above
                // for why the session no longer ends here.
                send(&mut out, &wire::Message::Done)?;
            }
            // The push direction. Held, not answered, and not written — see the
            // doc comment, and `Commit` below for every rule that applies.
            Ok(wire::Message::Entity { kind, name, yaml }) => {
                pushed.push(client::PushedEntity { kind, name, yaml });
            }
            Ok(wire::Message::Commit) => {
                let batch = std::mem::take(&mut pushed);
                match client::accept_pushed(&batch) {
                    Ok(received) => {
                        if let Some(at) = &received.backup {
                            // The one thing the user on the *other* machine
                            // cannot work out for themselves, and the only way
                            // back if this merge was not what they wanted.
                            eprintln!("backed this ranch up to {} before the first push", at.display());
                        }
                        if let Some(problem) = &received.problem {
                            // stderr, never stdout: stdout is the protocol
                            // stream. `Transport` captures this and quotes it
                            // back to the user, which is how a partial apply's
                            // *reason* reaches the person who asked for it while
                            // the acknowledgement stays one frame carrying one
                            // fact — what landed.
                            eprintln!("a pushed batch was only partly applied: {}", problem);
                        }
                        // The cached ranch now describes the state before the
                        // push. A later `Manifest` or `Want` answered from it
                        // would describe entities this machine no longer holds.
                        ranch = None;
                        send(&mut out, &wire::Message::Applied { keys: received.landed })?;
                    }
                    // Refused outright: nothing was written, which is exactly
                    // what `Error` means everywhere else on this stream. The
                    // session continues — the refusal is the client's to act on.
                    Err(e) => {
                        send(
                            &mut out,
                            &wire::Message::Error {
                                message: format!("{:#}", e),
                            },
                        )?;
                    }
                }
            }
            Ok(wire::Message::Done) => {
                send(&mut out, &wire::Message::Done)?;
                return Ok(());
            }
            Ok(wire::Message::Error { message }) => {
                // stderr, never stdout: stdout is the protocol stream, and ssh
                // keeps the two apart so this reaches the user's terminal
                // without corrupting the peer's parse.
                eprintln!("the peer ended the session: {}", message);
                return Ok(());
            }
            Ok(wire::Message::Unknown) => {
                let tag = wire::tag_of(frame).unwrap_or_else(|| "(unnamed)".to_string());
                send(
                    &mut out,
                    &wire::Message::Error {
                        message: format!(
                            "this machine speaks ranch protocol {} and has no handler for a \
                             message of type {:?}",
                            PROTOCOL_VERSION, tag
                        ),
                    },
                )?;
            }
            Ok(other) => {
                send(
                    &mut out,
                    &wire::Message::Error {
                        message: format!(
                            "`yeehaw ranch serve` on this machine does not answer {} yet",
                            other.label()
                        ),
                    },
                )?;
            }
            Err(e) => {
                send(
                    &mut out,
                    &wire::Message::Error {
                        message: format!(
                            "that was not a protocol frame, so this session is out of step: {}",
                            e
                        ),
                    },
                )?;
                return Ok(());
            }
        }
    }
}

/// This machine's entities, loaded at most once per session.
///
/// A load failure comes back as the *words* for a [`wire::Message::Error`]
/// rather than as an `Err`. `serve` is talking to a peer that is blocked on a
/// reply: bailing here would exit non-zero with the explanation on stderr, where
/// ssh keeps it clear of the protocol stream and the client can report nothing
/// better than "the peer died".
fn local_ranch(held: &mut Option<client::Ranch>) -> std::result::Result<&client::Ranch, String> {
    if held.is_none() {
        match client::load_local() {
            Ok(loaded) => *held = Some(loaded),
            Err(e) => return Err(format!("this machine cannot read its own ranch: {:#}", e)),
        }
    }
    Ok(held.as_ref().expect("just filled"))
}

// ============================================================================
// The naming authority
// ============================================================================

/// What the Ranch House calls a machine that claims `proposed` and brands itself
/// `brand`, given the house's roster — or the words for the refusal.
///
/// # The brand is the identity; the name is a label the ranch assigns
///
/// A hostname is not a ranch-wide name. Two Raspberry Pis are both `pi`, and a
/// machine that names *itself* collides with the first one the moment the second
/// joins — on a roster where names are also filenames, that collision silently
/// writes one machine's barn record over another's. The public half of a brand,
/// on the other hand, is unique per machine, is generated on the machine it
/// belongs to, and already travels as `Barn.brand`. So it is the discriminator,
/// and the name is only what the user reads.
///
/// # The policy, in the order the branches have to be taken
///
/// 1. **A barn already carries this brand** → that barn's name, whatever was
///    proposed. This branch is first for a reason: it is what makes a re-join
///    idempotent, and it has to win even when the proposed name now *clashes* —
///    a Pi re-joining proposes `pi`, and `pi` is taken by itself.
/// 2. **Brand unknown, the proposed name free** → the proposed name.
/// 3. **Brand unknown, the proposed name taken by some other barn** → refused,
///    naming what is taken.
///
/// Deliberately no auto-suffix. A machine called `pi-2` that the user never
/// chose is worse than an interruption that asks them to choose; interrupting is
/// recoverable and a name, once adopted, is written into every livestock record
/// on that machine. If that trade is ever revisited, this function is the one
/// place it lives.
///
/// Returns the words for a [`wire::Message::Error`] rather than an
/// `anyhow::Error`: the caller is answering a peer that is blocked on a reply,
/// where a refusal has to be a *message*.
///
/// Filename safety is not re-checked here. A name that cannot be a filename is
/// refused by `config::validate_name` when the joining machine's adoption tries
/// to write `barns/<name>.yaml`, and that rule has exactly one copy.
pub fn assign_name(
    proposed: &str,
    brand: &str,
    barns: &[Barn],
) -> std::result::Result<String, String> {
    // No brand, no identity. Everything below rests on the brand being the thing
    // that survives a rename and distinguishes two machines with one hostname;
    // assigning a name to a claim that carries none would hand out exactly the
    // un-idempotent enrollment this exists to prevent.
    let Some(claimed) = brand_key(brand) else {
        return Err(
            "the joining machine sent no usable brand, and a brand is how this ranch tells one \
             machine from another. Run `yeehaw ranch join` again on a build that brands itself"
                .to_string(),
        );
    };

    if let Some(known) = barns.iter().find(|b| {
        b.brand.as_deref().and_then(brand_key).is_some_and(|k| k == claimed)
    }) {
        return Ok(known.name.clone());
    }

    let proposed = proposed.trim();
    if proposed.is_empty() {
        return Err(
            "the joining machine proposed no name for itself. Pass one: \
             `yeehaw ranch join <target> --as <name>`"
                .to_string(),
        );
    }
    // `local` is synthetic — injected into every listing and never persisted — so
    // a machine adopted under it would point every one of its livestock at a name
    // that still means "whichever machine reads this". `migrate::adopt_this_machine`
    // refuses it on the joining side too; saying so here costs one frame instead
    // of a failed adoption.
    if proposed == crate::config::LOCAL_BARN_NAME {
        return Err(format!(
            "'{}' is reserved for whichever machine is reading a file, so it cannot name one. \
             Pass a real name: `yeehaw ranch join <target> --as <name>`",
            crate::config::LOCAL_BARN_NAME
        ));
    }

    match barns.iter().find(|b| b.name == proposed) {
        Some(taken) => Err(format!(
            "this ranch already has a barn called '{}'{}, and its brand is not yours — so that \
             name is some other machine's. Pass a different one: \
             `yeehaw ranch join <target> --as <name>`",
            taken.name,
            match taken.host.as_deref() {
                Some(host) => format!(" ({})", host),
                None => String::new(),
            }
        )),
        None => Ok(proposed.to_string()),
    }
}

/// The part of an ssh public key that identifies it: `(type, base64)`, with the
/// comment dropped.
///
/// The comment is not identity. `brand::ensure_brand` mints it once as
/// `yeehaw-ranch-<barn>` and never re-mints, so a barn that was renamed carries a
/// comment naming its old name — and comparing whole lines would fail to
/// recognize that machine in precisely the case this lookup exists for.
///
/// `None` for anything that is not two or more whitespace-separated fields, so a
/// blank or truncated brand can never compare equal to another one.
fn brand_key(brand: &str) -> Option<(&str, &str)> {
    let mut fields = brand.split_whitespace();
    let kind = fields.next()?;
    let material = fields.next()?;
    if kind.is_empty() || material.is_empty() {
        return None;
    }
    Some((kind, material))
}

// ============================================================================
// D4 — `ranch init`
// ============================================================================

/// What [`init`] changed.
#[derive(Debug, Clone, PartialEq)]
pub struct InitReport {
    /// The name this machine is now known by across the ranch.
    pub barn: String,
    /// This machine's public brand.
    pub brand: String,
    pub adoption: crate::migrate::AdoptionReport,
    /// Entities that were stamped and had a base recorded.
    pub stamped: usize,
}

/// Makes this machine the Ranch House.
///
/// # The order is load-bearing
///
/// 1. **Refuse if the ranch already has a house.** Two houses is not a state the
///    merge has a tie-break for — `Side` names one house, and both machines
///    claiming it means each adopts the other's uuids and neither converges.
/// 2. **[`crate::migrate::adopt_this_machine`]**, which persists this machine as
///    a real barn and rewrites every machine-relative livestock. Its refusals
///    are surfaced, not summarised: an unparseable project file and "that name
///    is already some other machine" are the only guidance the user gets, and
///    both name the thing to go and fix.
/// 3. **The brand**, before anything claims to be a house: a Ranch House with no
///    keypair cannot push a managed `authorized_keys` block anywhere, so it is
///    the thing most worth having failed *before* the marker goes down.
/// 4. **The house marker on this machine's own barn**, with its base recorded in
///    the same step.
/// 5. **Every other entity stamped**, with a base for each.
///
/// # What each failure point leaves behind
///
/// Every step is idempotent, so the remedy for all of them is to run `ranch
/// init` again. `adopt_this_machine` already writes `this_barn` before its
/// livestock sweep for exactly this reason, and this follows that precedent: the
/// record that *defines* this machine is written, with its base, before the bulk
/// sweep that may fail partway.
///
/// - **Before step 2 finishes** — whatever `adopt_this_machine` guarantees:
///   either nothing, or a barn record plus `this_barn` plus some rewritten
///   projects. No brand, no house marker; nothing else on the ranch believes a
///   house exists.
/// - **At step 3** — this machine is a barn and nothing is a house. A brand may
///   exist (generation is idempotent and a re-run keeps it).
/// - **At step 4** — a brand exists, unused. Still no house.
/// - **Partway through step 5** — this machine *is* the house, correctly marked
///   and based, and some entities are stamped with bases while the rest are not.
///   That is a coherent ranch, not a broken one: an unstamped entity is the
///   normal state of everything on a ranch that has never synced, it gets an id
///   the next time it is written, and a first join matches it by name. The only
///   cost is that name-matching stays the mechanism for those entities until one
///   of them is stamped.
pub fn init(name: Option<String>) -> Result<InitReport> {
    // Barns off the disk, never `config::load_barns()`: that injects a synthetic
    // `local` which could never be a house, and drops a real `barns/local.yaml`,
    // which on the actual ranch exists and does not parse.
    let barns = manifest::barns_from_disk();
    if let Some(house) = barns.items.iter().find(|b| b.is_ranch_house == Some(true)) {
        anyhow::bail!(
            "this ranch already has a Ranch House: '{}'. Two houses is not a state a sync can \
             resolve — each machine would adopt the other's uuids and neither would converge. \
             To move the house, clear `is_ranch_house` on barns/{}.yaml first",
            house.name,
            house.name
        );
    }

    let name = match name {
        Some(name) => name,
        None => this_machine_default_name()?,
    };

    // Surfaced whole. `{:#}` at the print site renders the chain, so the
    // refusal's own words — the unparseable project's path, or the barn that
    // turns out to be another machine — reach the user intact rather than as
    // "init failed".
    let adoption = crate::migrate::adopt_this_machine(&name)
        .context("this machine could not be adopted as a barn, so there is no house to mark")?;

    let brand = brand::ensure_brand(&name)
        .context("this machine has no brand, and a Ranch House with no keypair cannot push a \
                  key to any barn")?;

    // The record that defines this machine, written with its base before the
    // sweep that may fail partway. Read back off the disk rather than taken from
    // `adoption`: `adopt_this_machine` may have found an existing hostless barn
    // rather than creating one, and that record's critters and addresses must
    // survive.
    let mut house = load_barn_from_disk(&name)?.ok_or_else(|| {
        anyhow::anyhow!(
            "this machine was adopted as '{}' but barns/{}.yaml is not there to mark",
            name,
            name
        )
    })?;
    house.is_ranch_house = Some(true);
    // Syncing with itself is the one relationship that is never in question.
    house.synced = Some(true);
    house.brand = Some(brand.clone());
    {
        let _guard = crate::store::lock_entity(&crate::config::barns_dir(), &name)?;
        base::accept("barn", &mut house, |b| crate::config::save_barn(b))?;
    }

    // One entity at a time, each under its own lock, dropped before the next is
    // taken. A sync has to hold the whole batch — half a merge applied while
    // another writer changes the rest is the hazard `base::accept` documents —
    // but nothing here is a merge: each entity is stamped from its own contents,
    // so there is no cross-entity invariant to protect and no reason to hold
    // thirty-six locks at once.
    let mut stamped = 1; // the house barn above
    let ranch = client::load_local()?;

    for mut project in ranch.projects {
        let at = project.name.clone();
        stamp_one("project", &at, &mut project, &crate::config::projects_dir(), |p| {
            crate::config::save_project(p)
        })?;
        stamped += 1;
    }
    for mut barn in ranch.barns.into_iter().filter(|b| b.name != name) {
        let at = barn.name.clone();
        stamp_one("barn", &at, &mut barn, &crate::config::barns_dir(), |b| {
            crate::config::save_barn(b)
        })?;
        stamped += 1;
    }
    for mut worm in ranch.worms {
        let at = worm.name.clone();
        stamp_one("worm", &at, &mut worm, &crate::config::worms_dir(), |w| {
            crate::config::save_worm(w)
        })?;
        stamped += 1;
    }
    for mut trail in ranch.trails {
        let at = trail.name.clone();
        stamp_one("trail", &at, &mut trail, &crate::config::trails_dir(), |t| {
            crate::config::save_trail(t)
        })?;
        stamped += 1;
    }
    for mut rh in ranch.ranchhands {
        let at = rh.name.clone();
        stamp_one("ranchhand", &at, &mut rh, &crate::config::ranchhands_dir(), |r| {
            crate::config::save_ranchhand(r)
        })?;
        stamped += 1;
    }

    Ok(InitReport { barn: name, brand, adoption, stamped })
}

/// One entity stamped and based, under its own lock.
///
/// `name` is the entity's own name, which is both the lock key and the filename
/// its `save` will write — so the lock and the write cannot disagree about which
/// entity is in hand.
fn stamp_one<T>(
    kind: &str,
    name: &str,
    entity: &mut T,
    dir: &std::path::Path,
    save: impl FnOnce(&mut T) -> Result<()>,
) -> Result<()>
where
    T: serde::Serialize + crate::types::Identified,
{
    let _guard = crate::store::lock_entity(dir, name)?;
    base::accept(kind, entity, save)
        .with_context(|| format!("failed to stamp {} '{}'", kind, name))
}

/// The barn record at `barns/<name>.yaml`, parsed, or `None` if there is no such
/// file.
///
/// An unparseable file is an error rather than a `None`: treating it as absent is
/// how a write lands on top of a barn that merely failed to read.
fn load_barn_from_disk(name: &str) -> Result<Option<Barn>> {
    let path = crate::config::barns_dir().join(format!("{}.yaml", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(
            serde_yaml::from_str(&content)
                .with_context(|| format!("barn file {} does not parse", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// What to call this machine when the user did not say.
///
/// `hostname -s`, shelled out the same way this codebase shells out to `ssh`,
/// `kubectl` and `crontab` — there is no hostname in `std` and no crate for it
/// in this tree. Lowercased, because a barn name is typed at a shell and spelled
/// into livestock records by hand, and `Cams-iMac` is not what anybody types.
///
/// Refuses rather than guessing when the command says nothing useful: a barn
/// called `""` or `localhost` is worse than being asked for a name.
fn this_machine_default_name() -> Result<String> {
    let out = std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .context("could not run `hostname` to name this machine; pass a name: \
                  `yeehaw ranch init <name>`")?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    if !out.status.success() || name.is_empty() || name == "localhost" {
        anyhow::bail!(
            "could not work out a name for this machine from `hostname`; pass one: \
             `yeehaw ranch init <name>`"
        );
    }
    Ok(name)
}

// ============================================================================
// D5 — `ranch join`
// ============================================================================

/// The three things `join` does to the world outside this process, hoisted so a
/// test can drive the whole verb without an ssh, a terminal, or a barn.
///
/// Not a trait: there is exactly one production implementation and exactly one
/// test implementation, and a trait would put the three of them in three places
/// instead of one.
pub struct JoinIo<'a> {
    /// Builds the command that runs `yeehaw ranch serve` on the far side.
    /// Production is `ssh::command`; the transport takes any `Command`, which is
    /// what lets a test point it at a local child.
    pub spawn: &'a mut dyn FnMut(&Barn) -> Result<std::process::Command>,
    /// Shown the rendered plan, answers whether to apply it.
    pub confirm: &'a mut dyn FnMut(&str) -> Result<bool>,
    /// Rewrites the managed block of the target's `authorized_keys`.
    pub push_brand: &'a mut dyn FnMut(&Barn, &[String]) -> Result<()>,
}

/// What [`join_with`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinOutcome {
    /// The name the peer gave for itself in its `Hello`.
    pub peer: String,
    /// What this machine is called on the ranch it just joined — the name the
    /// house assigned, never the peer's own name.
    pub barn: String,
    /// What adopting this machine as [`JoinOutcome::barn`] changed, or `None`
    /// when it was already adopted under that name and nothing had to be
    /// rewritten.
    pub adopted: Option<crate::migrate::AdoptionReport>,
    /// The plan, as the user was shown it.
    pub plan_text: String,
    /// `None` when the user declined. Nothing was written in that case.
    pub applied: Option<client::Applied>,
    /// How many outgoing entities the house confirmed it wrote. Not the same as
    /// how many were sent — see [`JoinOutcome::warnings`] for a shortfall — and
    /// not the same as `applied.based`, which skips an entity with no uuid for
    /// this machine to file a base under.
    pub pushed: usize,
    /// Where `~/.yeehaw` was copied before the first write, if this was the
    /// first join.
    pub backup: Option<std::path::PathBuf>,
    pub brand_pushed: bool,
    /// Things that went wrong *after* the plan was applied, and so could not be
    /// turned into a refusal. The apply already happened; a failed key push is a
    /// thing to retry, not a reason to pretend the sync did not land.
    pub warnings: Vec<String>,
}

/// Joins the ranch at `target`, applying the incoming half on confirmation.
///
/// `target` is a configured barn's name or a bare `[user@]host[:port]` — a
/// machine being joined to is frequently one this ranch has no record of yet,
/// which is the whole point of joining it.
///
/// `name` is what this machine *proposes* to be called; the house decides. See
/// [`join_with`] for that exchange and [`assign_name`] for the policy.
pub fn join(target: &str, name: Option<String>) -> Result<JoinOutcome> {
    let mut spawn = |barn: &Barn| {
        crate::ssh::command(
            barn,
            // `bash -lc`, for the same reason the probe, `connect` and
            // `remote_grid` all use it: sshd's non-interactive shell has neither
            // Homebrew nor `~/.local/bin` on PATH, and `~/.local/bin` is exactly
            // where `install.sh` puts yeehaw. Without this a join against a
            // freshly installed barn fails with `yeehaw: command not found`,
            // which reads as "the peer said nothing at all".
            //
            // The inner command holds no `$` and no double quote, so it nests as
            // one argv element with no further escaping — the same property
            // `ssh::PROBE_CMD` depends on.
            "bash -lc \"yeehaw ranch serve\"",
            // `batch`: a sync must fail rather than sit on a password prompt
            // nobody is watching. No `tty` — the far side speaks a protocol on
            // stdout and a pty would put the terminal's own bytes in it.
            crate::ssh::Opts { batch: true, ..Default::default() },
        )
    };
    let mut confirm = |plan: &str| -> Result<bool> {
        println!("{}", plan);
        // Both halves, in one question. The human is at the joining machine and
        // the house is unattended, so there is nobody on the far side to ask —
        // this answer is the only consent the push will ever get, and a prompt
        // naming only the incoming half would be collecting it under false
        // pretences.
        ask_yes_no("Apply this plan — write the incoming half here and push the outgoing half?")
    };
    let mut push = |barn: &Barn, keys: &[String]| brand::push_to_barn(barn, keys);

    join_with(
        target,
        name,
        JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
    )
}

/// [`join`] with its three outside-world effects injected. See [`JoinIo`].
///
/// # The naming step, and why the adoption sits exactly where it does
///
/// A join is an *enrollment*: this machine stops being "whichever machine is
/// reading this file" and becomes a named barn. Until that has happened its
/// livestock say `barn: null`, `merge::livestock_key` collapses that to `""`, and
/// its `web` and the house's `web` key the same — two different machines'
/// deployments merged as though they were one, with the house's `path` winning.
/// So the adoption is not a side errand of a join; it is a precondition of the
/// merge, and `migrate::adopt_this_machine` is the thing that performs it.
///
/// The name comes from the **house**, not from here: see [`assign_name`]. This
/// machine only proposes one — `--as`, else the name it already has, else
/// `hostname -s`.
///
/// That fixes the ordering, which is otherwise a genuine choice:
///
/// - **Not before the network work.** The adoption rewrites every project's
///   livestock, and the name it would write is not knowable here — the roster is
///   on the house. Guessing locally is what put two Raspberry Pis on one name.
/// - **Not after the plan is shown.** The plan is built from `client::load_local`
///   and keyed by `(barn, name)`, so a plan computed before the adoption is a
///   plan of pre-adoption keys: the user would approve one merge and get
///   another. Worse, it is precisely the merge that fuses the two `web`s.
/// - **So: after `NameAssigned`, before `client::load_local`.** Nothing local is
///   rewritten until the house has confirmed the name, and nothing is merged
///   from pre-adoption data.
///
/// The cost of that, stated plainly: a user who declines the plan is still left
/// adopted under the name the house gave. That is the right half to keep — it is
/// idempotent, it is what every later join and sync needs, and it wrote no
/// entity the user said no to.
///
/// # The push goes before the pull
///
/// Reading has to come first — the plan cannot be computed without the peer's
/// entities, so the `Want` batch necessarily precedes everything. The choice is
/// about the *writes*, and they go house-first: the outgoing half is pushed and
/// acknowledged before `client::apply` touches this machine's store.
///
/// The two halves are not one transaction and no pair of machines can make them
/// one, so the question is only which partial outcome the user is left holding.
///
/// - **Push first.** A failure between the halves leaves the house holding our
///   entities and this machine **byte for byte as it was**. Re-running `join`
///   re-offers the incoming half and finds the outgoing half already there and
///   identical, so it does nothing twice. The recovery is "run it again".
/// - **Pull first.** The same failure leaves this machine's store rewritten —
///   uuids adopted, entities merged — while the house has never heard of ours.
///   Re-running converges too, but the user's own machine changed before the
///   unattended one received anything, and the only way back to the state they
///   consented from is the backup directory.
///
/// The confirmation seals it. One y/n covers both halves (see [`join`]), the
/// human is at this machine, and the house is unattended — so if the house
/// refuses, the right outcome is that *nothing* happened anywhere, which only
/// push-first can deliver.
pub fn join_with(target: &str, name: Option<String>, io: JoinIo) -> Result<JoinOutcome> {
    if this_machine_is_ranch_house() {
        anyhow::bail!(
            "this machine is the Ranch House, so it has nothing to join. A house enrolls the \
             machines that join it; it does not join one of them"
        );
    }

    let barn = resolve_target(target)?;
    let mut peer = transport::Transport::spawn((io.spawn)(&barn)?)
        .with_context(|| format!("failed to start `yeehaw ranch serve` on '{}'", target))?;

    // We were not launched, so the peer speaks first.
    let (peer_name, peer_is_house) = match peer.recv()? {
        Some(wire::Message::Hello { protocol, barn, is_ranch_house }) => {
            if let Err(why) = check_protocol(protocol) {
                // Said to the peer as well as to the user: it is blocked on a
                // reply, and a close it cannot explain is the failure mode the
                // whole handshake exists to avoid.
                let _ = peer.send(&wire::Message::Error { message: why.clone() });
                let _ = peer.finish();
                anyhow::bail!("{}", why);
            }
            (barn, is_ranch_house)
        }
        Some(wire::Message::Error { message }) => {
            let _ = peer.finish();
            anyhow::bail!("'{}' refused the session: {}", target, message);
        }
        Some(other) => {
            let _ = peer.finish();
            anyhow::bail!(
                "'{}' opened with {} instead of a greeting, so this is not a ranch peer",
                target,
                other.label()
            );
        }
        None => {
            let exit = peer.finish()?;
            anyhow::bail!(
                "'{}' said nothing at all (exit {:?}){}",
                target,
                exit.status.code(),
                if exit.stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", exit.stderr.trim())
                }
            );
        }
    };

    if !peer_is_house {
        let _ = peer.finish();
        anyhow::bail!(
            "'{}' is not a Ranch House{}. Run `yeehaw ranch init` there first — a join adopts \
             the house's uuids, and there is no house to adopt them from",
            target,
            if peer_name.is_empty() {
                " and has not even been adopted as a barn".to_string()
            } else {
                format!(" (it calls itself '{}')", peer_name)
            }
        );
    }

    peer.send(&greeting())?;

    // The brand before anything else, for two reasons that happen to coincide: a
    // join that merged every entity and then failed to mint a keypair has left
    // the user a ranch they still cannot reach, *and* the claim below is keyed by
    // the brand, so it cannot be sent without one.
    //
    // The comment on the key is the *proposed* name, because the authoritative
    // name does not exist until the house answers a message this key has to be
    // inside. That is only ever a label — `ensure_brand` never re-mints, and a
    // renamed barn already keeps the comment it was minted with.
    let proposed = match name {
        Some(name) => name,
        // What this machine is already called, and only then `hostname -s`.
        // Never the peer's name: a MacBook joining a Pi is not a Pi, and a
        // machine that took the house's name for itself would collide with the
        // house's own record the moment either of them saved it.
        None => match crate::config::this_barn_name() {
            Some(current) => current,
            None => this_machine_default_name()?,
        },
    };
    let our_brand = brand::ensure_brand(&proposed)?;

    // The naming step. The house owns the roster, so the house owns the
    // namespace; this is a claim, not an announcement.
    peer.send(&wire::Message::ClaimName {
        proposed: proposed.clone(),
        brand: our_brand.clone(),
    })?;
    let our_name = match peer.recv()? {
        Some(wire::Message::NameAssigned { name }) => name,
        // The refusal is the house's own words — a name already taken, and which
        // machine has it. Nothing local has been written yet, which is the whole
        // reason the claim comes before the adoption.
        Some(wire::Message::Error { message }) => {
            let _ = peer.finish();
            anyhow::bail!(
                "the Ranch House at '{}' would not enroll this machine as '{}': {}",
                target,
                proposed,
                message
            );
        }
        other => {
            let _ = peer.finish();
            anyhow::bail!(
                "expected '{}' to name this machine, got {:?}. A join cannot merge anything \
                 until this machine has a name of its own — without one its livestock say \
                 `barn: null`, which every machine reads as its own",
                target,
                other.map(|m| m.label())
            );
        }
    };

    // THE ADOPTION. Here, and not a line later: everything below reads this
    // machine's entities, and read before this they are keyed by a barn that does
    // not exist. See this function's doc comment for the full ordering argument.
    //
    // `adopt_this_machine`'s refusals are surfaced whole, not summarised — an
    // unparseable project's path, or a barn that turns out to be another machine
    // — because they name the thing the user has to go and fix, and `{:#}` at the
    // print site renders the chain.
    let adopted = adopt_as(&our_name)?;

    let ours = client::load_local()?;
    peer.send(&client::manifest_of(&ours)?)?;

    // Read the peer's manifest *before* sending the want. Strict alternation
    // rather than pipelining: both sides are blocking writers on a pipe with no
    // timeout, and two unread manifests in flight is a deadlock on any ranch big
    // enough to fill a pipe buffer.
    let (their_entries, their_tombstones) = match peer.recv()? {
        Some(wire::Message::Manifest { entries, tombstones }) => (entries, tombstones),
        Some(wire::Message::Error { message }) => {
            let _ = peer.finish();
            anyhow::bail!("'{}' could not describe its ranch: {}", target, message);
        }
        other => {
            let _ = peer.finish();
            anyhow::bail!("expected a manifest from '{}', got {:?}", target, other.map(|m| m.label()));
        }
    };

    peer.send(&wire::Message::Want { keys: client::want_keys(&their_entries) })?;

    let mut theirs = client::Ranch { tombstones: their_tombstones, ..Default::default() };
    loop {
        match peer.recv()? {
            Some(wire::Message::Entity { kind, name, yaml }) => {
                client::absorb_entity(&mut theirs, &kind, &name, &yaml)?;
            }
            // The batch terminator, and the end of the session.
            Some(wire::Message::Done) | None => break,
            Some(wire::Message::Error { message }) => {
                let _ = peer.finish();
                anyhow::bail!("'{}' stopped sending entities: {}", target, message);
            }
            Some(other) => {
                let _ = peer.finish();
                anyhow::bail!(
                    "'{}' sent {} in the middle of an entity batch",
                    target,
                    other.label()
                );
            }
        }
    }

    // `Side::Remote`: the target is the Ranch House, so its uuid is the one
    // adopted and its value wins a field both machines changed. On a first join
    // that adoption is the entire mechanism — `merge::pair_up` matches by uuid
    // first and falls back to name, so the names line the two ranches up exactly
    // once and every sync after that is matched by id.
    let plan = client::build_plan(&ours, &theirs, merge::Side::Remote)?;
    let plan_text = client::render_plan(&plan, &peer_name);

    let mut outcome = JoinOutcome {
        peer: peer_name.clone(),
        barn: our_name.clone(),
        adopted,
        plan_text: plan_text.clone(),
        applied: None,
        pushed: 0,
        backup: None,
        brand_pushed: false,
        warnings: vec![],
    };

    if !(io.confirm)(&plan_text)? {
        let _ = peer.finish();
        return Ok(outcome);
    }

    // Before the backup, so a plan that is going to be refused does not leave a
    // gigabyte of copy behind. `client::apply` checks this again immediately
    // before it takes a lock; the duplication is deliberate — that one is the
    // invariant, this one is the ordering.
    let collisions = plan.lock_target_collisions();
    if !collisions.is_empty() {
        let _ = peer.finish();
        anyhow::bail!(
            "refusing to apply: {} filename(s) would be written by two different entities, \
             which writes one on top of the other. Rename one side and join again.\n{}",
            collisions.len(),
            collisions
                .iter()
                .map(|(kind, name)| format!("  {} {}", kind, name))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // The undo, before the first write. This is the run that merges months of
    // accumulated config on both machines; there has to be a way back. The
    // house takes one of its own when the push below reaches it.
    outcome.backup = client::backup_once()?;

    // THE PUSH, and it goes first — before a single byte of this machine's own
    // store is touched. See this function's doc comment for the ordering
    // argument; the short form is that a failure here leaves local state
    // untouched and `join` can simply be run again, whereas the same failure
    // after the incoming half had been applied would leave the user's own
    // machine rewritten and the house none the wiser, recoverable only through
    // the backup directory.
    let sending = client::pushable(&plan);
    let mut landed: Vec<String> = Vec::new();
    if !sending.is_empty() {
        for change in &sending {
            peer.send(&client::push_message(change)?)?;
        }
        // The terminator is what authorizes the write. An `Entity` on its own
        // never does — so a connection that dies mid-batch writes nothing, and
        // this machine, having heard no acknowledgement, records no base.
        peer.send(&wire::Message::Commit)?;
        match peer.recv()? {
            Some(wire::Message::Applied { keys }) => landed = keys,
            Some(wire::Message::Error { message }) => {
                let _ = peer.finish();
                anyhow::bail!(
                    "'{}' refused the push, so nothing has been written on either machine: {}",
                    target,
                    message
                );
            }
            other => {
                let said = peer.stderr();
                let _ = peer.finish();
                anyhow::bail!(
                    "expected '{}' to say what it applied, got {:?}. Nothing has been written \
                     here, so re-running the join is safe and will offer the same plan{}",
                    target,
                    other.map(|m| m.label()),
                    if said.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" (the peer's stderr said: {})", said.trim())
                    }
                );
            }
        }
        outcome.pushed = landed.len();
        if landed.len() < sending.len() {
            // Not a refusal, and so not a reason to abandon the incoming half:
            // the house took the batch and wrote part of it, and the pull the
            // user approved is independent work. What did not land has no base,
            // so it is offered again. Named anyway, because a sync that silently
            // moved less than the plan it was approved from is the thing a user
            // would otherwise only find out about later.
            outcome.warnings.push(format!(
                "{} of {} pushed entities did not land on '{}', so they keep no sync base and \
                 will be offered again on the next sync{}",
                sending.len() - landed.len(),
                sending.len(),
                target,
                if peer.stderr().trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", peer.stderr().trim())
                }
            ));
        }
    }

    // And now this machine's half, plus a sync base for every outgoing entity
    // the house named back — and only those.
    outcome.applied = Some(client::apply(&plan, &landed)?);

    // After `apply` has dropped its locks: this takes the same barn lock, and
    // `fs2` locks are neither re-entrant nor timed, so doing it inside the batch
    // would hang the thread against itself with no error and no way out.
    //
    // No base is recorded. The brand is a local edit this machine has not sent
    // anywhere, and a base that claimed otherwise would make the next sync read
    // it as already-synced and never offer it — the one drift direction
    // `base.rs` calls unrecoverable.
    if let Err(e) = record_our_brand(&our_brand) {
        outcome.warnings.push(format!(
            "this machine's brand was not recorded on its own barn record, so the house will \
             not learn it on the next sync: {:#}",
            e
        ));
    }

    // Every brand on the ranch, not only ours: the managed block is rewritten
    // whole, so writing ours alone would delete every other machine's key from
    // the house. The barn list is re-read after the apply, which is when the
    // house's own brands arrived.
    let brands = client::known_brands(Some(&our_brand), &client::load_local()?.barns);
    match (io.push_brand)(&barn, &brands) {
        Ok(()) => outcome.brand_pushed = true,
        Err(e) => outcome.warnings.push(format!(
            "the plan was applied, but this machine's brand was not installed on '{}', so ssh \
             to it still needs whatever key it needed before: {:#}",
            target, e
        )),
    }

    let exit = peer.finish()?;
    if !exit.status.success() {
        outcome.warnings.push(format!(
            "the peer exited {:?}{}",
            exit.status.code(),
            if exit.stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", exit.stderr.trim())
            }
        ));
    }

    Ok(outcome)
}

/// Makes this machine a barn called `name`, and reports what that changed.
///
/// `None` means it already *was* that barn and nothing had to be rewritten. The
/// check is for the recorded name rather than a blind re-run because
/// `adopt_this_machine` under the same name is a no-op that still returns a
/// report, and reporting a no-op would have `ranch join` announce an enrollment
/// that did not happen. Under a *different* name it is a refusal, which is
/// exactly what should reach the user: the house named this machine something
/// other than what it already calls itself, and renaming a barn is a job neither
/// this nor the migration does.
fn adopt_as(name: &str) -> Result<Option<crate::migrate::AdoptionReport>> {
    if crate::config::this_barn_name().as_deref() == Some(name) {
        return Ok(None);
    }
    let report = crate::migrate::adopt_this_machine(name).with_context(|| {
        format!(
            "this machine could not be adopted as barn '{}', so it has no name of its own — and \
             its livestock would stay machine-relative, which a merge reads as the house's own",
            name
        )
    })?;
    Ok(Some(report))
}

/// Records this machine's public brand on its own barn record.
///
/// A no-op, reported as one, on a machine that has never been adopted: there is
/// no barn record to write it to, and inventing one here would plant a barn the
/// user never named. Since the naming step, [`join_with`] cannot reach this
/// without having adopted first — so on that path the refusal is a guard against
/// a record deleted underneath a running join, not an expected outcome.
fn record_our_brand(brand: &str) -> Result<()> {
    let Some(name) = crate::config::this_barn_name() else {
        anyhow::bail!(
            "this machine has no barn record of its own yet, so there is nowhere to record its \
             brand. Run `yeehaw ranch init <name>` on the Ranch House and re-join, or adopt this \
             machine first"
        );
    };
    let _guard = crate::store::lock_entity(&crate::config::barns_dir(), &name)?;
    let Some(mut barn) = load_barn_from_disk(&name)? else {
        anyhow::bail!("barns/{}.yaml is not there to record a brand on", name);
    };
    barn.brand = Some(brand.to_string());
    barn.synced = Some(true);
    crate::config::save_barn(&mut barn)
}

/// The barn to dial for `target`.
///
/// A configured barn's name wins, because that is where the port, the user and
/// the identity file live. Otherwise `target` is parsed as `[user@]host[:port]`
/// — the machine being joined is usually one this ranch has no record of, which
/// is the point of joining it.
///
/// A bracketed IPv6 literal keeps its brackets and its port; a bare one is taken
/// whole, because `::1:22` cannot be split into a host and a port without
/// guessing which colon the user meant.
fn resolve_target(target: &str) -> Result<Barn> {
    if let Some(barn) = manifest::barns_from_disk().items.into_iter().find(|b| b.name == target) {
        if barn.host.is_none() {
            anyhow::bail!(
                "barn '{}' has no host to reach. That is what this machine's own record looks \
                 like — and a k8s-discovered node's — so there is nothing to ssh to. Give the \
                 target as user@host instead",
                target
            );
        }
        return Ok(barn);
    }

    let (user, rest) = match target.rsplit_once('@') {
        Some((user, rest)) => (Some(user.to_string()), rest),
        None => (None, target),
    };

    let (host, port) = if rest.starts_with('[') {
        match rest.rsplit_once("]:") {
            Some((addr, port)) => (format!("{}]", addr), Some(port)),
            None => (rest.to_string(), None),
        }
    } else if rest.matches(':').count() == 1 {
        let (host, port) = rest.rsplit_once(':').expect("exactly one colon");
        (host.to_string(), Some(port))
    } else {
        (rest.to_string(), None)
    };

    if host.is_empty() {
        anyhow::bail!("'{}' names no host to join", target);
    }
    let port = match port {
        Some(text) => Some(
            text.parse::<u16>()
                .with_context(|| format!("'{}' is not a port number in '{}'", text, target))?,
        ),
        None => None,
    };

    Ok(Barn {
        name: host.clone(),
        host: Some(host),
        user,
        port,
        // Not `Some(false)`: the whole operation is an attempt to reach it, and
        // this record is never saved.
        ..Default::default()
    })
}

/// Reads a y/n from the terminal. Anything that is not a yes is a no, and so is
/// end-of-input.
///
/// Default-no on EOF is the only safe reading: a `join` in a pipeline with no
/// stdin must not apply a plan nobody looked at.
fn ask_yes_no(question: &str) -> Result<bool> {
    use std::io::Write;
    print!("{} [y/N] ", question);
    std::io::stdout().flush().ok();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer)? == 0 {
        println!();
        return Ok(false);
    }
    let answer = answer.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// The remote command must run under a login shell. sshd's non-interactive
    /// shell has neither Homebrew nor `~/.local/bin` on PATH, and `install.sh`
    /// puts yeehaw in `~/.local/bin` — so a raw `yeehaw ranch serve` fails with
    /// `command not found` against every freshly installed barn, surfacing as
    /// "said nothing at all". The probe, `connect` and `remote_grid` all wrap
    /// for this reason; this is the fourth.
    #[test]
    fn the_remote_serve_command_runs_under_a_login_shell() {
        crate::testing::with_temp_ranch(|_| {
        let barn = Barn {
            name: "pi".into(),
            host: Some("pi.local".into()),
            ..Default::default()
        };
        let cmd = crate::ssh::command(
            &barn,
            "bash -lc \"yeehaw ranch serve\"",
            crate::ssh::Opts { batch: true, ..Default::default() },
        )
        .expect("a barn with a host builds a command");

        let remote = cmd
            .get_args()
            .last()
            .expect("ssh is given a remote command")
            .to_string_lossy()
            .to_string();

        assert!(
            remote.starts_with("bash -lc"),
            "the remote command must go through a login shell, got {remote:?}"
        );
        assert!(
            remote.contains("yeehaw ranch serve"),
            "the login shell must still run serve, got {remote:?}"
        );
        });
    }

    #[test]
    fn parses_each_verb() {
        assert_eq!(parse_args(&argv(&["yeehaw", "ranch", "serve"])), RanchCommand::Serve);
        assert_eq!(parse_args(&argv(&["yeehaw", "ranch", "status"])), RanchCommand::Status);
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "init", "imac"])),
            RanchCommand::Init { name: Some("imac".into()) }
        );
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "init"])),
            RanchCommand::Init { name: None }
        );
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "join", "cam@pi"])),
            RanchCommand::Join { target: "cam@pi".into(), name: None }
        );
    }

    /// The bare form still has to work, and both spellings of the override have
    /// to reach the same field — `--as` is the documented one, the positional is
    /// what gets typed by anyone who just used `ranch init <name>`.
    #[test]
    fn a_join_can_propose_what_this_machine_is_called() {
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "join", "cam@pi", "--as", "macbook"])),
            RanchCommand::Join { target: "cam@pi".into(), name: Some("macbook".into()) }
        );
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "join", "cam@pi", "macbook"])),
            RanchCommand::Join { target: "cam@pi".into(), name: Some("macbook".into()) }
        );
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "join", "cam@pi"])),
            RanchCommand::Join { target: "cam@pi".into(), name: None },
            "the bare form takes the hostname, and must keep parsing"
        );
        // A flag this verb does not know is not a name. Claiming it would enroll
        // the machine as `--dry-run` the day that flag is added.
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "join", "cam@pi", "--dry-run"])),
            RanchCommand::Join { target: "cam@pi".into(), name: None }
        );
        // And `--as` with nothing after it is usage, not a guess: this machine's
        // name is the last thing to invent on the user's behalf.
        assert_eq!(
            parse_args(&argv(&["yeehaw", "ranch", "join", "cam@pi", "--as"])),
            RanchCommand::Usage
        );
    }

    #[test]
    fn an_unknown_or_missing_verb_asks_for_usage() {
        assert_eq!(parse_args(&argv(&["yeehaw", "ranch"])), RanchCommand::Usage);
        assert_eq!(parse_args(&argv(&["yeehaw", "ranch", "wat"])), RanchCommand::Usage);
        // `join` without a target cannot be actioned.
        assert_eq!(parse_args(&argv(&["yeehaw", "ranch", "join"])), RanchCommand::Usage);
    }

    // ---- D0: the version check -------------------------------------------

    /// A floor above the ceiling accepts nothing at all, which would read in the
    /// field as "every peer is incompatible" with no hint that the constants are
    /// the bug.
    #[test]
    fn the_compatibility_floor_is_not_above_what_this_build_speaks() {
        assert!(
            MIN_COMPATIBLE_PROTOCOL <= PROTOCOL_VERSION,
            "MIN_COMPATIBLE_PROTOCOL ({}) above PROTOCOL_VERSION ({}) refuses every peer",
            MIN_COMPATIBLE_PROTOCOL,
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn a_peer_inside_the_supported_range_is_accepted() {
        assert!(check_protocol(PROTOCOL_VERSION).is_ok(), "our own version must be speakable");
        assert!(
            check_protocol(MIN_COMPATIBLE_PROTOCOL).is_ok(),
            "the floor is the oldest *supported* version, not the first refused one"
        );
    }

    /// Below the floor is the one direction where refusing is uncontroversial:
    /// the peer is old, we cannot parse it, and the peer cannot be taught.
    #[test]
    fn a_peer_below_the_floor_is_refused_with_both_versions_named() {
        // The floor is 1, so 0 is below it. The guard keeps this meaningful if
        // the floor is ever raised.
        assert!(MIN_COMPATIBLE_PROTOCOL >= 1, "there is no version below the floor to test with");
        let why = check_protocol(MIN_COMPATIBLE_PROTOCOL - 1)
            .expect_err("a peer below the floor cannot be parsed, so it must be refused");
        assert!(
            why.contains(&PROTOCOL_VERSION.to_string()) && why.contains("peer"),
            "the refusal goes to the peer verbatim; it has to say what was expected: {:?}",
            why
        );
    }

    /// The decision with teeth. A peer above our version is, by
    /// `PROTOCOL_VERSION`'s own definition, able to send us bytes we cannot
    /// parse — so this side refuses at the handshake instead of discovering it
    /// mid-apply, and the newer side owns the downgrade.
    #[test]
    fn a_peer_newer_than_this_build_is_refused_rather_than_guessed_at() {
        let why = check_protocol(PROTOCOL_VERSION + 1)
            .expect_err("a newer peer may send variants and fields this build cannot decode");
        assert!(
            why.contains(&(PROTOCOL_VERSION + 1).to_string()),
            "the refusal must name the version the peer declared: {:?}",
            why
        );
    }

    // ---- D0: `serve` answers instead of hanging ---------------------------

    /// Drives one session over in-memory buffers and returns what it said.
    ///
    /// Decoding every line here is half the assertion: `serve`'s stdout is the
    /// protocol stream, so a stray `println!` on this path is a peer that cannot
    /// parse our first message and has no way to say why.
    ///
    /// The temp ranch is not optional — `greeting()` reads `config.this_barn`,
    /// and `config::yeehaw_dir()` panics under `cfg(test)` without one.
    fn session(input: &str) -> Vec<wire::Message> {
        let _ranch = crate::testing::temp_ranch();
        let mut out: Vec<u8> = Vec::new();
        serve_session(std::io::Cursor::new(input.as_bytes().to_vec()), &mut out)
            .expect("a session ends cleanly; a refusal is a message, not an error");
        decode_all(&out)
    }

    /// Every line `serve` wrote, decoded. Decoding is half the assertion: a stray
    /// `println!` on this path is a peer that cannot parse our first message.
    fn decode_all(out: &[u8]) -> Vec<wire::Message> {
        String::from_utf8(out.to_vec())
            .expect("protocol is UTF-8")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| wire::decode(l).unwrap_or_else(|e| panic!("non-protocol line {:?}: {}", l, e)))
            .collect()
    }

    /// `Project` is built exhaustively on purpose — it has no `Default` impl, so a
    /// new field fails to compile here rather than being silently defaulted.
    fn project(name: &str) -> crate::types::Project {
        crate::types::Project {
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

    fn line(msg: &wire::Message) -> String {
        format!("{}\n", wire::encode(msg).unwrap())
    }

    fn hello_at(protocol: u32) -> wire::Message {
        wire::Message::Hello { protocol, barn: "pi".into(), is_ranch_house: true }
    }

    fn error_text(msg: &wire::Message) -> &str {
        match msg {
            wire::Message::Error { message } => message,
            other => panic!("expected an Error, got {:?}", other),
        }
    }

    /// `serve` is launched by the peer, so it speaks first and cannot have read
    /// anything yet. That ordering is why an incompatible peer has to be refused
    /// *after* the greeting rather than instead of it.
    #[test]
    fn serve_greets_before_it_has_read_anything() {
        let said = session("");
        assert_eq!(said.len(), 1, "a peer that says nothing still gets greeted: {:?}", said);
        match &said[0] {
            wire::Message::Hello { protocol, .. } => assert_eq!(*protocol, PROTOCOL_VERSION),
            other => panic!("the first line must be the greeting, got {:?}", other),
        }
    }

    /// The carried-forward gap. A peer declaring a version this build cannot
    /// parse must be told so in a message it can read, and the session must then
    /// end — a silent close leaves the client guessing between "incompatible",
    /// "ssh died" and "yeehaw is not installed".
    #[test]
    fn an_incompatible_peer_is_told_why_and_the_session_ends_cleanly() {
        let said = session(&format!(
            "{}{}",
            line(&hello_at(PROTOCOL_VERSION + 1)),
            // Still queued behind the bad hello. Answering it would mean
            // carrying on a session we just declared unspeakable.
            line(&wire::Message::Manifest { entries: vec![], tombstones: vec![] })
        ));

        assert_eq!(
            said.len(),
            2,
            "greeting, then the refusal, and nothing after it: {:?}",
            said
        );
        let why = error_text(&said[1]);
        assert!(
            why.contains(&(PROTOCOL_VERSION + 1).to_string())
                && why.contains(&PROTOCOL_VERSION.to_string()),
            "the refusal must name both versions so the user knows which machine to upgrade: {:?}",
            why
        );
    }

    #[test]
    fn a_peer_on_this_protocol_is_not_refused() {
        let said = session(&line(&hello_at(PROTOCOL_VERSION)));
        assert_eq!(said.len(), 1, "a compatible hello needs no answer of its own: {:?}", said);
        assert!(
            !matches!(said[0], wire::Message::Error { .. }),
            "a compatible peer must not be refused: {:?}",
            said
        );
    }

    /// The hang. A client sends something and blocks on a reply; a
    /// drain-and-discard loop never writes one and never reaches EOF, because the
    /// client is still holding its end open waiting. An `Error` is a worse answer
    /// than a real one and an infinitely better answer than none.
    ///
    /// DELIBERATE CHANGE, Task D5. This used to be driven with a `Manifest`,
    /// which `serve` now answers with a manifest of its own — that is the
    /// exchange D5 exists to add, and it is pinned by
    /// `a_manifest_is_answered_with_this_machines_own`.
    ///
    /// DELIBERATE CHANGE, the push slice. It was then driven with an `Entity`,
    /// on the grounds that "a joining client reads and applies, it never pushes,
    /// so nothing on this side accepts an entity yet". It does push now: an
    /// `Entity` is held for the `Commit` that authorizes writing it, and is
    /// deliberately the one frame with no reply of its own (see
    /// `a_pushed_batch_is_answered_once_at_its_terminator`). So this moves to
    /// `Applied`, which is genuinely unimplemented here and for a reason that
    /// will not expire — it is the *house's* answer to a commit, and a `serve`
    /// never receives one. The assertion is unchanged: every frame gets an
    /// answer, and the answer names what went unhandled.
    #[test]
    fn a_message_this_build_does_not_implement_is_answered_not_swallowed() {
        let said = session(&format!(
            "{}{}",
            line(&hello_at(PROTOCOL_VERSION)),
            line(&wire::Message::Applied { keys: vec!["project/abc".into()] })
        ));

        assert_eq!(said.len(), 2, "the message must get a reply: {:?}", said);
        assert!(
            error_text(&said[1]).contains("applied"),
            "the error must name what went unanswered: {:?}",
            said[1]
        );
    }

    /// The exchange. A client cannot ask for what it has not been told about, so
    /// a manifest has to be answered with a manifest rather than with an error.
    #[test]
    fn a_manifest_is_answered_with_this_machines_own() {
        let _ranch = crate::testing::temp_ranch();
        crate::config::save_project(&mut project("api")).unwrap();

        // Not `session()`: that establishes its own empty temp ranch, and the
        // point here is what a ranch with something in it answers.
        let input = format!(
            "{}{}",
            line(&hello_at(PROTOCOL_VERSION)),
            line(&wire::Message::Manifest { entries: vec![], tombstones: vec![] })
        );
        let mut out: Vec<u8> = Vec::new();
        serve_session(std::io::Cursor::new(input.into_bytes()), &mut out).unwrap();
        let said = decode_all(&out);

        assert_eq!(said.len(), 2, "greeting then our manifest: {:?}", said);
        match &said[1] {
            wire::Message::Manifest { entries, .. } => {
                assert_eq!(entries.len(), 1, "the ranch has one entity: {:?}", entries);
                assert_eq!(entries[0].name, "api");
                assert_eq!(entries[0].kind, "project");
            }
            other => panic!("a manifest must be answered with a manifest, got {:?}", other),
        }
    }

    /// The entity batch, and its terminator. Without the `Done` the client cannot
    /// tell "that was the last one" from "the next one is slow", and there is no
    /// timeout in this module to break the difference.
    #[test]
    fn a_want_is_answered_with_the_entities_and_then_a_terminator() {
        let _ranch = crate::testing::temp_ranch();
        let mut p = project("api");
        crate::config::save_project(&mut p).unwrap();
        let id = p.id.clone().expect("save stamps");

        let input = format!(
            "{}{}",
            line(&hello_at(PROTOCOL_VERSION)),
            line(&wire::Message::Want {
                keys: vec![format!("project/{}", id), "project/ghost".into()],
            })
        );
        let mut out: Vec<u8> = Vec::new();
        serve_session(std::io::Cursor::new(input.into_bytes()), &mut out).unwrap();
        let said = decode_all(&out);

        assert_eq!(
            said.len(),
            3,
            "greeting, the one entity we hold, then Done — a key that resolves to nothing is \
             skipped, not refused: {:?}",
            said
        );
        match &said[1] {
            wire::Message::Entity { kind, name, yaml } => {
                assert_eq!((kind.as_str(), name.as_str()), ("project", "api"));
                assert!(yaml.contains("name: api"), "{}", yaml);
            }
            other => panic!("expected the entity, got {:?}", other),
        }
        assert_eq!(said[2], wire::Message::Done, "the batch needs a terminator");
    }

    /// The unknown-variant policy, seen from the protocol layer: tolerated by
    /// the decoder so that it can be *named*, and then still refused. A newer
    /// peer learns which message type this build lacks.
    #[test]
    fn a_message_type_from_a_newer_peer_is_named_back_to_it() {
        let said = session(&format!(
            "{}{}\n",
            line(&hello_at(PROTOCOL_VERSION)),
            r#"{"msg":"lease","ttl":30}"#
        ));
        assert_eq!(said.len(), 2, "an unknown message type gets an answer too: {:?}", said);
        assert!(
            error_text(&said[1]).contains("lease"),
            "tolerating an unknown tag is only worth it if the tag is reported: {:?}",
            said[1]
        );
    }

    /// `Done` is the peer saying it is finished. Answering that with an error
    /// would make a clean goodbye look like a failure in the client's logs.
    #[test]
    fn a_polite_goodbye_is_answered_in_kind() {
        let said = session(&format!(
            "{}{}",
            line(&hello_at(PROTOCOL_VERSION)),
            line(&wire::Message::Done)
        ));
        assert_eq!(said.len(), 2, "{:?}", said);
        assert_eq!(said[1], wire::Message::Done, "a goodbye is not an error: {:?}", said[1]);
    }

    /// Both sides answer an unhandled message with `Error`. If `Error` were
    /// itself unhandled, two builds that disagreed would volley errors at each
    /// other until one of the pipes filled.
    #[test]
    fn a_peer_that_gives_up_is_not_answered_with_another_error() {
        let said = session(&format!(
            "{}{}",
            line(&hello_at(PROTOCOL_VERSION)),
            line(&wire::Message::Error { message: "the ranch house refused".into() })
        ));
        assert_eq!(
            said.len(),
            1,
            "an error must not be answered with an error; that is a volley: {:?}",
            said
        );
    }

    /// A line that will not decode means the stream is out of step: the peer
    /// believes it sent a frame we never read. There is nothing useful further
    /// down it, so say so and stop — but *say* so, on the stream, while it is
    /// still writable.
    #[test]
    fn an_undecodable_line_ends_the_session_with_an_explanation() {
        let said = session(&format!(
            "{}not a frame at all\n{}",
            line(&hello_at(PROTOCOL_VERSION)),
            line(&wire::Message::Done)
        ));
        assert_eq!(said.len(), 2, "greeting, complaint, nothing after: {:?}", said);
        assert!(
            matches!(said[1], wire::Message::Error { .. }),
            "a corrupt frame is an error, not a silent skip: {:?}",
            said[1]
        );
    }

    // ---- D4: `ranch init` -------------------------------------------------

    use crate::config;
    use crate::testing;

    #[test]
    fn init_marks_this_machine_as_the_house_stamps_everything_and_bases_it() {
        let _ranch = testing::temp_ranch();
        config::save_project(&mut project("api")).unwrap();
        config::save_barn(&mut Barn {
            name: "pi".into(),
            host: Some("pi.local".into()),
            ..Default::default()
        })
        .unwrap();

        let report = init(Some("imac".into())).expect("a fresh ranch can be initialized");

        assert_eq!(report.barn, "imac");
        assert!(report.brand.starts_with("ssh-ed25519 "), "brand: {:?}", report.brand);
        // imac (created), pi, api.
        assert_eq!(report.stamped, 3, "every entity must be stamped: {:?}", report);

        let house = this_machine_barn().expect("this machine now has a barn record");
        assert_eq!(house.is_ranch_house, Some(true), "the house marker is the point: {:?}", house);
        assert_eq!(house.synced, Some(true), "the house syncs itself: {:?}", house);
        assert_eq!(house.brand.as_deref(), Some(report.brand.as_str()));
        assert!(this_machine_is_ranch_house());

        // Ids exist *before* anybody joins. This is what makes first-join
        // name-matching a one-time event instead of the permanent state.
        for project in config::load_projects() {
            let id = project.id.clone().expect("every entity is stamped");
            assert!(
                base::load::<crate::types::Project>("project", &id).unwrap().is_some(),
                "a stamped entity with no base has no ancestor for its first merge"
            );
        }
        let pi = manifest::barns_from_disk()
            .items
            .into_iter()
            .find(|b| b.name == "pi")
            .expect("pi survives");
        let pi_id = pi.id.clone().expect("every barn is stamped too");
        assert!(base::load::<Barn>("barn", &pi_id).unwrap().is_some());
    }

    /// Two houses is not a state the merge has a tie-break for: `Side` names one,
    /// and both machines claiming it means each adopts the other's uuids.
    #[test]
    fn init_refuses_when_the_ranch_already_has_a_house() {
        let _ranch = testing::temp_ranch();
        config::save_barn(&mut Barn {
            name: "already".into(),
            host: Some("elsewhere".into()),
            is_ranch_house: Some(true),
            ..Default::default()
        })
        .unwrap();

        let why = format!("{:#}", init(Some("imac".into())).expect_err("one house per ranch"));
        assert!(why.contains("already"), "the refusal must name the existing house: {}", why);
        assert!(
            !config::barns_dir().join("imac.yaml").exists(),
            "the refusal must come before anything is written"
        );
    }

    #[test]
    fn init_twice_refuses_the_second_time_rather_than_re_initializing() {
        let _ranch = testing::temp_ranch();
        init(Some("imac".into())).unwrap();
        let why = format!("{:#}", init(Some("imac".into())).expect_err("already a house"));
        assert!(why.contains("imac"), "{}", why);
    }

    /// `adopt_this_machine` refuses to adopt a name that belongs to another
    /// machine, because doing so moves every livestock on this machine onto that
    /// one. That refusal is the user's only guidance, so it has to survive.
    #[test]
    fn init_surfaces_the_adoption_refusal_for_a_name_that_is_another_machine() {
        let _ranch = testing::temp_ranch();
        config::save_barn(&mut Barn {
            name: "pi".into(),
            host: Some("pi.local".into()),
            ..Default::default()
        })
        .unwrap();

        let why = format!("{:#}", init(Some("pi".into())).expect_err("pi is a real machine"));
        assert!(
            why.contains("another machine") && why.contains("pi.local"),
            "the adoption refusal must be surfaced verbatim, not summarised: {}",
            why
        );
    }

    /// The other refusal with teeth: a project file that will not parse would be
    /// skipped by the lenient loader, leaving its livestock machine-relative
    /// forever while the ranch claims to be migrated.
    #[test]
    fn init_surfaces_the_adoption_refusal_for_an_unparseable_project() {
        let ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        std::fs::write(ranch.dir.path().join("projects").join("broken.yaml"), "{[not yaml")
            .unwrap();

        let why = format!("{:#}", init(Some("imac".into())).expect_err("a bad project refuses"));
        assert!(
            why.contains("broken.yaml"),
            "the user can only act on the path, so the path has to survive: {}",
            why
        );
    }

    /// Partial init must leave a *coherent* ranch, not a broken one. A project
    /// whose name carries a forbidden character parses fine and cannot be
    /// written, which is the one way to fail partway through the sweep.
    #[test]
    fn a_failure_partway_through_stamping_leaves_a_coherent_ranch() {
        let ranch = testing::temp_ranch();
        config::ensure_config_dirs();
        config::save_project(&mut project("aaa")).unwrap();
        // Written by hand: `save_project` is what refuses this name, so it cannot
        // be used to create it.
        std::fs::write(
            ranch.dir.path().join("projects").join("zzz.yaml"),
            "name: bad/name\npath: /tmp/bad\nlivestock: []\nherds: []\nwiki: []\n",
        )
        .unwrap();

        let why = format!("{:#}", init(Some("imac".into())).expect_err("an unwritable entity"));
        assert!(why.contains("bad/name"), "the failure must name the entity: {}", why);

        // The house marker and its base landed before the sweep, exactly as
        // `adopt_this_machine` writes `this_barn` before its own sweep.
        assert!(
            this_machine_is_ranch_house(),
            "the record that defines this machine must be written before the bulk sweep"
        );
        let house = this_machine_barn().unwrap();
        assert!(house.brand.is_some(), "the house must carry its brand: {:?}", house);
        let house_id = house.id.clone().unwrap();
        assert!(
            base::load::<Barn>("barn", &house_id).unwrap().is_some(),
            "the house's own base must be recorded with it"
        );

        // And the entities the sweep reached are stamped *and* based — never one
        // without the other, which is what `base::accept` is for.
        let aaa = config::load_projects().into_iter().find(|p| p.name == "aaa").unwrap();
        let aaa_id = aaa.id.clone().expect("the sweep reached it");
        assert!(base::load::<crate::types::Project>("project", &aaa_id).unwrap().is_some());
    }

    // ---- D5: `ranch join` -------------------------------------------------

    /// Names the ranch a spawned serve child must serve. Set on the child's own
    /// environment by [`serve_child_command`] — never with `set_var`, which is
    /// process-global and unsafe against the concurrent `environ` reads every
    /// `Command` spawn and `tempdir()` makes.
    const TEST_SERVE_RANCH: &str = "YEEHAW_TEST_SERVE_RANCH";

    /// A `ranch serve` peer as a real child process, with no ssh and no network.
    ///
    /// The child is **this test binary**, re-run with a filter that selects
    /// [`a_spawned_serve_child`]. Deriving `target/debug/yeehaw` from
    /// `current_exe()` was the other option and is the one that goes stale: it is
    /// only built at all because an integration target exists, and a test that
    /// silently runs last week's binary is worse than no test. `current_exe()` is
    /// by construction the binary cargo just built.
    ///
    /// The harness prints a line or two of its own to stdout before the test
    /// runs. That is exactly the case `transport::recv` is lenient about before
    /// the first decodable line — "because barns print MOTDs" — so the preamble
    /// is stepped over, and it is the same tolerance a real `bash -lc` login
    /// shell needs.
    fn serve_child_command(ranch: &std::path::Path) -> std::process::Command {
        let mut cmd = std::process::Command::new(
            std::env::current_exe().expect("the test binary knows where it is"),
        );
        cmd.args(["--exact", "ranch::tests::a_spawned_serve_child", "--test-threads=1"])
            .env(TEST_SERVE_RANCH, ranch);
        cmd
    }

    /// Set alongside [`TEST_SERVE_RANCH`] to make the child hang up the instant a
    /// push is committed.
    const TEST_SERVE_DIES_AT_COMMIT: &str = "YEEHAW_TEST_SERVE_DIES_AT_COMMIT";

    /// A `serve` peer that reads a pushed batch and then dies, on the frame that
    /// would have authorized writing it.
    ///
    /// A *real* dropped connection rather than a mocked one, which is the whole
    /// point: the client really sends, the house really receives, and the
    /// acknowledgement really never comes. Nothing about the base gating can be
    /// satisfied by a stub that simply declines to call it.
    fn serve_child_that_hangs_up_at_the_commit(ranch: &std::path::Path) -> std::process::Command {
        let mut cmd = serve_child_command(ranch);
        cmd.env(TEST_SERVE_DIES_AT_COMMIT, "1");
        cmd
    }

    /// A reader that reports end-of-input when it reaches the `Commit` frame.
    ///
    /// `read_line` is overridden rather than `fill_buf`/`consume` because the cut
    /// has to be on a frame boundary: `serve_session` reads by line, and a cut
    /// mid-frame would be a *corrupt* stream, which is a different failure with a
    /// different answer.
    struct HangsUpAtCommit<R>(R);

    impl<R: std::io::BufRead> std::io::Read for HangsUpAtCommit<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl<R: std::io::BufRead> std::io::BufRead for HangsUpAtCommit<R> {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            self.0.fill_buf()
        }
        fn consume(&mut self, amount: usize) {
            self.0.consume(amount)
        }
        fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
            let mut line = String::new();
            let read = self.0.read_line(&mut line)?;
            if wire::tag_of(line.trim_end()).as_deref() == Some("commit") {
                return Ok(0); // the peer died holding the frame
            }
            buf.push_str(&line);
            Ok(read)
        }
    }

    /// The serve end of [`serve_child_command`]. A no-op in an ordinary run.
    ///
    /// The ranch arrives as an env var rather than an argument because libtest
    /// owns argv. `exit(0)` rather than returning: the harness would otherwise
    /// print its summary onto the protocol stream after the session, and the
    /// peer's exit status is part of what `transport::finish` reports.
    #[test]
    fn a_spawned_serve_child() {
        let Ok(ranch) = std::env::var(TEST_SERVE_RANCH) else {
            return; // an ordinary `cargo test` run: nothing to serve
        };
        // MEASURED, and the reason this test hung before it failed: libtest writes
        // `test <name> ... ` with **no trailing newline** and completes the line
        // later with its verdict. The first protocol frame is appended to it, so
        // the greeting arrives as part of a line that is not JSON — skipped as
        // preamble by `transport::recv`, exactly as a MOTD would be — and the
        // client then blocks forever on a greeting it will never see. Ending the
        // harness's line first is what puts the protocol on lines of its own.
        {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
        let hang_up = std::env::var(TEST_SERVE_DIES_AT_COMMIT).is_ok();
        testing::with_ranch_env(ranch, || {
            // Errors go nowhere on purpose: stdout is the protocol stream and
            // stderr is the peer's diagnostic channel, which `Transport` captures
            // and quotes.
            if hang_up {
                let _ = serve_session(
                    HangsUpAtCommit(std::io::stdin().lock()),
                    std::io::stdout().lock(),
                );
                return;
            }
            let _ = serve_session(std::io::stdin().lock(), std::io::stdout().lock());
        });
        std::process::exit(0);
    }

    /// Builds a Ranch House in `dir` with `extra` run against it first.
    fn a_house(dir: &std::path::Path, extra: impl FnOnce()) -> InitReport {
        testing::with_ranch_env(dir, || {
            extra();
            init(Some("imac".into())).expect("the house initializes")
        })
    }

    /// Collects what `join` pushed, instead of ssh-ing anywhere.
    struct Pushed(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

    impl Pushed {
        fn new() -> Self {
            Pushed(Default::default())
        }
        fn keys(&self) -> Vec<String> {
            self.0.borrow().clone()
        }
    }

    /// THE TEST THIS FEATURE EXISTS FOR. Two temp ranches, a first join, no ssh
    /// and no network — the peer is a real child process speaking the real
    /// protocol over real pipes.
    #[test]
    fn two_temp_ranches_complete_a_first_join_with_no_ssh() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();

        a_house(house_dir.path(), || {
            config::save_project(&mut project("api")).unwrap();
            config::save_barn(&mut Barn {
                name: "pi".into(),
                host: Some("pi.local".into()),
                ..Default::default()
            })
            .unwrap();
        });
        // The house's uuid for `api`, read straight off its disk. Adopting it is
        // the whole mechanism a first join exists to perform.
        let house_api_id: String = {
            let text =
                std::fs::read_to_string(house_dir.path().join("projects").join("api.yaml")).unwrap();
            serde_yaml::from_str::<crate::types::Project>(&text).unwrap().id.unwrap()
        };

        let pushed = Pushed::new();
        let outcome = testing::with_ranch_env(joiner_dir.path(), || {
            // Something only this machine has, so the plan has both halves.
            config::save_project(&mut project("only-here")).unwrap();

            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| Ok(true);
            let sink = std::rc::Rc::clone(&pushed.0);
            let mut push = move |_: &Barn, keys: &[String]| {
                *sink.borrow_mut() = keys.to_vec();
                Ok(())
            };
            join_with(
                "imac",
                // Proposed, not assumed: the house is the one that decides, and an
                // explicit name keeps this test off whatever `hostname -s` says
                // on the machine running it.
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("the join completes");

        assert_eq!(outcome.peer, "imac", "the peer names itself in its greeting");
        let applied = outcome.applied.expect("the plan was confirmed, so it was applied");
        // api, barn pi, barn imac.
        assert_eq!(applied.written, 3, "plan:\n{}", outcome.plan_text);
        assert_eq!(applied.deleted, 0);

        // The undo for the one run that merges months of accumulated config.
        let backup = outcome.backup.expect("the first join must leave a way back");
        assert!(backup.join("projects").join("only-here.yaml").exists(), "{}", backup.display());
        // The ordering, not just the existence: a copy taken *after* the apply is
        // not an undo for it. The house's project is the proof — it did not exist
        // here before the write, so a backup containing it was taken too late.
        assert!(
            !backup.join("projects").join("api.yaml").exists(),
            "the backup must be taken before the first write, or it cannot undo it: {}",
            backup.display()
        );

        testing::with_ranch_env(joiner_dir.path(), || {
            let projects = config::load_projects();
            let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
            assert!(names.contains(&"api"), "the house's project must arrive: {:?}", names);
            assert!(names.contains(&"only-here"), "ours must survive: {:?}", names);

            let api = projects.iter().find(|p| p.name == "api").unwrap();
            assert_eq!(
                api.id.as_deref(),
                Some(house_api_id.as_str()),
                "a first join adopts the house's uuid; that is what makes every later sync \
                 match by id instead of by name"
            );
            assert!(
                api.path.is_empty(),
                "`project.path` is machine-local — enrollment must not invent one: {:?}",
                api.path
            );
            assert!(
                base::load::<crate::types::Project>("project", &house_api_id).unwrap().is_some(),
                "an applied entity needs a base, or its next merge has no ancestor"
            );

            // The house's own barn arrived, marker and all.
            let house = manifest::barns_from_disk()
                .items
                .into_iter()
                .find(|b| b.name == "imac")
                .expect("the house's barn record is part of the ranch");
            assert_eq!(house.is_ranch_house, Some(true), "{:?}", house);
            assert!(house.brand.is_some(), "the house's brand is content and travels: {:?}", house);

            // DELIBERATE CHANGE, the push slice. This used to assert that
            // `only-here` had *no* base, because nothing outgoing was sent. It is
            // sent now and the house acknowledged it, so the base is owed — and
            // the conditions on it are the whole feature: the base exists, and
            // it describes the entity the house confirmed.
            let ours = config::load_projects().into_iter().find(|p| p.name == "only-here").unwrap();
            let id = ours.id.as_deref().expect("a saved project is stamped");
            let based = base::load::<crate::types::Project>("project", id)
                .unwrap()
                .expect("an entity the house confirmed it wrote has earned a base");
            assert_eq!(based.name, "only-here", "{:?}", based);
            assert!(
                based.path.is_empty(),
                "the base is the copy that crossed the wire, and `path` is machine-local: {:?}",
                based.path
            );
        });

        // The union, on the far side. The house is a child process that has since
        // exited, so this reads its disk directly.
        let on_the_house: Vec<String> = testing::with_ranch_env(house_dir.path(), || {
            config::load_projects().into_iter().map(|p| p.name).collect()
        });
        assert!(
            on_the_house.contains(&"only-here".to_string()),
            "the joining machine's project has to reach the house, or it is invisible to the \
             rest of the ranch: {:?}",
            on_the_house
        );
        // The joiner's own barn record too — the house's roster learns it from the
        // push rather than waiting for a later sync.
        assert_eq!(outcome.pushed, 2, "project only-here and barn macbook: {}", outcome.plan_text);
        assert_eq!(applied.based, 2, "both acknowledged entities earned a base");

        assert!(
            outcome.plan_text.contains("only-here") && outcome.plan_text.contains("Outgoing"),
            "the user has to be shown what this machine holds and the house does not:\n{}",
            outcome.plan_text
        );

        // Step 8: the brand goes into the target's managed block — every brand on
        // the ranch, because the block is rewritten whole and ours alone would
        // delete the rest.
        let keys = pushed.keys();
        assert!(keys.len() >= 2, "ours and the house's, at least: {:?}", keys);
        assert!(keys.iter().all(|k| k.starts_with("ssh-ed25519 ")), "{:?}", keys);
        assert!(outcome.brand_pushed, "{:?}", outcome.warnings);
    }

    // ---- the push ----------------------------------------------------------

    /// Two ranch directories under one root, so the backups `backup_once` writes
    /// into a ranch's *parent* are cleaned up with the root rather than left in
    /// the system temp directory.
    struct TwoRanches {
        root: tempfile::TempDir,
    }

    impl TwoRanches {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            for name in ["house", "joiner"] {
                std::fs::create_dir_all(root.path().join(name)).unwrap();
            }
            TwoRanches { root }
        }
        fn house(&self) -> std::path::PathBuf {
            self.root.path().join("house")
        }
        fn joiner(&self) -> std::path::PathBuf {
            self.root.path().join("joiner")
        }
        /// How many `*.pre-ranch-*` copies exist, whichever ranch they belong to.
        fn backups(&self) -> Vec<String> {
            let mut found: Vec<String> = std::fs::read_dir(self.root.path())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.contains(".pre-ranch-"))
                .collect();
            found.sort();
            found
        }
    }

    /// Every project on a ranch, by name, sorted.
    fn project_names(ranch: &std::path::Path) -> Vec<String> {
        testing::with_ranch_env(ranch, || {
            let mut names: Vec<String> =
                config::load_projects().into_iter().map(|p| p.name).collect();
            names.sort();
            names
        })
    }

    /// THE TEST THE PUSH EXISTS FOR. Two ranches holding **disjoint** projects —
    /// which is what the real ranch looks like — and after one join each holds
    /// the union.
    ///
    /// Before the push, the joining machine's projects stayed on the joining
    /// machine: the plan printed them under "Outgoing" and then nothing sent
    /// them, so they were invisible to every other machine on the ranch, for
    /// ever.
    #[test]
    fn two_ranches_with_disjoint_projects_end_up_holding_the_union() {
        let dirs = TwoRanches::new();

        a_house(&dirs.house(), || {
            config::save_project(&mut project("house-api")).unwrap();
            config::save_project(&mut project("house-web")).unwrap();
        });

        let outcome = testing::with_ranch_env(dirs.joiner(), || {
            config::save_project(&mut project("laptop-notes")).unwrap();
            config::save_project(&mut project("laptop-scratch")).unwrap();

            let mut spawn = |_: &Barn| Ok(serve_child_command(&dirs.house()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| Ok(());
            join_with(
                "imac",
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("the join completes");

        let union = vec![
            "house-api".to_string(),
            "house-web".to_string(),
            "laptop-notes".to_string(),
            "laptop-scratch".to_string(),
        ];
        assert_eq!(
            project_names(&dirs.joiner()),
            union,
            "the joining machine must end up holding both sides:\n{}",
            outcome.plan_text
        );
        assert_eq!(
            project_names(&dirs.house()),
            union,
            "and so must the house — a join that only pulls leaves the joiner's work \
             invisible to the rest of the ranch:\n{}",
            outcome.plan_text
        );

        // Not just present on the house: present *as this machine's entity*, with
        // the uuid the joiner holds, so the next sync matches by id rather than
        // re-adopting by name.
        let ours = testing::with_ranch_env(dirs.joiner(), || {
            config::load_projects().into_iter().find(|p| p.name == "laptop-notes").unwrap()
        });
        let theirs = testing::with_ranch_env(dirs.house(), || {
            config::load_projects().into_iter().find(|p| p.name == "laptop-notes").unwrap()
        });
        assert_eq!(ours.id, theirs.id, "the pushed entity must keep its identity on the house");
        assert!(
            theirs.path.is_empty(),
            "`path` is machine-local: a laptop checkout must not land on the house as a real \
             path: {:?}",
            theirs.path
        );

        // And the base the house's acknowledgement earned, which is what stops
        // this being offered again on every later sync.
        testing::with_ranch_env(dirs.joiner(), || {
            let id = ours.id.as_deref().unwrap();
            assert!(
                base::load::<crate::types::Project>("project", id).unwrap().is_some(),
                "an entity the house confirmed must have a base, or it is re-offered forever"
            );
        });

        // Both sides backed up, once each, before their first write.
        let backups = dirs.backups();
        assert_eq!(backups.len(), 2, "the joiner and the house each need a way back: {:?}", backups);
        assert!(
            backups.iter().any(|b| b.starts_with("house.pre-ranch-")),
            "the house is having another machine's config merged into it too: {:?}",
            backups
        );
    }

    /// THE CONSTRAINT. A push that is never acknowledged must leave **no base**
    /// on the sender — because a base recorded at send time claims the house
    /// holds something it does not, and the next merge then reads this machine's
    /// real local entity as already-synced and drops it. `base.rs` calls that the
    /// unrecoverable direction.
    ///
    /// The failure is real, not mocked: the peer is a live child process that
    /// reads the whole batch and then hangs up on the `Commit` frame — the exact
    /// frame that would have authorized the write. So the entities genuinely
    /// crossed the wire and genuinely were not applied, which is the only
    /// arrangement that can tell a base-on-send implementation apart from a
    /// base-on-ack one.
    #[test]
    fn a_push_the_house_never_acknowledged_leaves_no_base_and_is_re_offered() {
        let dirs = TwoRanches::new();
        a_house(&dirs.house(), || {
            config::save_project(&mut project("house-api")).unwrap();
        });

        let ours_id = testing::with_ranch_env(dirs.joiner(), || {
            let mut p = project("laptop-notes");
            config::save_project(&mut p).unwrap();
            p.id.unwrap()
        });

        let why = testing::with_ranch_env(dirs.joiner(), || {
            let mut spawn = |_: &Barn| Ok(serve_child_that_hangs_up_at_the_commit(&dirs.house()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| panic!("a failed push pushes no brand either");
            format!(
                "{:#}",
                join_with(
                    "imac",
                    Some("macbook".into()),
                    JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
                )
                .expect_err("a peer that never acknowledges the push cannot be trusted to have \
                             applied it")
            )
        });
        assert!(
            why.contains("Nothing has been written here"),
            "the user has to be told the local store is untouched, or the only safe action \
             looks like restoring a backup: {}",
            why
        );

        // The two halves of the invariant.
        testing::with_ranch_env(dirs.joiner(), || {
            assert!(
                base::load::<crate::types::Project>("project", &ours_id).unwrap().is_none(),
                "a base for an entity the house never confirmed claims a sync that did not \
                 happen"
            );
            assert!(
                !config::load_projects().iter().any(|p| p.name == "house-api"),
                "the push goes first, so a push that failed must leave the incoming half \
                 unapplied"
            );
        });
        assert!(
            !project_names(&dirs.house()).contains(&"laptop-notes".to_string()),
            "no `Commit` reached the house, so it must have written nothing: {:?}",
            project_names(&dirs.house())
        );

        // And the recovery: because no base was recorded, the next join offers it
        // again, and this time it lands.
        let outcome = testing::with_ranch_env(dirs.joiner(), || {
            let mut spawn = |_: &Barn| Ok(serve_child_command(&dirs.house()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| Ok(());
            join_with(
                "imac",
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("the retry is the whole point of leaving no base");

        assert!(
            outcome.plan_text.contains("laptop-notes"),
            "a failed push must be re-offered, not silently forgotten:\n{}",
            outcome.plan_text
        );
        assert!(
            project_names(&dirs.house()).contains(&"laptop-notes".to_string()),
            "the retry has to actually land it: {:?}",
            project_names(&dirs.house())
        );
    }

    /// The user is at the joining machine and the house is unattended, so the one
    /// y/n covers both halves — which means a "no" has to stop the push as
    /// squarely as it stops the apply. A decline that had already written to
    /// another machine would be the worst possible reading of the word.
    #[test]
    fn declining_the_plan_pushes_nothing_to_the_house() {
        let dirs = TwoRanches::new();
        a_house(&dirs.house(), || {
            config::save_project(&mut project("house-api")).unwrap();
        });

        let outcome = testing::with_ranch_env(dirs.joiner(), || {
            config::save_project(&mut project("laptop-notes")).unwrap();
            let mut spawn = |_: &Barn| Ok(serve_child_command(&dirs.house()));
            let mut confirm = |_: &str| Ok(false);
            let mut push = |_: &Barn, _: &[String]| panic!("a declined plan pushes nothing");
            join_with(
                "imac",
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("declining is not a failure");

        assert!(
            outcome.plan_text.contains("laptop-notes"),
            "the plan still has to show what would have gone:\n{}",
            outcome.plan_text
        );
        assert_eq!(outcome.pushed, 0, "a declined plan sends nothing");
        assert_eq!(
            project_names(&dirs.house()),
            vec!["house-api".to_string()],
            "the house must be exactly as it was"
        );
        assert_eq!(dirs.backups(), Vec::<String>::new(), "nothing was written, so nothing needed \
                                                          backing up — on either machine");
    }

    // ---- the push, seen from `serve` ---------------------------------------

    /// Drives one `serve_session` over in-memory buffers against the ranch at
    /// `ranch`, and returns everything it said.
    ///
    /// Separate from [`session`], which makes its own throwaway temp ranch — the
    /// push tests have to inspect the disk afterwards, and some of them run two
    /// sessions against the same one.
    fn serve_against(ranch: &std::path::Path, input: &str) -> Vec<wire::Message> {
        let mut out: Vec<u8> = Vec::new();
        testing::with_ranch_env(ranch, || {
            serve_session(std::io::Cursor::new(input.as_bytes().to_vec()), &mut out)
                .expect("a session ends cleanly; a refusal is a message, not an error")
        });
        decode_all(&out)
    }

    /// The entity frames plus their terminator, as a client would send them.
    fn a_pushed_batch(entities: &[(&str, &str, &str)]) -> String {
        let mut input = line(&hello_at(PROTOCOL_VERSION));
        for (kind, name, yaml) in entities {
            input.push_str(&line(&wire::Message::Entity {
                kind: (*kind).into(),
                name: (*name).into(),
                yaml: (*yaml).into(),
            }));
        }
        input.push_str(&line(&wire::Message::Commit));
        input
    }

    fn project_yaml(name: &str, id: &str) -> String {
        format!("name: {}\npath: ''\nlivestock: []\nherds: []\nwiki: []\nid: {}\n", name, id)
    }

    /// `Entity` is the one frame with no reply of its own. The batch is answered
    /// once, at its terminator — the same shape as the house's own entity batch,
    /// which ends with a single `Done` rather than an acknowledgement per entity.
    ///
    /// A reply per entity would be a round trip per entity between two blocking
    /// writers on a pipe with no timeout, for no information the batch answer
    /// does not already carry.
    #[test]
    fn a_pushed_batch_is_answered_once_at_its_terminator() {
        let ranch = testing::temp_ranch();
        testing::with_ranch_env(ranch.dir.path(), || init(Some("imac".into())).unwrap());

        let said = serve_against(
            ranch.dir.path(),
            &a_pushed_batch(&[
                ("project", "api", &project_yaml("api", "11111111-1111-1111-1111-111111111111")),
                ("project", "web", &project_yaml("web", "22222222-2222-2222-2222-222222222222")),
            ]),
        );

        assert_eq!(said.len(), 2, "the greeting and one answer for the whole batch: {:?}", said);
        assert_eq!(
            said[1],
            wire::Message::Applied {
                keys: vec![
                    "project/11111111-1111-1111-1111-111111111111".into(),
                    "project/22222222-2222-2222-2222-222222222222".into(),
                ],
            },
            "the acknowledgement names every key that landed, in `kind/id` spelling: {:?}",
            said[1]
        );

        let landed = project_names(ranch.dir.path());
        assert_eq!(landed, vec!["api".to_string(), "web".to_string()], "{:?}", landed);
    }

    /// Letting `serve` write at all is the new authority in this slice, and the
    /// house is the only machine that has it. The roster *is* the namespace and
    /// only the house holds an authoritative copy — the same reason `ClaimName`
    /// is refused elsewhere — so a machine that is not the house cannot vouch for
    /// the uuids or the names a merge into it would settle.
    #[test]
    fn a_machine_that_is_not_the_ranch_house_refuses_a_pushed_write() {
        let ranch = testing::temp_ranch();
        // Adopted, but not a house: the refusal is about authority, not about
        // being an unknown machine.
        testing::with_ranch_env(ranch.dir.path(), || {
            crate::migrate::adopt_this_machine("pi").unwrap();
        });

        let said = serve_against(
            ranch.dir.path(),
            &a_pushed_batch(&[(
                "project",
                "api",
                &project_yaml("api", "11111111-1111-1111-1111-111111111111"),
            )]),
        );

        assert_eq!(said.len(), 2, "the commit still gets an answer: {:?}", said);
        let why = error_text(&said[1]);
        assert!(why.contains("not the Ranch House"), "{}", why);
        assert!(why.contains("ranch init"), "the refusal must say what to do instead: {}", why);
        assert!(
            project_names(ranch.dir.path()).is_empty(),
            "a machine with no authority must write nothing at all"
        );
    }

    /// Two entities claiming one lock key collapse to one lock and one file, so
    /// applying both writes one on top of the other. `merge::plan_kind` never
    /// emits such a plan — but this side is taking entities off a wire, not out
    /// of a plan it built, so the refusal has to exist here too, and before the
    /// first write rather than after it.
    #[test]
    fn a_pushed_batch_whose_two_entities_claim_one_name_is_refused_before_the_house_writes() {
        let ranch = testing::temp_ranch();
        testing::with_ranch_env(ranch.dir.path(), || init(Some("imac".into())).unwrap());

        // Guard: if these ever stop colliding the test is proving nothing.
        assert_eq!(
            crate::store::lock_key("my api"),
            crate::store::lock_key("my_api"),
            "the two names no longer share a lock key; the test has no teeth"
        );

        let said = serve_against(
            ranch.dir.path(),
            &a_pushed_batch(&[
                ("project", "my api", &project_yaml("my api", "11111111-1111-1111-1111-111111111111")),
                ("project", "my_api", &project_yaml("my_api", "22222222-2222-2222-2222-222222222222")),
            ]),
        );

        // Asserted before the message is looked at: the thing that matters is
        // that nothing was written, and a test that checks the wording first
        // reports a wording problem for a data-loss bug.
        assert!(
            project_names(ranch.dir.path()).is_empty(),
            "the refusal must come before the first write, not after one of the two landed: {:?}",
            project_names(ranch.dir.path())
        );
        assert_eq!(said.len(), 2, "{:?}", said);
        let why = error_text(&said[1]);
        assert!(why.contains("two different entities"), "{}", why);
    }

    /// The house is now having another machine's config merged into it, which is
    /// the same event the joining side takes a backup for. Once, though: what has
    /// to be true is that *a* pre-push copy survives, not that there is one per
    /// push — a copy per sync would fill the parent directory with snapshots of
    /// an already-synced ranch.
    #[test]
    fn the_first_pushed_batch_backs_the_house_up_and_the_second_does_not() {
        let dirs = TwoRanches::new();
        let house = dirs.house();
        testing::with_ranch_env(&house, || init(Some("imac".into())).unwrap());
        assert_eq!(
            dirs.backups(),
            Vec::<String>::new(),
            "`ranch init` writes only this machine's own records; the undo is for a *peer's* \
             config arriving"
        );

        let first = serve_against(
            &house,
            &a_pushed_batch(&[(
                "project",
                "one",
                &project_yaml("one", "11111111-1111-1111-1111-111111111111"),
            )]),
        );
        assert!(matches!(first[1], wire::Message::Applied { .. }), "{:?}", first);
        let after_first = dirs.backups();
        assert_eq!(after_first.len(), 1, "the first receive needs a way back: {:?}", after_first);
        // Taken *before* the write, or it is not an undo for it.
        assert!(
            !dirs
                .root
                .path()
                .join(&after_first[0])
                .join("projects")
                .join("one.yaml")
                .exists(),
            "a copy containing the pushed entity was taken too late to undo it"
        );

        let second = serve_against(
            &house,
            &a_pushed_batch(&[(
                "project",
                "two",
                &project_yaml("two", "22222222-2222-2222-2222-222222222222"),
            )]),
        );
        assert!(matches!(second[1], wire::Message::Applied { .. }), "{:?}", second);
        assert_eq!(
            dirs.backups(),
            after_first,
            "one pre-push copy, not one per push: {:?}",
            dirs.backups()
        );

        let landed = project_names(&house);
        assert_eq!(landed, vec!["one".to_string(), "two".to_string()], "both batches landed: {:?}", landed);
    }

    /// A join adopts the house's uuids. There is nothing to adopt from a machine
    /// that is not a house, and matching by name against one would merge two
    /// unrelated ranches.
    #[test]
    fn joining_a_machine_that_is_not_a_ranch_house_is_refused() {
        let peer_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();

        let why = testing::with_ranch_env(joiner_dir.path(), || {
            let mut spawn = |_: &Barn| Ok(serve_child_command(peer_dir.path()));
            let mut confirm = |_: &str| panic!("a refused peer must never reach the plan");
            let mut push = |_: &Barn, _: &[String]| panic!("nothing is pushed to a non-house");
            format!(
                "{:#}",
                join_with(
                    "elsewhere",
                    None,
                    JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
                )
                .expect_err("a peer with no house marker is not joinable")
            )
        });

        assert!(why.contains("not a Ranch House"), "{}", why);
        assert!(why.contains("ranch init"), "the refusal must say what to do: {}", why);
    }

    #[test]
    fn the_ranch_house_has_nothing_to_join() {
        let _ranch = testing::temp_ranch();
        init(Some("imac".into())).unwrap();

        let mut spawn = |_: &Barn| panic!("the refusal must come before anything is spawned");
        let mut confirm = |_: &str| Ok(true);
        let mut push = |_: &Barn, _: &[String]| Ok(());
        let why = format!(
            "{:#}",
            join_with(
                "somewhere",
                None,
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
            .expect_err("a house does not join")
        );
        assert!(why.contains("Ranch House"), "{}", why);
    }

    #[test]
    fn declining_the_plan_writes_nothing_and_leaves_no_backup() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        a_house(house_dir.path(), || {
            config::save_project(&mut project("api")).unwrap();
        });

        let outcome = testing::with_ranch_env(joiner_dir.path(), || {
            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| Ok(false);
            let mut push = |_: &Barn, _: &[String]| panic!("a declined plan pushes nothing");
            join_with(
                "imac",
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("declining is not a failure");

        assert_eq!(outcome.applied, None);
        assert_eq!(outcome.backup, None, "nothing was written, so nothing needed backing up");
        assert!(!outcome.plan_text.is_empty(), "the user still saw a plan");
        testing::with_ranch_env(joiner_dir.path(), || {
            assert!(config::load_projects().is_empty(), "a declined plan writes no entity");
            // The one thing that *does* survive a decline, deliberately: the
            // name. It has to be settled before the plan is built, because the
            // plan is keyed by it (see `join_with`), so declining cannot undo it
            // — and nothing is lost by keeping it, since adoption is idempotent
            // and every later join and sync needs it anyway.
            assert_eq!(
                config::this_barn_name().as_deref(),
                Some("macbook"),
                "the name is settled before the plan, so a decline leaves it in place"
            );
        });
    }

    // ---- D5: the naming step ----------------------------------------------

    /// Key material only; the comment is appended per case, because what the
    /// roster lookup must *not* depend on is the comment.
    const KEY_MATERIAL: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFake";

    fn barn_named(name: &str) -> Barn {
        Barn { name: name.into(), ..Default::default() }
    }

    /// A livestock that names no barn — "whichever machine is reading this file".
    /// What the TUI writes when the barn field is left empty, which is the state
    /// every machine starts in and the one a join has to end.
    fn machine_relative_livestock(name: &str, path: &str) -> crate::types::Livestock {
        crate::types::Livestock {
            name: name.into(),
            path: path.into(),
            barn: None,
            repo: None,
            branch: None,
            log_path: None,
            env_path: None,
            source: None,
            k8s_metadata: None,
            trails: vec![],
        }
    }

    fn project_with_livestock(name: &str, ls: crate::types::Livestock) -> crate::types::Project {
        let mut p = project(name);
        p.livestock.push(ls);
        p
    }

    #[test]
    fn a_brand_the_roster_has_never_seen_gets_the_name_it_proposed() {
        let roster = vec![barn_named("imac"), barn_named("pi")];
        assert_eq!(assign_name("macbook", KEY_MATERIAL, &roster).unwrap(), "macbook");
        // Whitespace is not part of a name; a trimmed one is still free.
        assert_eq!(assign_name("  macbook \n", KEY_MATERIAL, &roster).unwrap(), "macbook");
    }

    /// THE IDEMPOTENCE THE BRAND IS FOR. A machine that re-joins gets the name the
    /// house already knows its brand by — and that branch has to win even when the
    /// proposed name is *taken*, because the machine holding it is this one. Two
    /// Raspberry Pis are both `hostname -s` = `pi`; without this the first one to
    /// re-join would be refused its own name.
    #[test]
    fn a_brand_the_roster_knows_keeps_the_name_it_is_already_known_by() {
        let mut pi = barn_named("pi");
        pi.brand = Some(format!("{} yeehaw-ranch-pi", KEY_MATERIAL));
        let roster = vec![barn_named("imac"), pi];

        assert_eq!(
            assign_name("pi", &format!("{} yeehaw-ranch-pi", KEY_MATERIAL), &roster).unwrap(),
            "pi",
            "a re-join must not be refused the name it already has"
        );
        assert_eq!(
            assign_name("something-else", KEY_MATERIAL, &roster).unwrap(),
            "pi",
            "the house's name for a brand it knows outranks a fresh proposal"
        );
    }

    /// `brand::ensure_brand` mints the comment once and never re-mints, so a
    /// renamed barn carries a comment naming what it used to be. Matching whole
    /// lines would fail to recognize that machine in exactly the case this lookup
    /// exists for.
    #[test]
    fn a_brand_is_recognized_by_its_key_material_not_its_comment() {
        let mut known = barn_named("macbook");
        known.brand = Some(format!("{} yeehaw-ranch-the-old-name", KEY_MATERIAL));
        assert_eq!(
            assign_name("whatever", &format!("{} yeehaw-ranch-macbook", KEY_MATERIAL), &[known])
                .unwrap(),
            "macbook"
        );
    }

    /// The refusal a user actually hits, and what it owes them: which name is
    /// taken, which machine has it, and how to pass a different one. No
    /// auto-suffix — a machine called `pi-2` that nobody chose is worse than being
    /// interrupted, because the name is written into every livestock record on it.
    #[test]
    fn a_name_another_machine_already_holds_is_refused_by_name() {
        let mut pi = barn_named("pi");
        pi.host = Some("pi.local".into());
        let why = assign_name("pi", KEY_MATERIAL, &[pi]).expect_err("that name is taken");

        assert!(why.contains("'pi'"), "name what is taken: {}", why);
        assert!(why.contains("pi.local"), "say which machine has it: {}", why);
        assert!(why.contains("--as"), "say what to do instead: {}", why);
        assert!(!why.contains("pi-2"), "no auto-suffix: {}", why);
    }

    /// Everything about the policy rests on the brand being the discriminator, so
    /// a claim that carries none cannot be answered with a name: doing so would
    /// hand out exactly the un-idempotent enrollment it exists to prevent.
    #[test]
    fn a_claim_with_no_usable_brand_is_refused_rather_than_named() {
        assert!(assign_name("macbook", "", &[]).is_err(), "no brand at all");
        assert!(
            assign_name("macbook", "ssh-ed25519", &[]).is_err(),
            "a key type with no key material is not a brand"
        );
        // And a blank brand on the roster never compares equal to a blank claim.
        let mut ghost = barn_named("ghost");
        ghost.brand = Some(String::new());
        let why = assign_name("macbook", "   ", &[ghost]).expect_err("neither is a brand");
        assert!(!why.contains("ghost"), "two non-brands are not the same machine: {}", why);
    }

    #[test]
    fn a_machine_can_be_named_neither_nothing_nor_local() {
        let blank = assign_name("   ", KEY_MATERIAL, &[]).expect_err("a blank name is a path");
        assert!(blank.contains("--as"), "{}", blank);

        let reserved = assign_name(config::LOCAL_BARN_NAME, KEY_MATERIAL, &[])
            .expect_err("'local' means whichever machine is reading, so it names none");
        assert!(reserved.contains("reserved"), "{}", reserved);
    }

    /// A machine that has never run `ranch init` joins a house and comes away with
    /// a name of its OWN.
    ///
    /// The bug this replaces fell back to the peer's name: a MacBook joining a Pi
    /// called itself `pi`, branded itself `yeehaw-ranch-pi`, and — because nothing
    /// adopted it — left every one of its livestock saying `barn: null`, which the
    /// merge reads as the house's own.
    #[test]
    fn an_unadopted_joiner_is_named_by_the_house_never_after_the_peer() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        a_house(house_dir.path(), || {});

        // The bare `join <target>` form: nothing proposed, so the proposal is this
        // machine's hostname. Whatever that is, it is not the house's name.
        let expected = this_machine_default_name().expect("a hostname to propose");
        assert_ne!(expected, "imac", "this test needs a hostname that is not the house's name");

        let outcome = testing::with_ranch_env(joiner_dir.path(), || {
            config::save_project(&mut project_with_livestock(
                "api",
                machine_relative_livestock("web", "/joiner/web"),
            ))
            .unwrap();

            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| Ok(());
            join_with(
                "imac",
                None,
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("the join completes");

        assert_eq!(outcome.barn, expected, "this machine is named for itself");
        assert_ne!(
            outcome.barn, outcome.peer,
            "naming this machine after the peer is the bug: it collides with the peer's own \
             record the moment either of them is saved"
        );
        let report = outcome.adopted.expect("a machine with no name is adopted by its first join");
        assert!(report.barn_created, "{:?}", report);
        assert_eq!(report.livestock_reassigned, 1, "{:?}", report);

        testing::with_ranch_env(joiner_dir.path(), || {
            assert_eq!(config::this_barn_name().as_deref(), Some(expected.as_str()));

            let api = config::load_projects().into_iter().find(|p| p.name == "api").unwrap();
            let web = api.livestock.iter().find(|l| l.name == "web").expect("ours survived");
            assert_eq!(
                web.barn.as_deref(),
                Some(expected.as_str()),
                "a livestock still saying `barn: null` after enrollment is one every other \
                 machine on the ranch reads as its own"
            );

            let public = std::fs::read_to_string(brand::public_key_path()).unwrap();
            assert!(
                public.contains(&format!("yeehaw-ranch-{}", expected)),
                "the brand is commented with this machine, not the peer: {}",
                public
            );
            assert!(!public.contains("yeehaw-ranch-imac"), "{}", public);

            let ours = manifest::barns_from_disk()
                .items
                .into_iter()
                .find(|b| b.name == expected)
                .expect("this machine now has a barn record of its own");
            assert!(ours.brand.is_some(), "and its brand is on it: {:?}", ours);
            assert!(ours.host.is_none(), "there is nothing to dial to reach yourself: {:?}", ours);
        });
    }

    /// `--as` is how a user escapes `hostname -s`, which on this machine says
    /// `Camerons-MacBook-Air-2` and on two Raspberry Pis says `pi` twice.
    #[test]
    fn a_proposed_name_that_is_free_is_the_one_assigned() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        a_house(house_dir.path(), || {});

        let outcome = testing::with_ranch_env(joiner_dir.path(), || {
            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| Ok(());
            join_with(
                "imac",
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("the join completes");

        assert_eq!(outcome.barn, "macbook");
        testing::with_ranch_env(joiner_dir.path(), || {
            assert_eq!(config::this_barn_name().as_deref(), Some("macbook"));
        });
    }

    /// THE DATA LOSS THIS CHANGE EXISTS TO STOP.
    ///
    /// Two machines, each running a deployment called `web` that names no barn —
    /// which is what the TUI writes when the barn field is left empty, on both of
    /// them. `merge::livestock_key` collapses `None` to `""`, so while the joining
    /// machine is unadopted both sides key as `("", "web")` and the merge settles
    /// them as *one* element: the house's copy wins whole, and this machine's
    /// `/joiner/web` is gone with nothing but a conflict line to say so.
    ///
    /// Adopting before the merge is what separates them. Note that the house's own
    /// copy still arrives as `barn: null` — a livestock added on the house *after*
    /// its `init` is machine-relative again, which is the same defect on the other
    /// side of the wire and is not in this change's reach. What is pinned here is
    /// that this machine's deployment is no longer keyed as `""` and so no longer
    /// fuses with whatever the house calls local.
    #[test]
    fn two_machines_deployments_of_the_same_name_survive_a_join_as_two() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();

        a_house(house_dir.path(), || {});
        // Added after `init`, which is the case that keeps happening: `init`
        // rewrites the livestock that exist when it runs, and the next one the
        // user adds with an empty barn field is machine-relative all over again.
        testing::with_ranch_env(house_dir.path(), || {
            config::save_project(&mut project_with_livestock(
                "api",
                machine_relative_livestock("web", "/house/web"),
            ))
            .unwrap();
        });

        testing::with_ranch_env(joiner_dir.path(), || {
            config::save_project(&mut project_with_livestock(
                "api",
                machine_relative_livestock("web", "/joiner/web"),
            ))
            .unwrap();

            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| Ok(());
            join_with(
                "imac",
                Some("macbook".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("the join completes");

        testing::with_ranch_env(joiner_dir.path(), || {
            let api = config::load_projects().into_iter().find(|p| p.name == "api").unwrap();
            let paths: Vec<&str> = api.livestock.iter().map(|l| l.path.as_str()).collect();

            assert_eq!(
                api.livestock.len(),
                2,
                "two physically different machines' deployments, not one: {:?}",
                paths
            );
            let ours = api
                .livestock
                .iter()
                .find(|l| l.path == "/joiner/web")
                .unwrap_or_else(|| panic!(
                    "this machine's own deployment was merged away: it keyed as (\"\", \"web\") \
                     and so did the house's, so the house's path won. Got {:?}",
                    paths
                ));
            assert_eq!(
                ours.barn.as_deref(),
                Some("macbook"),
                "and it is keyed by the barn this machine now is"
            );
            assert!(paths.contains(&"/house/web"), "the house's must survive too: {:?}", paths);
        });
    }

    /// A re-join is keyed by the brand, so it is idempotent: the machine gets the
    /// name the house already knows it by whatever it proposes, and no second barn
    /// appears for one machine.
    #[test]
    fn a_re_join_keeps_the_name_the_house_knows_this_brand_by() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();

        // The joiner, already enrolled: a brand of its own, and a name.
        let our_brand = testing::with_ranch_env(joiner_dir.path(), || {
            crate::migrate::adopt_this_machine("macbook").unwrap();
            brand::ensure_brand("macbook").unwrap()
        });

        a_house(house_dir.path(), || {});
        // The house's roster as it looks once that machine's barn record has
        // reached it: its brand is on the record.
        testing::with_ranch_env(house_dir.path(), || {
            config::save_barn(&mut Barn {
                name: "macbook".into(),
                brand: Some(our_brand.clone()),
                connectable: Some(false),
                ..Default::default()
            })
            .unwrap();
        });

        let outcome = testing::with_ranch_env(joiner_dir.path(), || {
            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| Ok(true);
            let mut push = |_: &Barn, _: &[String]| Ok(());
            join_with(
                "imac",
                // Proposing something else entirely, to prove which side decides.
                Some("laptop".into()),
                JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
            )
        })
        .expect("a re-join completes");

        assert_eq!(
            outcome.barn, "macbook",
            "the brand is the identity, so the name the house already has for it wins"
        );
        assert_eq!(
            outcome.adopted, None,
            "already this barn: nothing to rewrite, and nothing to announce"
        );

        testing::with_ranch_env(joiner_dir.path(), || {
            assert_eq!(config::this_barn_name().as_deref(), Some("macbook"));
            let names: Vec<String> =
                manifest::barns_from_disk().items.into_iter().map(|b| b.name).collect();
            assert!(
                !names.contains(&"laptop".to_string()),
                "a re-join must not mint a second barn for one machine: {:?}",
                names
            );
            assert_eq!(
                names.iter().filter(|n| *n == "macbook").count(),
                1,
                "one machine, one record: {:?}",
                names
            );
        });
    }

    /// Two Raspberry Pis are both `pi`. The second is stopped with the name that
    /// is taken and what to do about it — and *before* anything local is rewritten,
    /// which is the reason the name is claimed before it is adopted.
    #[test]
    fn a_clashing_name_refuses_the_join_before_anything_local_is_rewritten() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        a_house(house_dir.path(), || {
            config::save_barn(&mut Barn {
                name: "pi".into(),
                host: Some("pi.local".into()),
                ..Default::default()
            })
            .unwrap();
        });

        let why = testing::with_ranch_env(joiner_dir.path(), || {
            config::save_project(&mut project_with_livestock(
                "api",
                machine_relative_livestock("web", "/joiner/web"),
            ))
            .unwrap();

            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| panic!("a machine with no name must never reach a plan");
            let mut push = |_: &Barn, _: &[String]| panic!("nothing is pushed to a refused join");
            let why = format!(
                "{:#}",
                join_with(
                    "imac",
                    Some("pi".into()),
                    JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
                )
                .expect_err("that name is another machine's")
            );

            // Half-enrolled is the state worth avoiding: a name recorded for a
            // machine the house does not agree is that name, or a livestock
            // rewritten to a barn some Pi already has. (The brand *is* minted —
            // it is the identity being claimed, it is idempotent, and it is what
            // makes the next attempt recognizable.)
            assert_eq!(config::this_barn_name(), None, "no name was recorded");
            assert!(
                manifest::barns_from_disk().items.is_empty(),
                "and no barn record was written"
            );
            let api = config::load_projects().into_iter().find(|p| p.name == "api").unwrap();
            assert_eq!(api.livestock[0].barn, None, "the livestock was not rewritten either");
            why
        });

        assert!(why.contains("'pi'"), "{}", why);
        assert!(why.contains("pi.local"), "say which machine holds it: {}", why);
        assert!(why.contains("--as"), "say what to do instead: {}", why);
    }

    /// `migrate::adopt_this_machine`'s refusals are the whole of the user's
    /// guidance — the path of the project that will not parse, or the barn that
    /// turns out to be another machine — so they have to arrive intact rather than
    /// as "the join failed".
    #[test]
    fn an_adoption_refusal_reaches_the_user_with_the_file_to_go_and_fix() {
        let house_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        a_house(house_dir.path(), || {});

        let why = testing::with_ranch_env(joiner_dir.path(), || {
            // A project that does not parse. The migration refuses rather than
            // skipping it: a skipped project keeps its livestock machine-relative
            // forever while the ranch claims to be adopted.
            std::fs::create_dir_all(config::projects_dir()).unwrap();
            std::fs::write(config::projects_dir().join("broken.yaml"), "name: [unclosed\n").unwrap();

            let mut spawn = |_: &Barn| Ok(serve_child_command(house_dir.path()));
            let mut confirm = |_: &str| panic!("an unadopted machine must never reach a plan");
            let mut push = |_: &Barn, _: &[String]| panic!("nothing is pushed");
            let why = format!(
                "{:#}",
                join_with(
                    "imac",
                    Some("macbook".into()),
                    JoinIo { spawn: &mut spawn, confirm: &mut confirm, push_brand: &mut push },
                )
                .expect_err("a machine whose projects do not parse cannot be adopted")
            );
            assert_eq!(config::this_barn_name(), None, "and nothing was half-written");
            why
        });

        assert!(
            why.contains("broken.yaml"),
            "the path is the only thing the user can act on: {}",
            why
        );
        // Specifically the *adoption's* refusal, which names the barn it could
        // not adopt. `client::load_local` refuses an unparseable project too, and
        // a join that got as far as that one would have skipped the enrollment.
        assert!(why.contains("adopted as barn 'macbook'"), "{}", why);
    }

    // ---- D5: target resolution --------------------------------------------

    #[test]
    fn a_bare_target_is_parsed_as_user_at_host_and_port() {
        let _ranch = testing::temp_ranch();

        let plain = resolve_target("pi.local").unwrap();
        assert_eq!((plain.host.as_deref(), plain.user.as_deref(), plain.port), (Some("pi.local"), None, None));

        let full = resolve_target("cam@pi.local:2222").unwrap();
        assert_eq!(
            (full.host.as_deref(), full.user.as_deref(), full.port),
            (Some("pi.local"), Some("cam"), Some(2222))
        );

        // A bare IPv6 literal has no unambiguous port, so it is taken whole; a
        // bracketed one keeps its brackets, which is what `ssh` wants.
        assert_eq!(resolve_target("::1").unwrap().host.as_deref(), Some("::1"));
        let v6 = resolve_target("cam@[::1]:22").unwrap();
        assert_eq!((v6.host.as_deref(), v6.port), (Some("[::1]"), Some(22)));
    }

    /// A configured barn wins: that is where the port, the user and the identity
    /// file live, and a bare hostname would silently drop all three.
    #[test]
    fn a_configured_barn_is_preferred_over_parsing_its_name() {
        let _ranch = testing::temp_ranch();
        config::save_barn(&mut Barn {
            name: "pi".into(),
            host: Some("10.0.0.9".into()),
            user: Some("cam".into()),
            port: Some(2222),
            ..Default::default()
        })
        .unwrap();

        let barn = resolve_target("pi").unwrap();
        assert_eq!(
            (barn.host.as_deref(), barn.user.as_deref(), barn.port),
            (Some("10.0.0.9"), Some("cam"), Some(2222)),
            "a bare hostname would have dropped the user, the port and the identity file"
        );
    }

    /// This machine's own record, and a k8s-discovered node, both have no host.
    #[test]
    fn a_hostless_barn_cannot_be_joined_and_says_so() {
        let _ranch = testing::temp_ranch();
        config::save_barn(&mut Barn { name: "myself".into(), ..Default::default() }).unwrap();
        let why = format!("{:#}", resolve_target("myself").expect_err("nothing to dial"));
        assert!(why.contains("user@host"), "the refusal must say what to pass instead: {}", why);
    }
}
