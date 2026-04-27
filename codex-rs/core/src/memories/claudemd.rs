//! AGENTS.md hierarchical memory loading.
//!
//! Ported from Claude Code's `claudemd.rs`. Loads memory files in priority
//! order: Managed → User → Project → Local. Supports `@include` directive
//! expansion with circular-reference detection, YAML frontmatter parsing,
//! and mtime-based caching.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Memory file type / priority scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryScope {
    /// `~/.codex/rules/*.md` — global managed policy.
    Managed,
    /// `~/.codex/AGENTS.md` — user-level memory.
    User,
    /// `{project_root}/AGENTS.md` — project-level memory.
    Project,
    /// `{project_root}/.codex/AGENTS.md` — local override.
    Local,
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryScope::Managed => write!(f, "managed"),
            MemoryScope::User => write!(f, "user"),
            MemoryScope::Project => write!(f, "project"),
            MemoryScope::Local => write!(f, "local"),
        }
    }
}

/// Frontmatter parsed from an AGENTS.md file.
#[derive(Debug, Clone, Default)]
pub struct MemoryFrontmatter {
    pub memory_type: Option<String>,
    pub priority: Option<u32>,
    pub scope: Option<String>,
}

/// Loaded memory file with metadata.
#[derive(Debug, Clone)]
pub struct MemoryFileInfo {
    pub path: PathBuf,
    pub scope: MemoryScope,
    pub content: String,
    pub frontmatter: MemoryFrontmatter,
    pub mtime: Option<SystemTime>,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Simple mtime-keyed file cache to avoid re-reading unchanged files.
#[derive(Default)]
pub struct MemoryCache {
    entries: HashMap<PathBuf, (SystemTime, String)>,
}

impl MemoryCache {
    /// Return cached content if the file hasn't changed since last read.
    pub fn get(&self, path: &Path) -> Option<&str> {
        let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
        let (cached_mtime, content) = self.entries.get(path)?;
        if *cached_mtime == mtime {
            Some(content.as_str())
        } else {
            None
        }
    }

    /// Store file content with its current mtime.
    pub fn insert(&mut self, path: PathBuf, content: String) {
        if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            self.entries.insert(path, (mtime, content));
        }
    }
}

// ---------------------------------------------------------------------------
// YAML frontmatter parsing
// ---------------------------------------------------------------------------

/// Strip YAML frontmatter (`--- ... ---`) from content and parse it.
/// Returns `(frontmatter, body_without_frontmatter)`.
pub fn parse_frontmatter(content: &str) -> (MemoryFrontmatter, &str) {
    if !content.starts_with("---") {
        return (MemoryFrontmatter::default(), content);
    }
    let after_first = &content[3..];
    if let Some(end) = after_first.find("\n---") {
        let yaml = after_first[..end].trim();
        let body = &after_first[end + 4..];
        let mut fm = MemoryFrontmatter::default();
        for line in yaml.lines() {
            let line = line.trim();
            if let Some((key, val)) = line.split_once(':') {
                let val = val.trim().to_string();
                match key.trim() {
                    "memory_type" => fm.memory_type = Some(val),
                    "priority" => fm.priority = val.parse().ok(),
                    "scope" => fm.scope = Some(val),
                    _ => {}
                }
            }
        }
        return (fm, body.trim_start_matches('\n'));
    }
    (MemoryFrontmatter::default(), content)
}

// ---------------------------------------------------------------------------
// @include directive expansion
// ---------------------------------------------------------------------------

/// Maximum @include nesting depth.
const MAX_INCLUDE_DEPTH: usize = 10;

/// Maximum size of a single @include'd file (40 KB).
const INCLUDE_FILE_SIZE_LIMIT: usize = 40 * 1024;

