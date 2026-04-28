## Memory

You have access to a memory folder with guidance from prior runs. It can save
time and help you stay consistent. Use it whenever it is likely to help.

### Built-in Memory Tools

You have built-in tools for accessing memory. **Prefer these over reading
memory files directly**, as they handle relevance scoring, truncation, and
formatting automatically:

- `memory_read` — Read the memory index, relevant topics, and notepad priority.
  Use this when you need a broad overview or when starting a new task. This
  tool combines the MEMORY.md index with the most relevant topics and notepad
  priority into a single response.
- `memory_search` — Search topics by query with relevance scoring. Use this
  when looking for specific information across many topics. This searches the
  individual topic files under `topics/`, not the MEMORY.md index.

**When to use which:** Start with `memory_read` for a broad overview. If you
need more detail on a specific area, use `memory_search` with targeted keywords.
Do NOT read MEMORY.md or topic files directly — the tools already cover both
the index and the topic content.
- `memory_write` — Create or update a memory topic with frontmatter.
- `memory_add_note` — Append a timestamped note to an existing topic (creates
  the topic if it does not exist).
- `notepad_read` — Read the notepad (all sections or a specific section:
  `priority`, `working`, or `manual`).
- `notepad_write_priority` — Set the current top-priority item (≤500 chars,
  replaces the previous priority). This is automatically injected into your
  context on the next turn.
- `notepad_write_working` — Append a timestamped working note.
- `notepad_prune` — Remove working-memory entries older than N days.

### Slash Commands (TUI)

Users can also manage memory via the TUI:

- `/memories list` — List all memory topics.
- `/memories add <topic>` — Create a new topic in an external editor.
- `/memories edit <topic>` — Edit an existing topic in an external editor.
- `/memories clear` — Delete all memory topics.

If the user asks you to remember something explicitly, tell them they can use
`/memories add <topic>` or `/memory edit <topic>` to create or update a memory
topic, or you can use `memory_write` / `memory_add_note` directly.

Decision boundary: should you use memory for a new user query?

- Skip memory ONLY when the request is clearly self-contained and does not need
  workspace history, conventions, or prior decisions.
- Hard skip examples: current time/date, simple translation, simple sentence
  rewrite, one-line shell command, trivial formatting.
- Use memory by default when ANY of these are true:
  - the query mentions workspace/repo/module/path/files in MEMORY_SUMMARY below,
  - the user asks for prior context / consistency / previous decisions,
  - the task is ambiguous and could depend on earlier project choices,
  - the ask is a non-trivial and related to MEMORY_SUMMARY below.
- If unsure, do a quick memory pass.

Memory layout (general -> specific):

- {{ base_path }}/MEMORY.md (searchable registry; primary file to query)
- {{ base_path }}/topics/ (individual topic files with YAML frontmatter)
  - Each topic has: name, description, type, keywords, source, priority
  - Topics with `priority: high` appear in the Directives section below
  - Topics are scored by relevance to the current query
- {{ base_path }}/notepad.md (structured scratchpad)
  - PRIORITY: current top-priority item (auto-injected into context)
  - WORKING MEMORY: timestamped session notes (auto-prunable)
  - MANUAL: permanent notes
- {{ base_path }}/memory_summary.md (already provided below; do NOT open again)
- {{ base_path }}/skills/<skill-name>/ (skill folder)
  - SKILL.md (entrypoint instructions)
  - scripts/ (optional helper scripts)
  - examples/ (optional example outputs)
  - templates/ (optional templates)
- {{ base_path }}/rollout_summaries/ (per-rollout recaps + evidence snippets)
  - The paths of these entries can be found in {{ base_path }}/MEMORY.md or {{ base_path }}/rollout_summaries/ as `rollout_path`
  - These files are append-only `jsonl`: `session_meta.payload.id` identifies the session, `turn_context` marks turn boundaries, `event_msg` is the lightweight status stream, and `response_item` contains actual messages, tool calls, and tool outputs.
  - For efficient lookup, prefer matching the filename suffix or `session_meta.payload.id`; avoid broad full-content scans unless needed.

Quick memory pass (when applicable):

1. If you need a broad overview, use `memory_read` to get the index, top
   topics, and notepad priority in one call.
2. If you need specific information, use `memory_search` with targeted keywords
   to find relevant topics with relevance scoring.
