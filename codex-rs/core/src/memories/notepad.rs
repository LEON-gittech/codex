//! Notepad subsystem for session-scoped working memory.
//!
//! Inspired by oh-my-codex's notepad system. Provides three sections:
//! - **PRIORITY**: Current highest-priority context (replaced entirely, ≤500 chars)
//! - **WORKING MEMORY**: Timestamped entries for in-progress notes (prunable by age)
//! - **MANUAL**: Permanent notes that are never auto-pruned
//!
//! Storage: `{codex_home}/memories/notepad.md`
//! Writes use atomic rename (`tmp.{pid}` + `rename`) for crash safety.
//! Concurrent access is guarded by a directory-based lock with stale detection.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use tokio::fs;
use tracing::warn;

/// Maximum character length for the PRIORITY section.
const PRIORITY_MAX_CHARS: usize = 500;

/// Default number of days before WORKING MEMORY entries are pruned.
const DEFAULT_PRUNE_DAYS: u64 = 7;

/// Lock acquisition timeout in milliseconds.
const LOCK_TIMEOUT_MS: u64 = 5000;

/// Lock polling interval in milliseconds.
const LOCK_POLL_MS: u64 = 100;

/// Section header markers.
const PRIORITY_HEADER: &str = "## PRIORITY";
const WORKING_HEADER: &str = "## WORKING MEMORY";
const MANUAL_HEADER: &str = "## MANUAL";

/// Notepad section identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotepadSection {
    Priority,
    Working,
    Manual,
}

/// Returns the path to the notepad file.
pub fn notepad_path(root: &Path) -> PathBuf {
    root.join("notepad.md")
}

/// Parse a notepad file into its three sections.
/// Returns `(priority, working, manual)` as raw string content per section.
fn parse_sections(raw: &str) -> (String, String, String) {
    let mut priority = String::new();
    let mut working = String::new();
    let mut manual = String::new();

    let mut current: Option<&mut String> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == PRIORITY_HEADER {
            current = Some(&mut priority);
            continue;
        } else if trimmed == WORKING_HEADER {
            current = Some(&mut working);
            continue;
        } else if trimmed == MANUAL_HEADER {
            current = Some(&mut manual);
            continue;
        }

        if let Some(ref mut section) = current {
            if !section.is_empty() || !line.trim().is_empty() {
                if !section.is_empty() {
                    section.push('\n');
                }
                section.push_str(line);
            }
        }
    }

    (priority.trim_end().to_string(), working.trim_end().to_string(), manual.trim_end().to_string())
}

/// Reconstruct the full notepad content from three section strings.
fn build_notepad_content(priority: &str, working: &str, manual: &str) -> String {
    let mut parts = Vec::new();

    parts.push(PRIORITY_HEADER.to_string());
    if !priority.is_empty() {
        parts.push(priority.to_string());
    }

    parts.push(WORKING_HEADER.to_string());
    if !working.is_empty() {
        parts.push(working.to_string());
    }

    parts.push(MANUAL_HEADER.to_string());
    if !manual.is_empty() {
        parts.push(manual.to_string());
    }

    parts.join("\n\n") + "\n"
}

