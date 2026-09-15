//! This machine's brand: the ed25519 keypair that gets it onto every barn.
//!
//! A brand is generated once, on the machine it belongs to, and its **private
//! half never leaves that machine**. It is not in the vault, it is not a synced
//! entity, and it is not on the wire — `manifest::build` enumerates the five
//! entity directories and `brand/` is not one of them. Only the public half
//! circulates, as `Barn.brand` on an ordinary synced barn record, and the Ranch
//! House writes the collected public halves into each barn's
//! `authorized_keys`.
//!
//! # Why `ssh-keygen` rather than a Rust crate
//!
//! The consumer is `sshd`, so the artifact has to be exactly what `sshd`
//! accepts — an OpenSSH private key and a one-line `ssh-ed25519 AAAA... comment`
//! public half. `ssh-keygen` is the reference implementation of that format and
//! is already on every machine that can be a barn at all (it ships with the ssh
//! client this codebase shells out to everywhere else). A crate would be a
//! second opinion about a format we do not control.

#![allow(dead_code)] // `push_to_barn` is wired up by Slice E.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// `ssh-keygen`, by name rather than by path.
///
/// Resolved through `PATH` the same way `ssh`, `kubectl` and `crontab` are
/// elsewhere in this codebase: a hardcoded `/usr/bin/ssh-keygen` is right on
/// macOS and wrong on a NixOS barn.
const KEYGEN: &str = "ssh-keygen";

/// `~/.yeehaw/brand` — this machine's own keypair, and nothing else.
pub fn brand_dir() -> PathBuf {
    crate::config::yeehaw_dir().join("brand")
}

/// The private half. Never read by anything but `ssh` itself.
pub fn private_key_path() -> PathBuf {
    brand_dir().join("id_ed25519")
}

/// The public half — the only part that circulates.
pub fn public_key_path() -> PathBuf {
    brand_dir().join("id_ed25519.pub")
}

/// This machine's public brand, or `None` if it has never been branded.
pub fn public_key() -> Result<Option<String>> {
    match std::fs::read_to_string(public_key_path()) {
        Ok(text) => Ok(Some(text.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| {
            format!("failed to read this machine's brand at {}", public_key_path().display())
        }),
    }
}

/// This machine's public brand, generating the keypair if it has none.
///
/// # Idempotent, and that is the safety property rather than a nicety
///
/// An existing private half is **never** replaced, whatever `barn` says. The
/// public half it pairs with has by then been pushed into the managed block of
/// every barn's `authorized_keys` across the whole ranch, and those files are
/// the one thing a re-brand cannot reach: a second `ensure_brand` that minted a fresh
/// key would leave every barn trusting a key this machine had just thrown away,
/// and this machine unable to reach any of them — including, if it is not the
/// Ranch House, the one it would have to reach to fix it.
///
/// `barn` is therefore used only when a key is actually minted, as the comment
/// on the public half. A renamed barn keeps its key and keeps the old comment;
/// the comment is a label for a human reading `authorized_keys`, not an
/// identity, and re-minting to correct it would cost the ranch its trust for
/// this machine.
///
/// The one thing that *is* regenerated is a **missing public half** over a
/// private half that is still there — `ssh-keygen -y` derives it from the
/// private key, so the key itself is unchanged and every barn still trusts it.
/// Minting a new pair in that situation would be the lockout above, triggered
/// by nothing worse than a deleted `.pub` file.
pub fn ensure_brand(barn: &str) -> Result<String> {
    if private_key_path().exists() {
        if let Some(existing) = public_key()? {
            return Ok(existing);
        }
        // The private half survived and the public half did not. Derive rather
        // than re-mint: the key every barn trusts is still right here.
        let derived = derive_public_half(barn)?;
        crate::store::write_atomic(&public_key_path(), &format!("{}\n", derived))
            .with_context(|| {
                format!("failed to restore the public brand at {}", public_key_path().display())
            })?;
        return Ok(derived);
    }

    generate(barn)?;
    harden_private_half()?;

    public_key()?.ok_or_else(|| {
        anyhow::anyhow!(
            "{} reported success but wrote no public half at {}",
            KEYGEN,
            public_key_path().display()
        )
    })
}

/// `yeehaw-ranch-<barn>` — the comment on the public half.
fn comment_for(barn: &str) -> String {
    format!("yeehaw-ranch-{}", barn)
}

fn generate(barn: &str) -> Result<()> {
    generate_with(KEYGEN, barn)
}

/// Split out from [`generate`] on the `tool` argument so the missing-binary
/// path is reachable from a test without touching `PATH` — which is
/// process-global and, as `testing::RANCH` documents at length, not safe to
/// mutate while other threads are spawning children.
fn generate_with(tool: &str, barn: &str) -> Result<()> {
    prepare_brand_dir()?;

    let out = std::process::Command::new(tool)
        .args(["-t", "ed25519", "-N", "", "-C", &comment_for(barn), "-f"])
        .arg(private_key_path())
        // Null, not inherited. `ssh-keygen` prompts "Overwrite (y/n)?" when `-f`
        // names a file that already exists; [`ensure_brand`] checks first, but a
        // concurrent `ranch init` could still land in between, and a prompt on an
        // inherited stdin is a hang. On a closed stdin it reads EOF and declines.
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| spawn_failure(tool, e))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        anyhow::bail!(
            "{} could not write this machine's brand to {}{}",
            tool,
            private_key_path().display(),
            if stderr.is_empty() { String::new() } else { format!(": {}", stderr) }
        );
    }
    Ok(())
}

