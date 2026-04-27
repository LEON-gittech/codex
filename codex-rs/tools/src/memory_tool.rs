//! Tool spec definitions for memory and notepad built-in tools.

use crate::JsonSchema;
use crate::ResponsesApiTool;
use crate::ToolSpec;
use std::collections::BTreeMap;

pub fn create_memory_read_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some(
                "Optional search query for relevance scoring of memory topics.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_read".to_string(),
        description:
            "Read memories: the MEMORY.md index, relevant topic files, and notepad priority section. \
             Optionally provide a query to prioritize the most relevant topics."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    })
}

pub fn create_memory_write_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "name".to_string(),
            JsonSchema::string(Some(
                "Topic name (used as the filename, e.g. 'architecture').".to_string(),
            )),
        ),
        (
            "description".to_string(),
            JsonSchema::string(Some(
                "Short description for relevance scoring.".to_string(),
            )),
        ),
        (
            "type".to_string(),
            JsonSchema::string(Some(
                "Topic type: user, feedback, project, or reference. Default: project.".to_string(),
            )),
        ),
        (
            "keywords".to_string(),
            JsonSchema::array(
                JsonSchema::string(Some("Keyword for relevance scoring.".to_string())),
                Some("Keywords for relevance scoring.".to_string()),
            ),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some("Topic content in markdown.".to_string())),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_write".to_string(),
        description:
            "Write or update a memory topic file. Creates a new topic or overwrites an existing one \
             with the same name."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["name".to_string(), "content".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_memory_add_note_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "topic".to_string(),
            JsonSchema::string(Some(
                "Topic name to append the note to. Creates the topic if it doesn't exist."
                    .to_string(),
            )),
        ),
        (
            "note".to_string(),
            JsonSchema::string(Some("Note content to append (timestamped automatically).".to_string())),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_add_note".to_string(),
        description:
            "Append a timestamped note to an existing memory topic. Creates the topic if it doesn't exist."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["topic".to_string(), "note".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_memory_search_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some("Search query for finding relevant memory topics.".to_string())),
        ),
        (
            "limit".to_string(),
            JsonSchema::number(Some(
                "Maximum number of results to return (default: 5, max: 20).".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_search".to_string(),
        description:
            "Search memory topics by query. Returns matching topic names, types, and descriptions \
             ranked by relevance."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["query".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_notepad_read_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "section".to_string(),
            JsonSchema::string(Some(
                "Optional section filter: 'priority', 'working', or 'manual'. Returns all sections if omitted.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "notepad_read".to_string(),
        description:
            "Read the notepad. The notepad has three sections: PRIORITY (current highest-priority context), \
             WORKING MEMORY (timestamped session notes), and MANUAL (permanent notes)."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    })
}

pub fn create_notepad_write_priority_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "content".to_string(),
            JsonSchema::string(Some(
                "Priority content to set (max 500 chars, will be truncated).".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "notepad_write_priority".to_string(),
        description:
            "Set the notepad PRIORITY section. Replaces any existing priority content. \
             Use this for the current highest-priority context that should be front of mind."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["content".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_notepad_write_working_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "entry".to_string(),
            JsonSchema::string(Some(
                "Working memory entry to add (timestamped automatically).".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "notepad_write_working".to_string(),
        description:
            "Append a timestamped entry to the notepad WORKING MEMORY section. \
             Use this for in-progress observations, decisions, and checkpoints."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["entry".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub fn create_notepad_prune_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "max_age_days".to_string(),
            JsonSchema::number(Some(
                "Maximum age in days for working memory entries (default: 7).".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "notepad_prune".to_string(),
        description:
            "Prune old entries from the notepad WORKING MEMORY section. Entries older than \
             max_age_days are removed. MANUAL and PRIORITY sections are never pruned."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    })
}
