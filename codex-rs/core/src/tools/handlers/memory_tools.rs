//! Built-in memory CRUD tools for the agent.
//!
//! Inspired by oh-my-codex's MCP memory tools. Provides the agent with
//! active read/write access to the memory subsystem, including topic files
//! and the notepad.

use chrono::Utc;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::memories;
use crate::memories::notepad;
use crate::memories::notepad::NotepadSection;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

// ---------------------------------------------------------------------------
// memory_read
// ---------------------------------------------------------------------------

pub struct MemoryReadHandler;

#[derive(Deserialize)]
struct MemoryReadArgs {
    /// Optional query for relevance scoring of topics.
    #[serde(default)]
    query: Option<String>,
}

impl ToolHandler for MemoryReadHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "memory_read handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: MemoryReadArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let query = args.query.as_deref().unwrap_or("");

        let root = crate::memories::memory_root(codex_home);

        // Load MEMORY.md index.
        let mut parts = Vec::new();
        if let Some(index) = memories::load_memory_index(&root).await {
            parts.push(format!("## Memory Index\n{index}"));
        }

        // Load relevant topics.
        let topics = memories::scan_memory_topics(&root).await;
        if !topics.is_empty() {
            let mut scored: Vec<_> = topics
                .into_iter()
                .map(|t| (memories::relevance_score(&t, query), t))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.truncate(8);

            for (score, topic) in scored {
                parts.push(format!(
                    "## Topic: {} (type: {}, score: {})\n{}",
                    topic.frontmatter.name,
                    topic.frontmatter.r#type,
                    score,
                    topic.content
                ));
            }
        }

        // Load notepad priority.
        if let Some(priority) = notepad::read_notepad(&root, Some(NotepadSection::Priority)).await
        {
            parts.push(format!("## Notepad Priority\n{}", priority));
        }

        if parts.is_empty() {
            Ok(FunctionToolOutput::from_text(
                "No memories found. Use `memory_write` to create a topic or `notepad_write_priority` to set priority context.".to_string(),
                Some(true),
            ))
        } else {
            Ok(FunctionToolOutput::from_text(parts.join("\n\n"), Some(true)))
        }
    }
}

// ---------------------------------------------------------------------------
// memory_write
// ---------------------------------------------------------------------------

pub struct MemoryWriteHandler;

#[derive(Deserialize)]
struct MemoryWriteArgs {
    /// Topic name (used as filename).
    name: String,
    /// Short description for relevance scoring.
    #[serde(default)]
    description: Option<String>,
    /// Topic type: user, feedback, project, reference.
    #[serde(default = "default_memory_type")]
    #[serde(rename = "type")]
    memory_type: String,
    /// Keywords for relevance scoring.
    #[serde(default)]
    keywords: Option<Vec<String>>,
    /// Topic content (markdown).
    content: String,
    /// Merge with existing topic (true) or replace entirely (false, default).
    #[serde(default)]
    merge: bool,
}

fn default_memory_type() -> String {
    "project".to_string()
}