/// The public half, recomputed from the private one.
fn derive_public_half(barn: &str) -> Result<String> {
    let out = std::process::Command::new(KEYGEN)
        .arg("-y")
        .arg("-f")
        .arg(private_key_path())
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| spawn_failure(KEYGEN, e))?;

    if !out.status.success() {
        anyhow::bail!(
            "{} could not read the private brand at {} to recover its public half: {}",
            KEYGEN,
            private_key_path().display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let derived = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if derived.is_empty() {
        anyhow::bail!("{} -y produced nothing for {}", KEYGEN, private_key_path().display());
    }
    // `-y` *usually* carries the comment through, because the OpenSSH private
    // key format stores it — but the format is allowed not to, and an older or
    // differently-generated key can come back as the bare two fields. So the
    // comment is appended only when there is none to begin with; appending
    // unconditionally produced `... yeehaw-ranch-imac yeehaw-ranch-imac`, which
    // `sshd` accepts (everything past the blob is comment) and a human reading
    // the file does not.
    if derived.split_whitespace().count() >= 3 {
        Ok(derived)
    } else {
        Ok(format!("{} {}", derived, comment_for(barn)))
    }
}

/// "No such file or directory (os error 2)" is the error a missing `ssh-keygen`
/// produces, and it sends the user looking at their ranch. Naming the tool and
/// the package it comes from is the difference between that and one `apt
/// install`.
fn spawn_failure(tool: &str, e: std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!(
            "could not run `{}`, which this machine needs in order to mint its ranch brand. \
             It ships with OpenSSH — install the OpenSSH client package (`openssh-client` on \
             Debian/Ubuntu, `openssh` on Alpine and Arch; it is present by default on macOS) \
             and run this again",
            tool
        )
    } else {
        anyhow::Error::new(e).context(format!("failed to run `{}`", tool))
    }
}

/// `~/.yeehaw/brand`, 0700.
///
/// The directory bits matter independently of the file's: a world-readable
/// directory is not itself a leak of the key, but `ssh` refuses a private key in
/// a group-writable directory, and a brand `ssh` will not load is a machine that
/// silently cannot reach any barn.
fn prepare_brand_dir() -> Result<()> {
    let dir = brand_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to restrict {} to this user", dir.display()))?;
    }
    Ok(())
}

/// Confirms — and, if need be, fixes — the private half's 0600.
///
/// `ssh-keygen` writes 0600 itself, so this is belt and braces rather than the
/// mechanism. It is here anyway because the failure it guards is silent in both
/// directions: a wider mode hands every account on a shared machine the run of
/// the ranch, and `ssh` refuses to use such a key at all, which surfaces as
/// "every barn stopped working" rather than as a permissions problem.
fn harden_private_half() -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = private_key_path();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict {} to this user", path.display()))?;
        let mode = std::fs::metadata(&path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .permissions()
            .mode()
            & 0o777;
        if mode != 0o600 {
            anyhow::bail!(
                "this machine's private brand at {} is mode {:o} rather than 600, and could not \
                 be restricted. Anything wider lets another account on this machine reach every \
                 barn on the ranch",
                path.display(),
                mode
            );
        }
    }
    Ok(())
}

// ============================================================================
// D3 — the managed `authorized_keys` block
// ============================================================================

/// Opens the block this code owns. Every line between this and [`END_MARKER`]
/// is rewritten on every push; nothing outside them is.
pub const BEGIN_MARKER: &str = "# BEGIN YEEHAW RANCH - DO NOT EDIT";
/// Closes it.
pub const END_MARKER: &str = "# END YEEHAW RANCH";