/// Atomic write: write to a temp file then rename.
/// Ensures the parent directory exists before writing.
async fn atomic_write(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!("creating parent directory: {}", parent.display())
        })?;
    }
    let tmp_path = {
        let pid = std::process::id();
        path.with_file_name(format!(
            ".{}.tmp.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("notepad.md"),
            pid
        ))
    };
    fs::write(&tmp_path, content)
        .await
        .with_context(|| format!("writing temp file: {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .await
        .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Directory-based file locking (inspired by oh-my-codex agents-overlay.ts)
// ---------------------------------------------------------------------------

/// Returns the lock directory path for the notepad under `root`.
fn lock_dir(root: &Path) -> PathBuf {
    root.join(".notepad.lock")
}

/// Acquire a directory-based lock with PID owner metadata and stale detection.
///
/// The lock is a directory created atomically via `mkdir`. If the directory
/// already exists, the owner PID is checked: if the owner process is dead,
/// the stale lock is reaped and we retry.
async fn acquire_lock(root: &Path) -> anyhow::Result<()> {
    let lock = lock_dir(root);
    if let Some(parent) = lock.parent() {
        fs::create_dir_all(parent).await.ok();
    }

    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(LOCK_TIMEOUT_MS);

    while start.elapsed() < timeout {
        // Attempt atomic directory creation.
        match fs::create_dir(&lock).await {
            Ok(()) => {
                // Write owner metadata for stale detection.
                let owner = lock.join("owner.json");
                let meta = format!(r#"{{"pid":{},"ts":{}}}"#, std::process::id(), start.elapsed().as_millis());
                fs::write(&owner, meta).await.ok();
                return Ok(());
            }
            Err(_) => {
                // Lock exists — check if owner is dead.
                let owner_path = lock.join("owner.json");
                if let Ok(raw) = fs::read_to_string(&owner_path).await {
                    if let Some(pid_str) = raw.split(r#""pid":"#).nth(1) {
                        if let Some(end) = pid_str.find(',') {
                            if let Ok(pid) = pid_str[..end].parse::<u32>() {
                                // Check if PID is still alive (Unix: signal 0).
                                #[cfg(unix)]
                                {
                                    if unsafe { libc::kill(pid as i32, 0) } != 0 {
                                        // Owner is dead — reap stale lock.
                                        fs::remove_dir_all(&lock).await.ok();
                                        continue;
                                    }
                                }
                                let _ = pid; // Silence unused warning on non-Unix.
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(LOCK_POLL_MS)).await;
            }
        }
    }

    anyhow::bail!("failed to acquire notepad lock within {}ms", LOCK_TIMEOUT_MS)
}

/// Release the directory-based lock.
async fn release_lock(root: &Path) {
    let lock = lock_dir(root);
    fs::remove_dir_all(&lock).await.ok();
}

/// Execute a closure while holding the notepad lock.
async fn with_notepad_lock<F, Fut, T>(root: &Path, f: F) -> anyhow::Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    acquire_lock(root).await?;
    let result = f().await;
    release_lock(root).await;
    result
}

/// Read the notepad file, optionally filtering to a specific section.
///
/// Returns `None` if the file doesn't exist.
pub async fn read_notepad(root: &Path, section: Option<NotepadSection>) -> Option<String> {
    let path = notepad_path(root);
    let raw = fs::read_to_string(&path).await.ok()?;
    let (priority, working, manual) = parse_sections(&raw);

    match section {
        None => Some(raw),
        Some(NotepadSection::Priority) => {
            if priority.is_empty() {
                None
            } else {
                Some(priority)
            }
        }
        Some(NotepadSection::Working) => {
            if working.is_empty() {
                None
            } else {
                Some(working)
            }
        }
        Some(NotepadSection::Manual) => {
            if manual.is_empty() {
                None
            } else {
                Some(manual)
            }
        }
    }
}

/// Write (replace) the PRIORITY section content.
/// Content is truncated to `PRIORITY_MAX_CHARS` if it exceeds the limit.
/// Acquires a file lock to prevent concurrent writes.
pub async fn write_priority(root: &Path, content: &str) -> anyhow::Result<()> {
    let root = root.to_path_buf();
    let content = content.to_string();
    with_notepad_lock(&root, || {
        let root = root.clone();
        let content = content.clone();
        async move {
            let path = notepad_path(&root);
            let truncated = if content.len() > PRIORITY_MAX_CHARS {
                &content[..content.floor_char_boundary(PRIORITY_MAX_CHARS)]
            } else {
                &content
            };

            let (.., working, manual) = if path.exists() {
                let raw = fs::read_to_string(&path).await.unwrap_or_default();
                parse_sections(&raw)
            } else {
                (String::new(), String::new(), String::new())
            };

            let new_content = build_notepad_content(truncated, &working, &manual);
            atomic_write(&path, &new_content).await
        }
    })
    .await
}

/// Append a timestamped entry to the WORKING MEMORY section.
/// Acquires a file lock to prevent concurrent writes.
pub async fn append_working(root: &Path, entry: &str) -> anyhow::Result<()> {
    let root = root.to_path_buf();
    let entry = entry.to_string();
    with_notepad_lock(&root, || {
        let root = root.clone();
        let entry = entry.clone();
        async move {
            let path = notepad_path(&root);
            let timestamp = Utc::now().to_rfc3339();
            let new_entry = format!("[{timestamp}] {entry}");

            let (priority, mut working, manual) = if path.exists() {
                let raw = fs::read_to_string(&path).await.unwrap_or_default();
                parse_sections(&raw)
            } else {
                (String::new(), String::new(), String::new())
            };

            if !working.is_empty() {
                working.push('\n');
            }
            working.push_str(&new_entry);

            let new_content = build_notepad_content(&priority, &working, &manual);
            atomic_write(&path, &new_content).await
        }
    })
    .await
}

/// Append an entry to the MANUAL section (never auto-pruned).
/// Acquires a file lock to prevent concurrent writes.
pub async fn append_manual(root: &Path, entry: &str) -> anyhow::Result<()> {
    let root = root.to_path_buf();
    let entry = entry.to_string();
    with_notepad_lock(&root, || {
        let root = root.clone();
        let entry = entry.clone();
        async move {
            let path = notepad_path(&root);

            let (priority, working, mut manual) = if path.exists() {
                let raw = fs::read_to_string(&path).await.unwrap_or_default();
                parse_sections(&raw)
            } else {
                (String::new(), String::new(), String::new())
            };

            if !manual.is_empty() {
                manual.push('\n');
            }
            manual.push_str(&entry);

            let new_content = build_notepad_content(&priority, &working, &manual);
            atomic_write(&path, &new_content).await
        }
    })
    .await
}

/// Prune WORKING MEMORY entries older than `max_age_days`.
/// Acquires a file lock to prevent concurrent writes.
///
/// Returns the number of entries removed.
pub async fn prune_working(root: &Path, max_age_days: Option<u64>) -> anyhow::Result<usize> {
    let root = root.to_path_buf();
    let max_days = max_age_days.unwrap_or(DEFAULT_PRUNE_DAYS);
    with_notepad_lock(&root, || {
        let root = root.clone();
        async move {
            let path = notepad_path(&root);

            if !path.exists() {
                return Ok(0);
            }

            let raw = fs::read_to_string(&path).await.unwrap_or_default();
            let (priority, working, manual) = parse_sections(&raw);

            let cutoff = Utc::now() - chrono::Duration::days(max_days as i64);
            let mut retained = String::new();
            let mut removed = 0usize;

            for line in working.lines() {
                let trimmed = line.trim();
                if let Some(ts_end) = trimmed.find("] ") {
                    let ts_str = &trimmed[1..ts_end];
                    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                        if ts.with_timezone(&Utc) < cutoff {
                            removed += 1;
                            continue;
                        }
                    }
                }
                if !retained.is_empty() {
                    retained.push('\n');
                }
                retained.push_str(line);
            }

            if removed > 0 {
                let new_content = build_notepad_content(&priority, &retained, &manual);
                atomic_write(&path, &new_content).await?;
            }

            Ok(removed)
        }
    })
    .await
}

/// Return notepad statistics: file size, entry counts per section, oldest/newest timestamps.
#[derive(Debug, Clone)]
pub struct NotepadStats {
    pub file_size_bytes: u64,
    pub priority_chars: usize,
    pub working_entries: usize,
    pub manual_chars: usize,
    pub oldest_working: Option<String>,
    pub newest_working: Option<String>,
}

/// Compute notepad statistics.
pub async fn notepad_stats(root: &Path) -> anyhow::Result<NotepadStats> {
    let path = notepad_path(root);

    if !path.exists() {
        return Ok(NotepadStats {
            file_size_bytes: 0,
            priority_chars: 0,
            working_entries: 0,
            manual_chars: 0,
            oldest_working: None,
            newest_working: None,
        });
    }

    let meta = fs::metadata(&path).await?;
    let raw = fs::read_to_string(&path).await.unwrap_or_default();
    let (priority, working, manual) = parse_sections(&raw);

    let mut working_entries = 0usize;
    let mut oldest: Option<String> = None;
    let mut newest: Option<String> = None;

    for line in working.lines() {
        let trimmed = line.trim();
        if let Some(ts_end) = trimmed.find("] ") {
            let ts_str = &trimmed[1..ts_end];
            if chrono::DateTime::parse_from_rfc3339(ts_str).is_ok() {
                working_entries += 1;
                let ts = ts_str.to_string();
                if oldest.is_none() {
                    oldest = Some(ts.clone());
                }
                newest = Some(ts);
            }
        }
    }

    Ok(NotepadStats {
        file_size_bytes: meta.len(),
        priority_chars: priority.len(),
        working_entries,
        manual_chars: manual.len(),
        oldest_working: oldest,
        newest_working: newest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_root() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn parse_sections_empty() {
        let (p, w, m) = parse_sections("");
        assert!(p.is_empty());
        assert!(w.is_empty());
        assert!(m.is_empty());
    }

    #[test]
    fn parse_sections_all_present() {
        let raw = "## PRIORITY\nurgent stuff\n\n## WORKING MEMORY\n[2026-01-01T00:00:00Z] note1\n\n## MANUAL\npermanent note";
        let (p, w, m) = parse_sections(raw);
        assert_eq!(p, "urgent stuff");
        assert!(w.contains("note1"));
        assert!(m.contains("permanent note"));
    }

    #[tokio::test]
    async fn write_and_read_priority() {
        let root = test_root();
        write_priority(root.path(), "test priority").await.unwrap();
        let content = read_notepad(root.path(), Some(NotepadSection::Priority))
            .await
            .unwrap();
        assert_eq!(content, "test priority");
    }

    #[tokio::test]
    async fn append_working_entries() {
        let root = test_root();
        append_working(root.path(), "first entry")
            .await
            .unwrap();
        append_working(root.path(), "second entry")
            .await
            .unwrap();
        let content = read_notepad(root.path(), Some(NotepadSection::Working))
            .await
            .unwrap();
        assert!(content.contains("first entry"));
        assert!(content.contains("second entry"));
    }

    #[tokio::test]
    async fn prune_old_working_entries() {
        let root = test_dir_with_old_entries();
        let removed = prune_working(root.path(), Some(0)).await.unwrap();
        // All entries should be pruned with max_age_days=0
        assert!(removed > 0);
    }

    fn test_dir_with_old_entries() -> TempDir {
        let root = TempDir::new().unwrap();
        // Create a notepad with old-style entries
        let content = "## PRIORITY\n\n## WORKING MEMORY\n[2020-01-01T00:00:00Z] old entry\n[2020-06-01T00:00:00Z] another old\n\n## MANUAL\n";
        std::fs::write(root.path().join("notepad.md"), content).unwrap();
        root
    }
}