impl ToolHandler for MemoryWriteHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "memory_write handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: MemoryWriteArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        let (frontmatter, content) = if args.merge {
            // Merge: read existing topic, keep its frontmatter fields unless overridden.
            let topics = crate::memories::scan_memory_topics(&root).await;
            let existing = topics.into_iter().find(|t| {
                t.frontmatter.name.eq_ignore_ascii_case(&args.name)
            });

            if let Some(existing) = existing {
                let merged_name = if args.name.is_empty() { existing.frontmatter.name.clone() } else { args.name.clone() };
                let merged_desc = args.description.as_deref()
                    .filter(|d| !d.is_empty())
                    .unwrap_or(&existing.frontmatter.description)
                    .to_string();
                let merged_type = if args.memory_type == "project" && existing.frontmatter.r#type != "project" {
                    existing.frontmatter.r#type.clone()
                } else {
                    args.memory_type.clone()
                };
                let merged_keywords = match args.keywords.as_ref() {
                    Some(k) if !k.is_empty() => k.clone(),
                    _ => existing.frontmatter.keywords.clone(),
                };
                let merged_priority = existing.frontmatter.priority.clone();

                let merged_content = format!("{}\n\n{}", existing.content.trim_end(), args.content.trim());

                let fm = memories::MemoryFrontmatter {
                    name: merged_name,
                    description: merged_desc,
                    r#type: merged_type,
                    keywords: merged_keywords,
                    source: "agent".to_string(),
                    priority: merged_priority,
                    updated_at: Some(Utc::now()),
                };
                (fm, merged_content)
            } else {
                // No existing topic — fall through to create.
                let fm = memories::MemoryFrontmatter {
                    name: args.name.clone(),
                    description: args.description.unwrap_or_else(|| args.name.clone()),
                    r#type: args.memory_type.clone(),
                    keywords: args.keywords.unwrap_or_default(),
                    source: "agent".to_string(),
                    priority: None,
                    updated_at: Some(Utc::now()),
                };
                (fm, args.content.clone())
            }
        } else {
            let fm = memories::MemoryFrontmatter {
                name: args.name.clone(),
                description: args.description.unwrap_or_else(|| args.name.clone()),
                r#type: args.memory_type.clone(),
                keywords: args.keywords.unwrap_or_default(),
                source: "agent".to_string(),
                priority: None,
                updated_at: Some(Utc::now()),
            };
            (fm, args.content.clone())
        };

        let path = memories::write_topic(&root, &args.name, &frontmatter, &content)
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("failed to write memory topic: {e}"))
            })?;

        let action = if args.merge { "merged into" } else { "written to" };
        Ok(FunctionToolOutput::from_text(
            format!("Memory topic '{}' {} {}", args.name, action, path.display()),
            Some(true),
        ))
    }
}

// ---------------------------------------------------------------------------
// memory_add_note
// ---------------------------------------------------------------------------

pub struct MemoryAddNoteHandler;

#[derive(Deserialize)]
struct MemoryAddNoteArgs {
    /// Topic name to append the note to.
    topic: String,
    /// Note content.
    note: String,
}