/// Rewrites the managed block of an `authorized_keys` file from `keys`.
///
/// Pure, over the file's whole contents, so the thing that can lock a user out
/// of their own machine is testable without an ssh, a remote, or a real
/// `~/.ssh` anywhere in sight. Pushing the result somewhere is a separate and
/// deliberately thin layer ([`install_block_at`], [`push_to_barn`]).
///
/// # Matching `crontab::sync_crontab`
///
/// Same reviewed shape, and for the same reason: whole-line marker matching,
/// the block rebuilt rather than edited in place, everything outside the markers
/// carried through, and trailing blank lines trimmed so repeated rewrites do not
/// accumulate whitespace. Two deliberate departures, both because this file is
/// `authorized_keys` and not a crontab:
///
/// 1. **An unterminated `BEGIN` does not swallow the rest of the file.**
///    `sync_crontab` sets `in_section = true` and never clears it, so every line
///    below a marker with no `END` is dropped. In a crontab that loses jobs; here
///    it would delete the user's own public keys and lock them out of the
///    machine. So a `BEGIN` with no matching `END` is not a block: the marker
///    line itself is dropped (it is ours — we are the only thing that writes it)
///    and the lines below it are kept as ordinary content. The cost is named
///    rather than hidden: a stale brand can survive above the fresh block until
///    somebody tidies the file. That is the same choice `base.rs` makes between a
///    lagging and a leading failure — a key that should have gone is recoverable,
///    a key that should have stayed is a locked door.
///
/// 2. **The separating blank line is only written when something precedes the
///    block.** `sync_crontab` emits it unconditionally, which on an empty file
///    leaves a leading blank line that grows nothing but is noise in a file
///    people read with `ssh-keygen -lf`. Checked by
///    `rewriting_an_already_managed_file_changes_nothing`, which is the property
///    that actually matters: applying this to its own output is a no-op.
///
/// # Keys are filtered, not trusted
///
/// `Barn.brand` is a **synced field**: its value arrives from a peer, and on a
/// first join from a peer this machine has never merged with before. A value
/// carrying a newline would smuggle arbitrary lines — including a forged
/// `END_MARKER`, putting attacker-chosen keys outside the managed region where
/// no later push would ever remove them — straight into `authorized_keys`. So a
/// key is used only if it is one non-blank line and is not itself a marker.
/// Duplicates collapse, first occurrence winning, because two barns can
/// legitimately have been handed the same brand and a file listing it twice is
/// only noise.
pub fn rewrite_block(existing: &str, keys: &[String]) -> String {
    let kept = strip_managed_blocks(existing);

    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }

    let installable = installable_keys(keys);
    if !installable.is_empty() {
        // Only when something precedes it. See the doc comment: unconditional,
        // as `sync_crontab` does it, this is a leading blank line on a file that
        // had no content at all.
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(BEGIN_MARKER);
        out.push('\n');
        for key in &installable {
            out.push_str(key);
            out.push('\n');
        }
        out.push_str(END_MARKER);
        out.push('\n');
    }

    out
}

/// Every line that is not part of a managed block, with trailing blank lines
/// dropped.
///
/// A block is a `BEGIN` **and** a matching `END`. The two damaged shapes are
/// handled rather than trusted:
///
/// - **`BEGIN` with no `END`** — not a block. The marker line is dropped and
///   everything below it kept, because those lines are as likely to be the
///   user's keys as ours and dropping them locks them out. This is the departure
///   from `sync_crontab`, which swallows to EOF.
/// - **`END` with no `BEGIN`** — a stray marker of ours. Dropped, because
///   leaving it makes the *next* pass read everything above it as managed
///   content and delete it, which is the lockout one rewrite later.
fn strip_managed_blocks(existing: &str) -> Vec<&str> {
    let lines: Vec<&str> = existing.lines().collect();

    // Found first, used second. Whether a `BEGIN` opens a block depends on
    // whether an `END` turns up later, which a single forward pass cannot know
    // at the moment it meets the marker.
    let mut kept: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i] == BEGIN_MARKER {
            match lines[i + 1..].iter().position(|l| *l == END_MARKER) {
                // A real block: skip the markers and everything between them.
                Some(offset) => i += offset + 2,
                // Unterminated. Drop our marker, keep the rest as content.
                None => i += 1,
            }
            continue;
        }
        // A stray `END`, since any `END` belonging to a block was skipped above.
        if lines[i] == END_MARKER {
            i += 1;
            continue;
        }
        kept.push(lines[i]);
        i += 1;
    }

    // `sync_crontab`'s rule, and the reason a repeated push does not slowly grow
    // the file: the separator below is re-added every time, so the one left by
    // the previous push has to come off.
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }

    kept
}

/// The keys that may be written, in first-seen order.
///
/// See [`rewrite_block`]'s docs: `Barn.brand` is a synced field whose value
/// arrives from a peer, so a multi-line value is an injection and not a typo.
/// Such a key is dropped whole rather than truncated at its first newline —
/// truncating would install an attacker-chosen *prefix*, which may still be a
/// usable key.
fn installable_keys(keys: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for key in keys {
        let key = key.trim();
        if key.is_empty() || key == BEGIN_MARKER || key == END_MARKER {
            continue;
        }
        // `\r` too: a CRLF file read as text leaves it on the end of the line,
        // and a lone `\r` is a line terminator to enough parsers to matter.
        if key.contains('\n') || key.contains('\r') {
            continue;
        }
        if seen.insert(key) {
            out.push(key.to_string());
        }
    }
    out
}

/// Rewrites the managed block of one `authorized_keys` file on this machine.
///
/// The path is a parameter rather than `~/.ssh/authorized_keys` computed inside,
/// which is what lets every test of this run against a `tempfile::tempdir()`.
/// **No test in this module reads or writes the developer's real `~/.ssh`.**
///
/// A missing file is an empty one: the first push to a fresh barn is the normal
/// case, not an error. The mode bits are not decoration — `sshd` silently
/// ignores an `authorized_keys` that is group- or world-writable, and ignores one
/// in a directory that is, so getting them wrong produces "the key was installed
/// and still does not work" with nothing in any log the user will find.
pub fn install_block_at(path: &Path, keys: &[String]) -> Result<()> {
    if let Some(dir) = path.parent() {
        // The mode is set only on a directory this call created. Re-chmodding one
        // that was already there would reach past the job: the parent of a path
        // handed in from elsewhere can be `$HOME`, and silently narrowing a
        // user's home directory to 0700 is not something installing a key is
        // entitled to do. A `.ssh` the user has already set up is theirs, and if
        // its bits are wrong `sshd` says so in its own log.
        let existed = dir.exists();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        #[cfg(unix)]
        if !existed {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to restrict {}", dir.display()))?;
        }
    }

    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // The first push to a barn that has never had a key installed.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", path.display()))
        }
    };

    // `write_atomic_with_mode`, not `write_atomic`: a plain atomic write carries
    // the temp file's `0666 & ~umask` onto the target, which would *widen* an
    // `authorized_keys` that was already 0600 and make `sshd` start ignoring it.
    crate::store::write_atomic_with_mode(
        path,
        rewrite_block(&existing, keys).as_bytes(),
        0o600,
    )
    .with_context(|| format!("failed to write {}", path.display()))
}