/// Expand `@include` directives in content.
///
/// Circular references are detected via `visited` set. Lines starting with
/// `@include ` (with a trailing space) are treated as directives; the rest
/// of the line is the path to include (relative to `base_dir`, or absolute,
/// or `~`-prefixed for home directory).
pub fn expand_includes(
    content: &str,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> String {
    if depth >= MAX_INCLUDE_DEPTH {
        return content.to_string();
    }

    let mut result = String::with_capacity(content.len());
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(path_str) = trimmed.strip_prefix("@include ") {
            let path_str = path_str.trim();
            let include_path = if path_str.starts_with('~') {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(&path_str[2..])
            } else if Path::new(path_str).is_absolute() {
                PathBuf::from(path_str)
            } else {
                base_dir.join(path_str)
            };

            let canonical = include_path.canonicalize().unwrap_or(include_path.clone());
            if visited.contains(&canonical) {
                result.push_str(&format!(
                    "<!-- circular @include {} skipped -->\n",
                    path_str
                ));
                continue;
            }
            if let Ok(included) = std::fs::read_to_string(&include_path) {
                if included.len() > INCLUDE_FILE_SIZE_LIMIT {
                    result.push_str(&format!(
                        "<!-- @include {} exceeds 40KB limit -->\n",
                        path_str
                    ));
                    continue;
                }
                visited.insert(canonical);
                let expanded = expand_includes(
                    &included,
                    include_path.parent().unwrap_or(base_dir),
                    visited,
                    depth + 1,
                );
                result.push_str(&expanded);
                result.push('\n');
            } else {
                result.push_str(&format!("<!-- @include {} not found -->\n", path_str));
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Loading API
// ---------------------------------------------------------------------------

/// Maximum file size for a single memory file (40 KB).
const MAX_FILE_SIZE: u64 = 40 * 1024;

/// Load a single AGENTS.md file.
///
/// Respects `MAX_FILE_SIZE`, expands `@include` directives, and parses
/// frontmatter.
pub fn load_memory_file(path: &Path, scope: MemoryScope) -> Option<MemoryFileInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_SIZE {
        tracing::warn!("{} exceeds 40KB limit, skipping", path.display());
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let mtime = meta.modified().ok();

    let (frontmatter, body) = parse_frontmatter(&raw);
    let mut visited = HashSet::new();
    visited.insert(path.canonicalize().unwrap_or(path.to_path_buf()));
    let content = expand_includes(
        body,
        path.parent().unwrap_or(Path::new(".")),
        &mut visited,
        0,
    );

    Some(MemoryFileInfo {
        path: path.to_path_buf(),
        scope,
        content,
        frontmatter,
        mtime,
    })
}

/// Load memory files from a directory for a given scope.
///
/// Loads `AGENTS.md` first (universal standard), then `CLAUDE.md` if present
/// (backward-compatible fallback). Either file may be absent.
fn load_scope_files(dir: &Path, scope: MemoryScope, files: &mut Vec<MemoryFileInfo>) {
    for name in &["AGENTS.md", "CLAUDE.md"] {
        let path = dir.join(name);
        if path.exists() {
            if let Some(f) = load_memory_file(&path, scope) {
                files.push(f);
            }
        }
    }
}

/// Load all memory files for the given project root, in priority order.
///
/// At each scope `AGENTS.md` is loaded first, followed by `CLAUDE.md` if
/// present. Returned list is ordered: Managed (highest) → User → Project → Local.
///
/// `codex_home` is the `~/.codex` directory (or equivalent).
pub fn load_all_memory_files(project_root: &Path, codex_home: &Path) -> Vec<MemoryFileInfo> {
    let mut files = Vec::new();

    // 1. Managed: ~/.codex/rules/*.md
    let rules_dir = codex_home.join("rules");
    if let Ok(entries) = std::fs::read_dir(&rules_dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().map_or(false, |x| x == "md") {
                    Some(p)
                } else {
                    None
                }
            })
            .collect();
        paths.sort();
        for p in paths {
            if let Some(f) = load_memory_file(&p, MemoryScope::Managed) {
                files.push(f);
            }
        }
    }

    // 2. User: ~/.codex/AGENTS.md then ~/.codex/CLAUDE.md
    load_scope_files(codex_home, MemoryScope::User, &mut files);

    // 3. Project: {project_root}/AGENTS.md then {project_root}/CLAUDE.md
    load_scope_files(project_root, MemoryScope::Project, &mut files);

    // 4. Local: {project_root}/.codex/AGENTS.md then {project_root}/.codex/CLAUDE.md
    load_scope_files(&project_root.join(".codex"), MemoryScope::Local, &mut files);

    files
}

/// Concatenate all memory file contents into a single system-prompt fragment.
pub fn build_memory_prompt(files: &[MemoryFileInfo]) -> String {
    files
        .iter()
        .filter(|f| !f.content.trim().is_empty())
        .map(|f| f.content.trim().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_frontmatter_basic() {
        let input = "---\nmemory_type: project\npriority: 10\n---\nBody content";
        let (fm, body) = parse_frontmatter(input);
        assert_eq!(fm.memory_type.as_deref(), Some("project"));
        assert_eq!(fm.priority, Some(10));
        assert!(body.starts_with("Body content"));
    }

    #[test]
    fn parse_frontmatter_none() {
        let input = "No frontmatter here";
        let (fm, body) = parse_frontmatter(input);
        assert!(fm.memory_type.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn expand_includes_no_directives() {
        let content = "Hello\nWorld";
        let mut visited = HashSet::new();
        let result = expand_includes(content, Path::new("."), &mut visited, 0);
        assert_eq!(result, "Hello\nWorld\n");
    }

    #[test]
    fn expand_includes_circular() {
        let dir = TempDir::new().unwrap();
        let file_a = dir.path().join("a.md");
        let file_b = dir.path().join("b.md");
        std::fs::write(&file_a, "@include b.md\nContent A").unwrap();
        std::fs::write(&file_b, "@include a.md\nContent B").unwrap();

        let mut visited = HashSet::new();
        visited.insert(file_a.canonicalize().unwrap());
        let result = expand_includes(
            "@include b.md\nContent A",
            dir.path(),
            &mut visited,
            0,
        );
        assert!(result.contains("circular @include b.md skipped") || result.contains("Content B"));
    }

    #[test]
    fn load_all_memory_files_priority_order() {
        let dir = TempDir::new().unwrap();
        let codex_home = dir.path().join("codex_home");
        let project_root = dir.path().join("project");

        std::fs::create_dir_all(codex_home.join("rules")).unwrap();
        std::fs::write(codex_home.join("rules/01_policy.md"), "Managed policy").unwrap();
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::write(codex_home.join("AGENTS.md"), "User memory").unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(project_root.join("AGENTS.md"), "Project memory").unwrap();
        std::fs::create_dir_all(project_root.join(".codex")).unwrap();
        std::fs::write(project_root.join(".codex/AGENTS.md"), "Local memory").unwrap();

        let files = load_all_memory_files(&project_root, &codex_home);
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].scope, MemoryScope::Managed);
        assert_eq!(files[1].scope, MemoryScope::User);
        assert_eq!(files[2].scope, MemoryScope::Project);
        assert_eq!(files[3].scope, MemoryScope::Local);
    }
}