impl ToolHandler for MemoryAddNoteHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "memory_add_note handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: MemoryAddNoteArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        let topics = memories::scan_memory_topics(&root).await;
        let matching = topics
            .into_iter()
            .find(|t| t.frontmatter.name.to_lowercase() == args.topic.to_lowercase());

        let timestamp = Utc::now().to_rfc3339();
        let note_with_ts = format!("\n[{timestamp}] {}", args.note);

        if let Some(topic) = matching {
            let updated_content = format!("{}\n{}", topic.content.trim_end(), note_with_ts);
            let mut frontmatter = topic.frontmatter.clone();
            frontmatter.updated_at = Some(Utc::now());

            let path = memories::write_topic(&root, &args.topic, &frontmatter, &updated_content)
                .await
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to append note to memory topic: {e}"
                    ))
                })?;

            Ok(FunctionToolOutput::from_text(
                format!("Note appended to topic '{}' at {}", args.topic, path.display()),
                Some(true),
            ))
        } else {
            // Create a new topic with the note as content.
            let frontmatter = memories::MemoryFrontmatter {
                name: args.topic.clone(),
                description: format!("Notes for {}", args.topic),
                r#type: "project".to_string(),
                keywords: Vec::new(),
                source: "agent".to_string(),
                priority: None,
                updated_at: Some(Utc::now()),
            };
            let content = note_with_ts.trim_start().to_string();

            let path = memories::write_topic(&root, &args.topic, &frontmatter, &content)
                .await
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to create memory topic with note: {e}"
                    ))
                })?;

            Ok(FunctionToolOutput::from_text(
                format!(
                    "Created new topic '{}' with note at {}",
                    args.topic,
                    path.display()
                ),
                Some(true),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// memory_search
// ---------------------------------------------------------------------------

pub struct MemorySearchHandler;

#[derive(Deserialize)]
struct MemorySearchArgs {
    /// Search query.
    query: String,
    /// Maximum number of results (default 5, max 20).
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

impl ToolHandler for MemorySearchHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "memory_search handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: MemorySearchArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        let limit = args.limit.min(20);
        let topics = memories::scan_memory_topics(&root).await;

        if topics.is_empty() {
            return Ok(FunctionToolOutput::from_text(
                "No memory topics found.".to_string(),
                Some(true),
            ));
        }

        let mut scored: Vec<_> = topics
            .into_iter()
            .map(|t| (memories::relevance_score(&t, &args.query), t))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(limit);

        let mut results = Vec::with_capacity(scored.len());
        for (score, topic) in scored {
            results.push(format!(
                "- **{}** (type: {}, score: {}): {}",
                topic.frontmatter.name,
                topic.frontmatter.r#type,
                score,
                topic.frontmatter.description
            ));
        }

        Ok(FunctionToolOutput::from_text(
            format!("Found {} matching topics:\n{}", results.len(), results.join("\n")),
            Some(true),
        ))
    }
}

// ---------------------------------------------------------------------------
// notepad_read
// ---------------------------------------------------------------------------

pub struct NotepadReadHandler;

#[derive(Deserialize)]
struct NotepadReadArgs {
    /// Optional section filter: "priority", "working", "manual".
    #[serde(default)]
    section: Option<String>,
}

impl ToolHandler for NotepadReadHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "notepad_read handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: NotepadReadArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        let section = args.section.as_deref().and_then(|s| match s.to_lowercase().as_str() {
            "priority" => Some(NotepadSection::Priority),
            "working" => Some(NotepadSection::Working),
            "manual" => Some(NotepadSection::Manual),
            _ => None,
        });

        match notepad::read_notepad(&root, section).await {
            Some(content) => Ok(FunctionToolOutput::from_text(content, Some(true))),
            None => Ok(FunctionToolOutput::from_text(
                "Notepad is empty or the requested section has no content.".to_string(),
                Some(true),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// notepad_write_priority
// ---------------------------------------------------------------------------

pub struct NotepadWritePriorityHandler;

#[derive(Deserialize)]
struct NotepadWritePriorityArgs {
    /// Priority content (max 500 chars, will be truncated).
    content: String,
}

impl ToolHandler for NotepadWritePriorityHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "notepad_write_priority handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: NotepadWritePriorityArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        notepad::write_priority(&root, &args.content)
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "failed to write notepad priority: {e}"
                ))
            })?;

        Ok(FunctionToolOutput::from_text(
            "Notepad priority section updated.".to_string(),
            Some(true),
        ))
    }
}

// ---------------------------------------------------------------------------
// notepad_write_working
// ---------------------------------------------------------------------------

pub struct NotepadWriteWorkingHandler;

#[derive(Deserialize)]
struct NotepadWriteWorkingArgs {
    /// Working memory entry.
    entry: String,
}

impl ToolHandler for NotepadWriteWorkingHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "notepad_write_working handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: NotepadWriteWorkingArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        notepad::append_working(&root, &args.entry)
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "failed to append to notepad working memory: {e}"
                ))
            })?;

        Ok(FunctionToolOutput::from_text(
            "Entry added to notepad working memory.".to_string(),
            Some(true),
        ))
    }
}

// ---------------------------------------------------------------------------
// notepad_prune
// ---------------------------------------------------------------------------

pub struct NotepadPruneHandler;

#[derive(Deserialize)]
struct NotepadPruneArgs {
    /// Maximum age in days for working memory entries (default: 7).
    #[serde(default)]
    max_age_days: Option<u64>,
}

impl ToolHandler for NotepadPruneHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "notepad_prune handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: NotepadPruneArgs = parse_arguments(&arguments)?;
        let codex_home = &invocation.turn.config.codex_home;
        let root = crate::memories::memory_root(codex_home);

        let removed = notepad::prune_working(&root, args.max_age_days)
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("failed to prune notepad: {e}"))
            })?;

        Ok(FunctionToolOutput::from_text(
            format!("Pruned {removed} old working memory entries."),
            Some(true),
        ))
    }
}
