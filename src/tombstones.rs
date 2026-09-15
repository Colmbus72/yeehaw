//! Records of deleted entities, so a sync can distinguish "deleted here"
//! from "never existed here".

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tombstone {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub deleted_at: String,
}

pub fn tombstones_dir() -> PathBuf {
    crate::config::yeehaw_dir().join("tombstones")
}

/// Entombs one deleted entity.
///
/// `id` is the entity's uuid, read from its file *before* the file was
/// removed. The uuid is what a peer matches on, so a tombstone without one
/// cannot cancel the peer's copy.
pub fn record(kind: &str, name: &str, id: Option<&str>) -> Result<()> {
    // An entity deleted before it was ever stamped has no uuid — everything on
    // disk predates Task 6, and nothing restamps a file until it is next saved.
    // Fall back to a deterministic kind/name key so the deletion is still
    // recorded rather than dropped.
    let id = id
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}--{}", kind, name));

    let t = Tombstone {
        id: id.clone(),
        kind: kind.to_string(),
        name: name.to_string(),
        deleted_at: chrono::Utc::now().to_rfc3339(),
    };

    // `write_atomic` creates the parent directory itself — its temp file has to
    // live there — so there is no `create_dir_all` here.
    crate::store::write_atomic(
        &tombstones_dir().join(format!("{}.yaml", file_stem_for(&id))),
        &serde_yaml::to_string(&t)?,
    )
}

/// The filename stem a tombstone with this `id` is stored under.
///
/// Primarily containment: `id` and the `{kind}--{name}` fallback are built from
/// caller-supplied text, and `Path::join` on `"../../escaped"` walks straight
/// out of the store. Collapsing everything outside `[A-Za-z0-9_-]` makes that
/// structurally impossible rather than relying on every caller to validate.
///
/// The sanitizer itself is `store::sanitized_stem` — one copy, shared with
/// `store::lock_key` and `ranch::base::file_stem_for`.
///
/// The mapping is many-to-one, exactly like `store::lock_key`: two ids that
/// differ only in punctuation share one file, and the later deletion overwrites
/// the earlier one. Uuids — what `record` is given in every real path — contain
/// only hex and `-`, so they are unaffected. The fallback key can collide
/// (`"my api"` and `"my_api"`), which would lose one deletion record. Carried
/// forward to Phase 2: escape rather than collapse if it ever matters.
fn file_stem_for(id: &str) -> String {
    crate::store::sanitized_stem(id)
}

fn load_all_with_paths() -> Vec<(PathBuf, Tombstone)> {
    let dir = tombstones_dir();
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(c) = fs::read_to_string(&path) {
                    if let Ok(t) = serde_yaml::from_str::<Tombstone>(&c) {
                        out.push((path, t));
                    }
                }
            }
        }
    }
    out
}

pub fn load_all() -> Vec<Tombstone> {
    load_all_with_paths().into_iter().map(|(_, t)| t).collect()
}

