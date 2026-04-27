//! Memory directory (memdir) subsystem.
//!
//! This module implements a Claude Code-style topic-based memory layout:
//! memories are stored as individual `.md` files under `topics/`, each with
//! YAML frontmatter. The system can scan, read, and selectively load topics
//! into the agent prompt.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs;
use tracing::warn;

use super::memory_root;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_output_truncation::truncate_text;
use codex_utils_output_truncation::TruncationPolicy;

/// Maximum number of topic files to scan.
const MAX_TOPIC_FILES: usize = 200;
/// Maximum lines for the MEMORY.md entrypoint index.
const MAX_ENTRYPOINT_LINES: usize = 200;
/// Maximum bytes for the MEMORY.md entrypoint index.
const MAX_ENTRYPOINT_BYTES: usize = 25_000;
/// Token budget per individual topic when building the prompt.
const TOPIC_TOKEN_LIMIT: usize = 800;
/// Maximum number of topics to include in the prompt.
const MAX_TOPICS_IN_PROMPT: usize = 8;
/// Age threshold (in seconds) beyond which a freshness warning is appended.
const FRESHNESS_AGE_SECONDS: u64 = 86_400; // 1 day

/// YAML frontmatter for a memory topic file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct MemoryFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default = "default_memory_type")]
    pub r#type: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

fn default_memory_type() -> String {
    "project".to_string()
}

fn default_source() -> String {
    "auto".to_string()
}

/// An in-memory representation of a single topic file.
#[derive(Debug, Clone)]
pub(crate) struct MemoryTopic {
    pub frontmatter: MemoryFrontmatter,
    pub content: String,
    pub path: PathBuf,
    /// File modification time in seconds since epoch (for freshness).
    pub modified_secs: u64,
}

/// Returns the `topics/` subdirectory inside a memory root.
pub(crate) fn topics_dir(root: &Path) -> PathBuf {
    root.join("topics")
}

/// Parse a markdown file that may contain YAML frontmatter delimited by `---`.
///
/// Returns `(frontmatter, body)` on success, or `None` if the file has no
/// frontmatter. Files without frontmatter are treated as plain markdown topics
/// with a default frontmatter derived from the file stem.
pub(crate) fn parse_topic(path: &Path, raw: &str) -> (MemoryFrontmatter, String) {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let yaml = &rest[..end];
            let body = rest[end + 4..].trim_start().to_string();
            match serde_yaml::from_str::<MemoryFrontmatter>(yaml) {
                Ok(frontmatter) => return (frontmatter, body),
                Err(err) => {
                    warn!(
                        ?path,
                        ?err,
                        "failed to parse YAML frontmatter in memory topic"
                    );
                }
            }
        }
    }

    // Fallback: no frontmatter – synthesize one from the file stem.
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let frontmatter = MemoryFrontmatter {
        name: name.clone(),
        description: format!("Memory topic: {name}"),
        r#type: default_memory_type(),
        keywords: Vec::new(),
        source: default_source(),
        updated_at: None,
    };
    (frontmatter, raw.to_string())
}

/// Scan the `topics/` directory under `root` and return all parsed memory
/// topics, sorted by modification time (newest first).
pub(crate) async fn scan_memory_topics(root: &Path) -> Vec<MemoryTopic> {
    let dir = topics_dir(root);
    let mut entries = match fs::read_dir(&dir).await {
        Ok(mut rd) => {
            let mut items = Vec::new();
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                if let Ok(meta) = entry.metadata().await {
                    if !meta.is_file() {
                        continue;
                    }
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if let Ok(raw) = fs::read_to_string(&path).await {
                        let (frontmatter, content) = parse_topic(&path, &raw);
                        items.push(MemoryTopic {
                            frontmatter,
                            content,
                            path,
                            modified_secs: modified,
                        });
                    }
                }
            }
            items
        }
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(?dir, ?err, "failed to read memory topics directory");
            }
            Vec::new()
        }
    };

    entries.sort_by(|a, b| b.modified_secs.cmp(&a.modified_secs));
    entries.truncate(MAX_TOPIC_FILES);
    entries
}

/// Load the `MEMORY.md` entrypoint index, if it exists.
/// The content is truncated to prevent token bloat.
pub(crate) async fn load_memory_index(root: &Path) -> Option<String> {
    let path = root.join("MEMORY.md");
    let raw = fs::read_to_string(&path).await.ok()?;
    let truncated = truncate_entrypoint(&raw);
    if truncated.is_empty() {
        return None;
    }
    Some(truncated)
}

/// Truncate entrypoint content to line + byte caps.
fn truncate_entrypoint(raw: &str) -> String {
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > MAX_ENTRYPOINT_LINES {
        lines.truncate(MAX_ENTRYPOINT_LINES);
        lines.push("\n... (truncated)\n");
    }
    let joined = lines.join("\n");
    if joined.len() > MAX_ENTRYPOINT_BYTES {
        let mut cut = MAX_ENTRYPOINT_BYTES;
        while cut > 0 && !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        let prefix = &joined[..cut];
        let last_newline = prefix.rfind('\n').unwrap_or(cut);
        format!("{}\n\n... (truncated)\n", &prefix[..last_newline])
    } else {
        joined
    }
}