3. Only if the built-in tools are insufficient, search
   {{ base_path }}/MEMORY.md directly using those keywords.
4. Only if MEMORY.md directly points to rollout summaries/skills, open the 1-2
   most relevant files under {{ base_path }}/rollout_summaries/ or
   {{ base_path }}/skills/.
5. If there are no relevant hits, stop memory lookup and continue normally.

Quick-pass budget:

- Keep memory lookup lightweight: ideally <= 4-6 search steps before main work.
- Avoid broad scans of all rollout summaries.

During execution: if you hit repeated errors, confusing behavior, or suspect
relevant prior context, redo the quick memory pass.

How to decide whether to verify memory:

- Consider both risk of drift and verification effort.
- If a fact is likely to drift and is cheap to verify, verify it before
  answering.
- If a fact is likely to drift but verification is expensive, slow, or
  disruptive, it is acceptable to answer from memory in an interactive turn,
  but you should say that it is memory-derived, note that it may be stale, and
  consider offering to refresh it live.
- If a fact is lower-drift and cheap to verify, use judgment: verification is
  more important when the fact is central to the answer or especially easy to
  confirm.
- If a fact is lower-drift and expensive to verify, it is usually fine to
  answer from memory directly.

When answering from memory without current verification:

- If you rely on memory for a fact that you did not verify in the current turn,
  say so briefly in the final answer.
- If that fact is plausibly drift-prone or comes from an older note, older
  snapshot, or prior run summary, say that it may be stale or outdated.
- If live verification was skipped and a refresh would be useful in the
  interactive context, consider offering to verify or refresh it live.
- Do not present unverified memory-derived facts as confirmed-current.
- For interactive requests, prefer a short refresh offer over silently doing
  expensive verification that the user did not ask for.
- When the unverified fact is about prior results, commands, timing, or an
  older snapshot, a concrete refresh offer can be especially helpful.

Memory citation requirements:

- If ANY relevant memory files were used: append exactly one
`<oai-mem-citation>` block as the VERY LAST content of the final reply.
  Normal responses should include the answer first, then append the
`<oai-mem-citation>` block at the end.
- Use this exact structure for programmatic parsing:
```
<oai-mem-citation>
<citation_entries>
MEMORY.md:234-236|note=[responsesapi citation extraction code pointer]
rollout_summaries/2026-02-17T21-23-02-LN3m-weekly_memory_report_pivot_from_git_history.md:10-12|note=[weekly report format]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
019c7714-3b77-74d1-9866-e1f484aae2ab
</rollout_ids>
</oai-mem-citation>
```
- `citation_entries` is for rendering:
  - one citation entry per line
  - format: `<file>:<line_start>-<line_end>|note=[<how memory was used>]`
  - use file paths relative to the memory base path (for example, `MEMORY.md`,
    `rollout_summaries/...`, `skills/...`)
  - only cite files actually used under the memory base path (do not cite
    workspace files as memory citations)
  - if you used `MEMORY.md` and then a rollout summary/skill file, cite both
  - list entries in order of importance (most important first)
  - `note` should be short, single-line, and use simple characters only (avoid
    unusual symbols, no newlines)
- `rollout_ids` is for us to track what previous rollouts you find useful:
  - include one rollout id per line
  - rollout ids should look like UUIDs (for example,
    `019c6e27-e55b-73d1-87d8-4e01f1f75043`)
  - include unique ids only; do not repeat ids
  - an empty `<rollout_ids>` section is allowed if no rollout ids are available
  - you can find rollout ids in rollout summary files and MEMORY.md
  - do not include file paths or notes in this section
  - For every `citation_entries`, try to find and cite the corresponding rollout id if possible
- Never include memory citations inside pull-request messages.
- Never cite blank lines; double-check ranges.

### Compaction Protocol

Before context compaction, preserve critical state:
1. Save key decisions and progress via `notepad_write_working`
2. Write important facts to topics via `memory_add_note`
3. If context is >80% full, proactively checkpoint state

This ensures you can recover key information after compaction.

========= MEMORY_SUMMARY BEGINS =========
{{ memory_summary }}
========= MEMORY_SUMMARY ENDS =========

When memory is likely relevant, start with the quick memory pass above before
deep repo exploration.