/// Drops tombstones older than `max_age_days`, returning how many went.
///
/// A tombstone only needs to outlive the longest plausible gap between syncs;
/// past that it is dead weight in every directory listing.
///
/// Two things this must not do, both of which resurrect deleted entities:
///
/// - **Compare `deleted_at` as a string.** RFC3339 is not lexically ordered
///   across offsets. `chrono`'s own serde impl emits a `Z` suffix, and `'Z'`
///   sorts above every digit; a peer's non-UTC offset shifts the digits by up
///   to a day in either direction. Parse first, always.
/// - **Rebuild the filename from the record.** The file to unlink is the file
///   it was found in. A stone that arrived from a peer is named by that peer,
///   and [`file_stem_for`] is many-to-one, so a rebuilt name can miss.
///
/// A tombstone whose `deleted_at` will not parse is *kept*. An unreadable date
/// is not evidence of age, and keeping one costs a file while dropping it costs
/// a deletion.
pub fn reap(max_age_days: i64) -> Result<usize> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
    let mut removed = 0;
    for (path, t) in load_all_with_paths() {
        let expired = chrono::DateTime::parse_from_rfc3339(&t.deleted_at)
            .map(|d| d.with_timezone(&chrono::Utc) < cutoff)
            .unwrap_or(false);
        if expired && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a tombstone file verbatim, bypassing `record`, so a test can
    /// control the timestamp and the filename independently.
    fn write_raw(file_stem: &str, t: &Tombstone) {
        fs::create_dir_all(tombstones_dir()).unwrap();
        fs::write(
            tombstones_dir().join(format!("{}.yaml", file_stem)),
            serde_yaml::to_string(t).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn records_and_loads() {
        crate::testing::with_temp_ranch(|_| {
            record("project", "api", Some("abc-123")).unwrap();
            let all = load_all();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].kind, "project");
            assert_eq!(all[0].name, "api");
            assert_eq!(all[0].id, "abc-123");
        });
    }

    #[test]
    fn falls_back_to_kind_name_key_when_id_absent() {
        crate::testing::with_temp_ranch(|_| {
            record("worm", "nightly", None).unwrap();
            assert_eq!(load_all()[0].id, "worm--nightly");
        });
    }

    #[test]
    fn ensure_config_dirs_creates_the_tombstone_directory() {
        crate::testing::with_temp_ranch(|_| {
            crate::config::ensure_config_dirs();
            assert!(
                tombstones_dir().is_dir(),
                "ensure_config_dirs() must create {}",
                tombstones_dir().display()
            );
        });
    }

    #[test]
    fn reap_removes_only_expired() {
        crate::testing::with_temp_ranch(|_| {
            record("project", "fresh", Some("fresh-id")).unwrap();

            write_raw(
                "old-id",
                &Tombstone {
                    id: "old-id".into(),
                    kind: "project".into(),
                    name: "old".into(),
                    deleted_at: (chrono::Utc::now() - chrono::Duration::days(120)).to_rfc3339(),
                },
            );

            assert_eq!(reap(90).unwrap(), 1);
            let remaining = load_all();
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].name, "fresh");
        });
    }

    /// A tombstone written by another machine carries whatever offset that
    /// machine's clock used, and RFC3339 is not lexically ordered across
    /// offsets. Comparing `deleted_at` as a string therefore reaps live
    /// tombstones — and a reaped tombstone is a resurrected entity on the next
    /// sync.
    ///
    /// The fresh stone below is three hours *newer* than the cutoff but,
    /// rendered at `-12:00`, its digits read nine hours *older*. Only parsing
    /// gets it right.
    #[test]
    fn reap_parses_timestamps_rather_than_comparing_them_as_strings() {
        crate::testing::with_temp_ranch(|_| {
            let west = chrono::FixedOffset::west_opt(12 * 3600).unwrap();
            let east = chrono::FixedOffset::east_opt(12 * 3600).unwrap();

            let fresh_instant =
                chrono::Utc::now() - chrono::Duration::days(90) + chrono::Duration::hours(3);
            write_raw(
                "fresh-id",
                &Tombstone {
                    id: "fresh-id".into(),
                    kind: "project".into(),
                    name: "fresh".into(),
                    deleted_at: fresh_instant.with_timezone(&west).to_rfc3339(),
                },
            );

            let expired_instant = chrono::Utc::now() - chrono::Duration::days(120);
            write_raw(
                "old-id",
                &Tombstone {
                    id: "old-id".into(),
                    kind: "project".into(),
                    name: "old".into(),
                    deleted_at: expired_instant.with_timezone(&east).to_rfc3339(),
                },
            );

            assert_eq!(reap(90).unwrap(), 1, "exactly the expired stone must go");
            let remaining = load_all();
            assert_eq!(remaining.len(), 1);
            assert_eq!(
                remaining[0].name, "fresh",
                "a stone three hours newer than the cutoff was reaped because \
                 its offset made it look older as a string"
            );
        });
    }

    /// The file to unlink is the file it was found in, not one rebuilt from
    /// the record's `id`. Two ids that sanitize to the same stem share a file,
    /// and a tombstone that arrives from a peer is named by that peer.
    /// Rebuilding the name silently leaves expired stones on disk forever.
    #[test]
    fn reap_removes_a_tombstone_whose_file_is_not_named_after_its_id() {
        crate::testing::with_temp_ranch(|_| {
            write_raw(
                "arrived-from-a-peer",
                &Tombstone {
                    id: "9f2c/weird:id".into(),
                    kind: "project".into(),
                    name: "old".into(),
                    deleted_at: (chrono::Utc::now() - chrono::Duration::days(120)).to_rfc3339(),
                },
            );

            assert_eq!(reap(90).unwrap(), 1);
            assert!(load_all().is_empty(), "expired stone survived the reap");
        });
    }

    /// `record` builds a filename out of caller-supplied text. An id or name
    /// carrying path separators must land inside the tombstone directory, not
    /// somewhere else on the filesystem.
    #[test]
    fn record_cannot_escape_the_tombstone_directory() {
        crate::testing::with_temp_ranch(|ranch| {
            record("project", "..", Some("../../escaped")).unwrap();

            assert_eq!(load_all().len(), 1, "the record must land in the store");
            assert!(
                !ranch.dir.path().parent().unwrap().join("escaped.yaml").exists(),
                "record wrote outside the tombstone directory"
            );
        });
    }
}