/// Compute a simple relevance score between a topic and the current user
/// message (or empty string if no message is available yet).
///
/// This is a lightweight placeholder. A future upgrade can issue a side-query
/// to a model for semantic relevance ranking.
pub(crate) fn relevance_score(topic: &MemoryTopic, query: &str) -> usize {
    if query.is_empty() {
        return 1; // Neutral score when no query supplied.
    }
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    let mut score = 0usize;

    // Name match.
    let name_lower = topic.frontmatter.name.to_lowercase();
    if name_lower.contains(&query_lower) {
        score += 10;
    }
    for w in &query_words {
        if name_lower.contains(w) {
            score += 3;
        }
    }

    // Description match.
    let desc_lower = topic.frontmatter.description.to_lowercase();
    if desc_lower.contains(&query_lower) {
        score += 8;
    }
    for w in &query_words {
        if desc_lower.contains(w) {
            score += 2;
        }
    }

    // Keyword match.
    for kw in &topic.frontmatter.keywords {
        let kw_lower = kw.to_lowercase();
        if query_lower.contains(&kw_lower) {
            score += 5;
        }
        for w in &query_words {
            if kw_lower.contains(w) || w.contains(&kw_lower) {
                score += 2;
            }
        }
    }

    // Content match (lightweight).
    let content_lower = topic.content.to_lowercase();
    for w in &query_words {
        if content_lower.contains(w) {
            score += 1;
        }
    }

    score
}

/// Build the memory content string that gets injected into the system prompt.
///
/// Strategy:
/// 1. Always include `MEMORY.md` index (truncated).
/// 2. If `topics/` exists and is non-empty, load the most relevant topics
///    (up to `MAX_TOPICS_IN_PROMPT`), truncating each to `TOPIC_TOKEN_LIMIT`.
/// 3. If `topics/` is empty, fall back to the legacy `memory_summary.md`.
/// 4. Append freshness warnings for topics older than 1 day.
pub(crate) async fn build_memory_prompt_content(
    codex_home: &AbsolutePathBuf,
    query: &str,
) -> Option<String> {
    let root = memory_root(codex_home);

    let mut parts: Vec<String> = Vec::new();

    // 1. Index.
    if let Some(index) = load_memory_index(&root).await {
        parts.push(format!("## Memory Index (MEMORY.md)\n{index}"));
    }

    // 2. Topics.
    let topics = scan_memory_topics(&root).await;
    if !topics.is_empty() {
        let mut scored: Vec<_> = topics
            .into_iter()
            .map(|t| {
                let score = relevance_score(&t, query);
                (score, t)
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(MAX_TOPICS_IN_PROMPT);

        for (score, topic) in scored {
            let truncated = truncate_text(
                &topic.content,
                TruncationPolicy::Tokens(TOPIC_TOKEN_LIMIT),
            );
            let freshness = if topic.modified_secs > 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if now.saturating_sub(topic.modified_secs) > FRESHNESS_AGE_SECONDS {
                    memory_freshness_text(topic.modified_secs)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            parts.push(format!(
                "## Topic: {}\nType: {} | Source: {} | Score: {score}\n{freshness}{}",
                topic.frontmatter.name,
                topic.frontmatter.r#type,
                topic.frontmatter.source,
                truncated,
            ));
        }
    } else {
        // 3. Legacy fallback: memory_summary.md.
        let summary_path = root.join("memory_summary.md");
        if let Ok(summary) = fs::read_to_string(&summary_path).await {
            let summary = truncate_text(
                summary.trim(),
                TruncationPolicy::Tokens(super::phase_one::MEMORY_TOOL_DEVELOPER_INSTRUCTIONS_SUMMARY_TOKEN_LIMIT),
            );
            if !summary.is_empty() {
                parts.push(format!("## Legacy Memory Summary\n{summary}"));
            }
        }
    }

    if parts.is_empty() {
        return None;
    }

    Some(parts.join("\n\n"))
}

/// Freshness warning text for stale memories.
pub(crate) fn memory_freshness_text(modified_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now.saturating_sub(modified_secs) / FRESHNESS_AGE_SECONDS;
    format!(
        "<system-reminder>This memory is {days} day(s) old. \
         Memories are point-in-time observations, not live state — \
         claims about code behavior may be outdated. \
         Verify against current code before asserting as fact.</system-reminder>\n\n"
    )
}

// ---------------------------------------------------------------------------
// User-facing CRUD helpers (used by slash commands)
// ---------------------------------------------------------------------------

/// Write (or overwrite) a topic file.
pub(crate) async fn write_topic(
    root: &Path,
    name: &str,
    frontmatter: &MemoryFrontmatter,
    content: &str,
) -> anyhow::Result<PathBuf> {
    let dir = topics_dir(root);
    fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating topics dir: {}", dir.display()))?;

    let safe_name = name
        .to_lowercase()
        .replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");
    let file_name = format!("{safe_name}.md");
    let path = dir.join(&file_name);

    let yaml = serde_yaml::to_string(frontmatter).context("serializing frontmatter")?;
    let output = format!("---\n{yaml}---\n\n{content}\n");
    fs::write(&path, output)
        .await
        .with_context(|| format!("writing topic file: {}", path.display()))?;
    Ok(path)
}

/// List names of all topic files under the memory root.
pub(crate) async fn list_topics(root: &Path) -> Vec<String> {
    let topics = scan_memory_topics(root).await;
    topics
        .into_iter()
        .map(|t| t.frontmatter.name)
        .collect()
}

/// Remove all `.md` files under the `topics/` directory.
pub(crate) async fn clear_topics(root: &Path) -> anyhow::Result<()> {
    let dir = topics_dir(root);
    let mut rd = match fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Err(err) = fs::remove_file(&path).await {
                warn!(?path, ?err, "failed to remove topic file during clear");
            }
        }
    }
    Ok(())
}