/// `~/.ssh/authorized_keys` on **this** machine.
///
/// Not used by [`install_block_at`], which takes its path, so that nothing in
/// this module's tests can reach it.
pub fn local_authorized_keys() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".ssh").join("authorized_keys")
}

/// Reads a barn's `authorized_keys`, as a shell command.
///
/// A barn that has never had a key pushed to it has no such file, and that is the
/// normal first-push case — so a missing file reads as empty rather than as a
/// failure. Without the `|| true` the command exits non-zero and the push aborts.
fn remote_read_command() -> String {
    "cat ~/.ssh/authorized_keys 2>/dev/null || true".to_string()
}

/// Installs `authorized_keys` from stdin, as a shell command.
///
/// Temp-file-then-`mv`, for exactly the reason `store::write_atomic` exists, and
/// with more at stake: a plain `cat > ~/.ssh/authorized_keys` truncates the live
/// file *first*, so an ssh that dies mid-transfer leaves the barn with an empty
/// or half-written `authorized_keys` — which is to say, locked. The rename is
/// atomic, so the file is either the old one or the new one.
///
/// The `chmod` is on the temp file, **before** the rename, for the same reason
/// `store::write_inner` does it in that order: after the rename the file is
/// already reachable at its real name, and `sshd` ignoring it for one moment is
/// a race nobody can see.
fn remote_install_command() -> String {
    // No `$`, no double quotes: this goes through `ssh` as a single argument and
    // may be re-expanded by a login shell on the far side, the same constraint
    // `ssh::PROBE_CMD` documents.
    concat!(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && ",
        "cat > ~/.ssh/.authorized_keys.yeehaw-new && ",
        "chmod 600 ~/.ssh/.authorized_keys.yeehaw-new && ",
        "mv ~/.ssh/.authorized_keys.yeehaw-new ~/.ssh/authorized_keys"
    )
    .to_string()
}

/// Rewrites the managed block of `barn`'s `authorized_keys` over ssh.
///
/// The thin layer [`rewrite_block`] was split out from: read the far side's
/// file, rewrite it here where it is testable, write it back. Everything that
/// can be wrong in a way that locks somebody out lives in `rewrite_block` and in
/// the two command builders above, all of which are tested; what is left is the
/// plumbing.
pub fn push_to_barn(barn: &crate::types::Barn, keys: &[String]) -> Result<()> {
    use std::io::Write;

    let existing = crate::ssh::run(barn, &remote_read_command(), crate::ssh::Opts::default())
        .with_context(|| format!("failed to read authorized_keys on barn '{}'", barn.name))?;

    let updated = rewrite_block(&existing, keys);

    let mut child =
        crate::ssh::command(barn, &remote_install_command(), crate::ssh::Opts::default())?
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to reach barn '{}'", barn.name))?;

    child
        .stdin
        .take()
        .expect("piped")
        .write_all(updated.as_bytes())
        .with_context(|| format!("failed to send authorized_keys to barn '{}'", barn.name))?;

    let out = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for barn '{}'", barn.name))?;
    if !out.status.success() {
        anyhow::bail!(
            "barn '{}' refused the authorized_keys update: {}",
            barn.name,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// The whole point of the private half. A brand readable by another account
    /// on a shared machine is a brand that account can use to reach every barn
    /// on the ranch.
    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The refusal that matters most, and the one a test cannot get from a
    /// second call alone: a brand that is *replaced* is a brand every barn's
    /// `authorized_keys` still trusts and this machine no longer holds. Every
    /// barn then has to be re-pushed, and until that happens this machine
    /// cannot reach any of them.
    #[test]
    fn a_second_brand_is_never_minted_over_the_first() {
        testing::with_temp_ranch(|_| {
            let first = ensure_brand("imac").unwrap();
            let private_before = std::fs::read(private_key_path()).unwrap();

            let second = ensure_brand("imac").unwrap();

            assert_eq!(
                first, second,
                "the public half changed, so every barn that trusts the first one now \
                 trusts a key this machine has thrown away"
            );
            assert_eq!(
                private_before,
                std::fs::read(private_key_path()).unwrap(),
                "the private half was replaced; the old public half is still in every \
                 barn's authorized_keys and this machine can no longer prove it holds it"
            );
        });
    }

    /// Even a different barn name must not re-brand. A machine has one identity;
    /// the name is a label on it, not a second key.
    #[test]
    fn a_different_barn_name_does_not_re_brand_the_machine() {
        testing::with_temp_ranch(|_| {
            let first = ensure_brand("imac").unwrap();
            let again = ensure_brand("the-pi").unwrap();
            assert_eq!(first, again, "renaming the barn must not mint a new key");
        });
    }

    #[test]
    #[cfg(unix)]
    fn the_private_half_is_readable_only_by_its_owner() {
        testing::with_temp_ranch(|_| {
            ensure_brand("imac").unwrap();
            assert_eq!(
                mode_of(&private_key_path()),
                0o600,
                "anything wider lets another account on this machine reach every barn"
            );
        });
    }

    /// `Barn.brand` is a synced field. A private key that reached it would be
    /// published to every machine on the ranch and written into a file the TUI
    /// browses.
    #[test]
    fn the_brand_is_the_public_half_and_only_the_public_half() {
        testing::with_temp_ranch(|_| {
            let brand = ensure_brand("imac").unwrap();

            assert!(
                brand.starts_with("ssh-ed25519 "),
                "a brand is one OpenSSH public key line, got {:?}",
                brand
            );
            assert_eq!(brand.lines().count(), 1, "a public key is one line: {:?}", brand);
            assert!(
                !brand.contains("PRIVATE KEY"),
                "the private half must never reach Barn.brand: {:?}",
                brand
            );
            // Not a paraphrase of the file: what `sshd` accepts is what the
            // public half literally says.
            assert_eq!(
                brand,
                std::fs::read_to_string(public_key_path()).unwrap().trim(),
                "the brand must be the public key file's own contents"
            );
        });
    }

    /// The comment is how a human reading a barn's `authorized_keys` can tell
    /// which machine a line belongs to, and the only place the barn name appears
    /// in the key material at all.
    #[test]
    fn the_public_half_is_commented_with_the_barn_it_belongs_to() {
        testing::with_temp_ranch(|_| {
            let brand = ensure_brand("the-pi").unwrap();
            assert!(
                brand.ends_with("yeehaw-ranch-the-pi"),
                "the comment names the barn so a line in authorized_keys can be traced: {:?}",
                brand
            );
        });
    }

    /// A deleted `.pub` must not cost the machine its identity. The private half
    /// is still there, every barn still trusts the key it pairs with, and
    /// `ssh-keygen -y` can reproduce the public half exactly — so re-minting
    /// here would be a ranch-wide lockout caused by one missing file.
    #[test]
    fn a_lost_public_half_is_recovered_from_the_private_one_rather_than_re_minted() {
        testing::with_temp_ranch(|_| {
            let original = ensure_brand("imac").unwrap();
            let private_before = std::fs::read(private_key_path()).unwrap();
            std::fs::remove_file(public_key_path()).unwrap();

            let recovered = ensure_brand("imac").unwrap();

            assert_eq!(
                recovered, original,
                "the public half must be derived from the surviving private key, not replaced"
            );
            assert_eq!(
                private_before,
                std::fs::read(private_key_path()).unwrap(),
                "the private half must not be touched to recover a public half"
            );
            assert_eq!(
                std::fs::read_to_string(public_key_path()).unwrap().trim(),
                original,
                "the recovered file has to hold what the ranch already trusts"
            );
        });
    }

    /// `public_key()` is what `ranch status` and the greeting read. On an
    /// unbranded machine that is a plain "no", not an error to handle.
    #[test]
    fn an_unbranded_machine_reports_no_brand_rather_than_failing() {
        testing::with_temp_ranch(|_| {
            assert_eq!(public_key().unwrap(), None);
        });
    }

    /// The invariant the whole module exists for, asserted against the real
    /// manifest builder rather than by inspection.
    #[test]
    fn no_part_of_the_brand_directory_can_reach_a_manifest() {
        testing::with_temp_ranch(|_| {
            ensure_brand("imac").unwrap();
            let private_half = std::fs::read_to_string(private_key_path()).unwrap();

            let entries = crate::ranch::manifest::build()
                .expect("a branded ranch still builds a manifest");

            let rendered = format!("{:?}", entries);
            assert!(
                !rendered.contains("id_ed25519"),
                "the brand's files must not appear in a manifest: {}",
                rendered
            );
            for line in private_half.lines().filter(|l| l.len() > 20) {
                assert!(
                    !rendered.contains(line),
                    "private key material reached the manifest: {:?}",
                    line
                );
            }
            assert!(
                !crate::ranch::manifest::KINDS.contains(&"brand"),
                "`brand` must not be an entity kind; the private half would sync"
            );
        });
    }

    /// `ssh-keygen` is missing on a stripped container more often than anyone
    /// expects. "No such file or directory (os error 2)" sends the user looking
    /// at their ranch; naming the tool sends them to their package manager.
    #[test]
    fn a_missing_ssh_keygen_says_what_is_missing() {
        testing::with_temp_ranch(|_| {
            let why = generate_with("yeehaw-no-such-keygen", "imac")
                .expect_err("a missing key generator cannot be a success");
            let text = format!("{:#}", why);
            assert!(
                text.contains("yeehaw-no-such-keygen"),
                "the error must name the program that is missing: {}",
                text
            );
            assert!(
                text.to_lowercase().contains("openssh"),
                "naming the package it comes from is the actionable half: {}",
                text
            );
        });
    }

    /// The production path must use the real tool. Without this the test above
    /// could pass against a `generate_with` nothing ever calls with `ssh-keygen`.
    #[test]
    fn the_generator_this_build_shells_out_to_is_ssh_keygen() {
        assert_eq!(KEYGEN, "ssh-keygen");
    }

    // ---- D3: the managed `authorized_keys` block --------------------------
    //
    // Every test below is a pure string comparison. NOTHING in this module's
    // tests reads or writes the developer's `~/.ssh`: `rewrite_block` is a
    // function over text, and the two tests that touch a filesystem at all
    // ([`install_block_at`]) are handed a path inside a `tempfile::tempdir()`.

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const BRAND_A: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAaaaaaaaa yeehaw-ranch-imac";
    const BRAND_B: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAbbbbbbbb yeehaw-ranch-the-pi";
    /// What the user put there themselves, years ago, from some other machine.
    const THEIRS: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAAuuuuuuuu cam@laptop";

    /// Pulls out the lines between the markers. Panics rather than returning an
    /// `Option`: a file this code wrote and cannot parse back is a bug worth a
    /// loud failure in every test that calls this.
    fn block_of(text: &str) -> Vec<String> {
        let mut inside = false;
        let mut found = false;
        let mut lines = Vec::new();
        for line in text.lines() {
            if line == BEGIN_MARKER {
                assert!(!inside, "nested BEGIN markers in:\n{}", text);
                inside = true;
                found = true;
                continue;
            }
            if line == END_MARKER {
                assert!(inside, "an END with no BEGIN in:\n{}", text);
                inside = false;
                continue;
            }
            if inside {
                lines.push(line.to_string());
            }
        }
        assert!(found, "no managed block at all in:\n{}", text);
        assert!(!inside, "the block was never closed in:\n{}", text);
        lines
    }

    /// Lines that are neither markers nor inside the block.
    fn outside_of(text: &str) -> Vec<String> {
        let mut inside = false;
        let mut lines = Vec::new();
        for line in text.lines() {
            if line == BEGIN_MARKER {
                inside = true;
                continue;
            }
            if line == END_MARKER {
                inside = false;
                continue;
            }
            if !inside && !line.is_empty() {
                lines.push(line.to_string());
            }
        }
        lines
    }

    #[test]
    fn a_file_with_no_block_yet_gains_one_and_keeps_what_was_there() {
        let out = rewrite_block(&format!("{}\n", THEIRS), &keys(&[BRAND_A]));

        assert_eq!(block_of(&out), vec![BRAND_A]);
        assert_eq!(
            outside_of(&out),
            vec![THEIRS],
            "the user's own key is how they get into this machine:\n{}",
            out
        );
    }

    /// The lockout. `authorized_keys` files routinely lack a final newline —
    /// every `echo -n "$key" >> authorized_keys` in every blog post leaves one
    /// that way — and appending to it without one produces
    /// `...cam@laptop# BEGIN YEEHAW RANCH`, which is one unparseable line where
    /// there were two. `sshd` then rejects the user's own key.
    #[test]
    fn a_file_with_no_final_newline_does_not_fuse_two_lines_into_one() {
        // No trailing newline, and two keys, so both joins are exercised.
        let existing = format!("{}\n{}", THEIRS, BRAND_B);

        let out = rewrite_block(&existing, &keys(&[BRAND_A]));

        for line in out.lines() {
            assert!(
                line.matches("ssh-").count() <= 1,
                "two keys ended up on one line, which breaks both:\n{}",
                out
            );
        }
        assert!(
            out.lines().any(|l| l == THEIRS),
            "the user's key must survive as its own whole line:\n{}",
            out
        );
        assert!(
            out.lines().any(|l| l == BEGIN_MARKER),
            "the marker must be its own line, or the block can never be found again:\n{}",
            out
        );
    }

    /// The ordinary case, and the one that has to be byte-exact: a push replaces
    /// the block and touches nothing else. The assertion is on the literal text
    /// above the block, not on a line list, because a dropped space in a key is
    /// a key that stops working.
    #[test]
    fn everything_above_and_below_an_existing_block_survives_byte_for_byte() {
        let existing = format!(
            "# my own keys, do not delete\n{}\n\n{}\n{}\n{}\n{}\n",
            THEIRS, BEGIN_MARKER, BRAND_B, END_MARKER, "ssh-ed25519 AAAAzzzz deploy@ci"
        );

        let out = rewrite_block(&existing, &keys(&[BRAND_A]));

        assert_eq!(
            block_of(&out),
            vec![BRAND_A],
            "the stale brand must be gone from the block:\n{}",
            out
        );
        assert!(
            out.starts_with(&format!("# my own keys, do not delete\n{}\n", THEIRS)),
            "the text above the block must come through verbatim:\n{}",
            out
        );
        assert!(
            outside_of(&out).contains(&"ssh-ed25519 AAAAzzzz deploy@ci".to_string()),
            "a key the user put *below* the block must survive too:\n{}",
            out
        );
        assert!(
            !out.contains(BRAND_B),
            "a brand no longer in the list must not survive anywhere:\n{}",
            out
        );
    }

    /// Removing a barn from the ranch removes exactly its line, and leaves the
    /// others exactly as they were.
    #[test]
    fn dropping_one_brand_removes_exactly_its_line() {
        let full = rewrite_block(&format!("{}\n", THEIRS), &keys(&[BRAND_A, BRAND_B]));
        assert_eq!(block_of(&full), vec![BRAND_A, BRAND_B]);

        let pruned = rewrite_block(&full, &keys(&[BRAND_A]));

        assert_eq!(block_of(&pruned), vec![BRAND_A]);
        assert_eq!(
            outside_of(&pruned),
            vec![THEIRS],
            "pruning a brand must not disturb the user's own keys:\n{}",
            pruned
        );
    }

    /// A ranch with no brands yet is not a file with an empty pair of markers.
    #[test]
    fn no_keys_leaves_no_block_at_all() {
        let existing = rewrite_block(&format!("{}\n", THEIRS), &keys(&[BRAND_A]));

        let out = rewrite_block(&existing, &[]);

        assert_eq!(out, format!("{}\n", THEIRS), "got:\n{:?}", out);
    }

    /// The property that keeps a push from slowly deforming the file: applying
    /// this to its own output must change nothing. Without it, blank lines or
    /// marker pairs accumulate a little on every sync and nobody notices for
    /// months.
    #[test]
    fn rewriting_an_already_managed_file_changes_nothing() {
        for existing in ["", "\n", &format!("{}\n", THEIRS), &format!("{}", THEIRS)] {
            let once = rewrite_block(existing, &keys(&[BRAND_A, BRAND_B]));
            let twice = rewrite_block(&once, &keys(&[BRAND_A, BRAND_B]));
            assert_eq!(
                once, twice,
                "a second identical push must be a no-op; from {:?} it was not",
                existing
            );
        }
    }

    /// Two blocks is what a file gets when an older build appended without
    /// stripping, or when two pushes raced. Both are ours, so both go, and one
    /// fresh block replaces them — the alternative is a file where the second
    /// block silently shadows nothing and a removed brand stays trusted forever.
    #[test]
    fn duplicated_blocks_collapse_into_one() {
        let existing = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            BEGIN_MARKER, BRAND_B, END_MARKER, THEIRS, BEGIN_MARKER, BRAND_B, END_MARKER
        );

        let out = rewrite_block(&existing, &keys(&[BRAND_A]));

        assert_eq!(out.matches(BEGIN_MARKER).count(), 1, "one block, got:\n{}", out);
        assert_eq!(out.matches(END_MARKER).count(), 1, "one block, got:\n{}", out);
        assert_eq!(block_of(&out), vec![BRAND_A]);
        assert_eq!(outside_of(&out), vec![THEIRS], "got:\n{}", out);
    }

    /// The second lockout, and the deliberate departure from `sync_crontab`.
    /// A `BEGIN` whose `END` somebody deleted must not take the rest of the file
    /// with it — the lines below it are as likely to be the user's own keys as
    /// ours, and dropping them locks them out of the machine.
    #[test]
    fn an_unterminated_begin_marker_does_not_swallow_the_keys_below_it() {
        let existing = format!("{}\n{}\n{}\n", BEGIN_MARKER, BRAND_B, THEIRS);

        let out = rewrite_block(&existing, &keys(&[BRAND_A]));

        assert!(
            out.lines().any(|l| l == THEIRS),
            "a key below a damaged marker is still the user's way in:\n{}",
            out
        );
        assert_eq!(block_of(&out), vec![BRAND_A], "got:\n{}", out);
        assert_eq!(
            out.matches(BEGIN_MARKER).count(),
            1,
            "the orphaned marker must not be left behind to damage the next parse:\n{}",
            out
        );
    }

    /// A stray `END` with nothing opening it is our marker too, and leaving it
    /// in place would make the *next* rewrite read everything above it as
    /// managed content and delete it.
    #[test]
    fn a_stray_end_marker_is_removed_rather_than_left_to_mislead_the_next_pass() {
        let existing = format!("{}\n{}\n", THEIRS, END_MARKER);

        let out = rewrite_block(&existing, &keys(&[BRAND_A]));

        assert_eq!(out.matches(END_MARKER).count(), 1, "got:\n{}", out);
        assert_eq!(block_of(&out), vec![BRAND_A]);
        assert!(out.lines().any(|l| l == THEIRS), "got:\n{}", out);
    }

    /// `Barn.brand` arrives over the wire from a peer. A value carrying a
    /// newline could close the block early and plant keys *outside* it, where no
    /// later push would ever remove them — a permanent back door installed by
    /// one malformed field.
    #[test]
    fn a_key_that_tries_to_smuggle_extra_lines_is_refused_entirely() {
        let smuggled = format!("{}\n{}\nssh-ed25519 AAAAevil attacker", BRAND_B, END_MARKER);

        let out = rewrite_block("", &keys(&[BRAND_A, &smuggled]));

        assert_eq!(
            block_of(&out),
            vec![BRAND_A],
            "only the well-formed key may be installed:\n{}",
            out
        );
        assert!(
            !out.contains("attacker"),
            "a multi-line brand must not reach the file at all:\n{}",
            out
        );
        assert_eq!(out.matches(END_MARKER).count(), 1, "the block was escaped:\n{}", out);
    }

    /// `Some("")` is a real value for an `Option<String>` field that something
    /// wrote without thinking, and a blank line inside the block is the kind of
    /// thing that makes a human delete the block by hand.
    #[test]
    fn blank_and_whitespace_only_keys_never_reach_the_block() {
        let out = rewrite_block("", &keys(&["", "   ", BRAND_A, "\t"]));
        assert_eq!(block_of(&out), vec![BRAND_A], "got:\n{}", out);
    }

    /// Two barn records can carry the same brand — the same machine adopted
    /// twice, or a hand-copied field. The file should say it once.
    #[test]
    fn the_same_brand_listed_twice_appears_once() {
        let out = rewrite_block("", &keys(&[BRAND_A, BRAND_B, BRAND_A]));
        assert_eq!(block_of(&out), vec![BRAND_A, BRAND_B], "got:\n{}", out);
    }

    /// Indentation is not a marker. A line that merely *contains* the text must
    /// not be treated as one, or a comment mentioning it rearranges the file.
    #[test]
    fn only_a_whole_line_counts_as_a_marker() {
        let existing = format!("# see {} below\n{}\n", BEGIN_MARKER, THEIRS);

        let out = rewrite_block(&existing, &keys(&[BRAND_A]));

        assert!(
            out.contains(&format!("# see {} below", BEGIN_MARKER)),
            "a comment mentioning the marker is content, not a marker:\n{}",
            out
        );
        assert_eq!(block_of(&out), vec![BRAND_A]);
        assert!(out.lines().any(|l| l == THEIRS), "got:\n{}", out);
    }

    /// The file always ends with a newline. `sshd` tolerates the alternative,
    /// but the next `>>` by hand does not — which is how the no-final-newline
    /// case above comes about in the first place.
    #[test]
    fn the_result_always_ends_with_a_newline() {
        for existing in ["", THEIRS, &format!("{}\n", THEIRS)] {
            let out = rewrite_block(existing, &keys(&[BRAND_A]));
            assert!(out.ends_with('\n'), "from {:?} got {:?}", existing, out);
        }
    }

    // ---- D3: the thin push layer -----------------------------------------

    /// A temp dir, never `~/.ssh`. The path is handed in precisely so this test
    /// cannot reach the developer's real one.
    #[test]
    fn installing_into_a_file_that_does_not_exist_yet_creates_it_locked_down() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".ssh").join("authorized_keys");

        install_block_at(&path, &keys(&[BRAND_A])).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(block_of(&written), vec![BRAND_A]);
        #[cfg(unix)]
        {
            assert_eq!(mode_of(&path), 0o600, "sshd ignores a group-writable authorized_keys");
            assert_eq!(
                mode_of(&path.parent().unwrap().to_path_buf()),
                0o700,
                "sshd ignores authorized_keys in a directory others can write"
            );
        }
    }

    #[test]
    fn installing_over_an_existing_file_keeps_the_keys_it_did_not_put_there() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("authorized_keys");
        std::fs::write(&path, format!("{}\n", THEIRS)).unwrap();

        install_block_at(&path, &keys(&[BRAND_A])).unwrap();
        // Twice, because a push happens on every join.
        install_block_at(&path, &keys(&[BRAND_A])).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(outside_of(&written), vec![THEIRS]);
        assert_eq!(block_of(&written), vec![BRAND_A]);
        assert_eq!(written.matches(BEGIN_MARKER).count(), 1, "got:\n{}", written);
    }

    /// The remote half cannot be driven without a barn, so the part of it that
    /// can be wrong in a damaging way — the shell snippet — is pinned here
    /// instead. `>` would truncate `authorized_keys` and then fail to fill it if
    /// the pipe broke; the temp-then-`mv` is the same reason `store::write_atomic`
    /// exists, and the `chmod` has to land on the temp file before the rename.
    #[test]
    fn the_remote_install_command_never_truncates_authorized_keys_in_place() {
        let cmd = remote_install_command();

        assert!(
            !cmd.contains("> ~/.ssh/authorized_keys") && !cmd.contains(">~/.ssh/authorized_keys"),
            "writing straight to the live file truncates it before the new content arrives, \
             and a broken pipe then leaves the user with no keys at all: {}",
            cmd
        );
        assert!(cmd.contains("mv "), "the new file has to be renamed into place: {}", cmd);
        let chmod = cmd.find("chmod 600").expect("the temp file must be locked down");
        let rename = cmd.find("mv ").expect("checked above");
        assert!(
            chmod < rename,
            "chmod after the rename leaves a window where the live file is world-readable: {}",
            cmd
        );
        assert!(
            cmd.contains("mkdir -p ~/.ssh") && cmd.contains("chmod 700 ~/.ssh"),
            "a barn with no ~/.ssh yet needs one sshd will actually read: {}",
            cmd
        );
    }

    /// The read side must not fail on a barn that has no `authorized_keys` yet —
    /// which is every barn the first time a brand is pushed to it. A non-zero
    /// exit there would abort the join after the local apply had already landed.
    #[test]
    fn the_remote_read_command_treats_a_missing_file_as_empty() {
        let cmd = remote_read_command();
        assert!(
            cmd.contains("2>/dev/null") && cmd.contains("|| true"),
            "a barn with no authorized_keys yet must read as empty, not as a failure: {}",
            cmd
        );
    }
}
