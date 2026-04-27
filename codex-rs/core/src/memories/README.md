# Memories Pipeline (Core)

This module runs a startup memory pipeline for eligible sessions, plus several
subsystems that give the agent active read/write access to its own memory.

---

## What's New

### v0.2 — Consolidated Memory Subsystem (2026-04-27)

Three new subsystems inspired by Claude Code and oh-my-codex have been added,
bringing the total to five cooperating components:

| Component | Source | Status |
|-----------|--------|--------|
| Phase 1 + Phase 2 Pipeline | Original codex-cli | Stable |
| AGENTS.md Hierarchical Loading | Claude Code `claudemd` | **New** |
| Memory CRUD Built-in Tools | oh-my-codex `memory_tools` | **New** |
| Notepad Section System | oh-my-codex `notepad` | **New** |
| AutoDream Background Daemon | Claude Code `autodream` | Planned |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│ Session Start                                                │
│                                                              │
│  1. claudemd: Load 4-scope AGENTS.md hierarchy + @include   │
│     → Injected into user_instructions                        │
│                                                              │
│  2. Background Pipeline: Phase1 Extract → Phase2 Consolidate │
│     → Produces MEMORY.md + topics/*.md                       │
│                                                              │
│  3. prompts.rs:                                              │
│     a. build_memory_prompt_content()                         │
│        → MEMORY.md + top-8 topics (relevance-scored)         │
│     b. Notepad PRIORITY section                              │
│     → Merged into developer_instructions                     │
│                                                              │
│  4. 8 Built-in Tools: Agent can actively CRUD memory         │
│     → Modified topics/notepad take effect on next turn       │
└─────────────────────────────────────────────────────────────┘
```

---

## Roadmap

- [ ] **AutoDream Background Daemon** — Replace startup-blocking Phase 2 with a
      3-gate background consolidator (time ≥ 24h, ≥ 5 new sessions, no lock).
      4-phase merge: Orient → Gather → Consolidate → Prune.
- [ ] **Notepad TUI Integration** — Show notepad sections in the TUI sidebar;
      allow manual editing.
- [ ] **Memory Tool Hook for TUI** — Surface `memory_read` / `notepad_read`
      output in the TUI's context panel.
- [ ] **Cross-session Memory Sharing** — Allow project-level memory roots to be
      shared across users (read-only).
- [ ] **Memory Versioning** — Keep a lightweight changelog of topic edits so
      agents can reason about what changed and when.

---

## Prompt Templates

Memory prompt templates live under `codex-rs/core/templates/memories/`.

- The undated template files are the canonical latest versions used at runtime:
  - `stage_one_system.md`
  - `stage_one_input.md`
  - `consolidation.md`
  - `read_path.md`
- In `codex`, edit those undated template files in place.
- The dated snapshot-copy workflow is used in the separate `openai/project/agent_memory/write` harness repo, not here.

---

## Phase 1: Rollout Extraction (per-thread)

Phase 1 finds recent eligible rollouts and extracts a structured memory from each one.

Eligible rollouts are selected from the state DB using startup claim rules. In practice this means
the pipeline only considers rollouts that are:

- from allowed interactive session sources
- within the configured age window
- idle long enough (to avoid summarizing still-active/fresh rollouts)
- not already owned by another in-flight phase-1 worker
- within startup scan/claim limits (bounded work per startup)

What it does:

- claims a bounded set of rollout jobs from the state DB (startup claim)
- filters rollout content down to memory-relevant response items
- sends each rollout to a model (in parallel, with a concurrency cap)
- expects structured output containing:
  - a detailed `raw_memory`
  - a compact `rollout_summary`
  - an optional `rollout_slug`
- redacts secrets from the generated memory fields
- stores successful outputs back into the state DB as stage-1 outputs

Concurrency / coordination:

- Phase 1 runs multiple extraction jobs in parallel (with a fixed concurrency cap) so startup memory generation can process several rollouts at once.
- Each job is leased/claimed in the state DB before processing, which prevents duplicate work across concurrent workers/startups.
- Failed jobs are marked with retry backoff, so they are retried later instead of hot-looping.

Job outcomes:

- `succeeded` (memory produced)
- `succeeded_no_output` (valid run but nothing useful generated)
- `failed` (with retry backoff/lease handling in DB)

Phase 1 is the stage that turns individual rollouts into DB-backed memory records.

## Phase 2: Global Consolidation

Phase 2 consolidates the latest stage-1 outputs into the filesystem memory artifacts and then runs a dedicated consolidation agent.

What it does:

- claims a single global phase-2 lock before touching the memories root (so only one consolidation
  inspects or mutates the workspace at a time)
- loads a bounded set of stage-1 outputs from the state DB using phase-2
  selection rules:
  - ignores memories whose `last_usage` falls outside the configured
    `max_unused_days` window
  - for memories with no `last_usage`, falls back to `generated_at` so fresh
    never-used memories can still be selected
  - ranks eligible memories by `usage_count` first, then by the most recent
    `last_usage` / `generated_at`
- computes a completion watermark from the claimed watermark + newest input timestamps
- syncs local memory artifacts under the memories root:
  - `raw_memories.md` (merged raw memories, latest first)
  - `rollout_summaries/` (one summary file per selected rollout)
- keeps the memories root itself as a git-baseline directory, initialized under
  `~/.codex/memories/.git` by `codex-git-utils`
- prunes stale rollout summaries that are no longer selected
- prunes memory extension resource files older than the extension retention
  window, so cleanup appears in the workspace diff
- writes `phase2_workspace_diff.md` in the memories root with the git-style diff
  from the previous successful Phase 2 baseline to the current worktree
- if the memory workspace has no changes after artifact sync/pruning, marks the
  job successful and exits

If the memory workspace has changes, it then:

- spawns an internal consolidation sub-agent
- builds the Phase 2 prompt with the path to the generated workspace diff
- points the agent at `phase2_workspace_diff.md` for the detailed diff context
- runs it with no approvals, no network, and local write access only
- disables collab for that agent (to prevent recursive delegation)
- watches the agent status and heartbeats the global job lease while it runs
- resets the memory git baseline after the agent completes successfully; the
  generated diff file is removed before this reset so deleted content is not
  kept in the prompt artifact or unreachable git objects
- marks the phase-2 job success/failure in the state DB when the agent finishes

Selection and workspace-diff behavior:

- successful Phase 2 runs mark the exact stage-1 snapshots they consumed with
  `selected_for_phase2 = 1` and persist the matching
  `selected_for_phase2_source_updated_at`
- Phase 1 upserts preserve the previous `selected_for_phase2` baseline until
  the next successful Phase 2 run rewrites it
- Phase 2 loads only the current top-N selected stage-1 inputs, syncs
  `rollout_summaries/` and `raw_memories.md` directly to that selection, then
  lets the git-style workspace diff surface additions, modifications, and
  deletions against the previous successful memory baseline
- when the selected input set is empty, stale `rollout_summaries/` files are
  removed and `raw_memories.md` is rewritten to the empty-input placeholder;
  consolidated outputs such as `MEMORY.md`, `memory_summary.md`, and `skills/`
  are left for the agent to update

Watermark behavior:

- The global phase-2 lock does not use DB watermarks as a dirty check; git
  workspace dirtiness decides whether an agent needs to run.
- The global phase-2 job row still tracks an input watermark as bookkeeping
  for the latest DB input timestamp known when the job was claimed.
- Phase 2 recomputes a `new_watermark` using the max of:
  - the claimed watermark
  - the newest `source_updated_at` timestamp in the stage-1 inputs it actually loaded
- On success, Phase 2 stores that completion watermark in the DB.
- This avoids moving the recorded completion watermark backwards, but does not
  decide whether Phase 2 has work.

In practice, this phase is responsible for refreshing the on-disk memory workspace and producing/updating the higher-level consolidated memory outputs.

## Why it is split into two phases

- Phase 1 scales across many rollouts and produces normalized per-rollout memory records.
- Phase 2 serializes global consolidation so the shared memory artifacts are updated safely and consistently.

---

## AGENTS.md Hierarchical Loading (claudemd)

Inspired by Claude Code's `claudemd` system. Loads instruction files from
four scopes in priority order:

| Scope | Path | Description |
|-------|------|-------------|
| Managed | `~/.codex/rules/*.md` | Global policy (sorted alphabetically) |
| User | `~/.codex/AGENTS.md` | User-level instructions |
| Project | `{project}/AGENTS.md` | Project-level instructions |
| Local | `{project}/.codex/AGENTS.md` | Local overrides |

Each scope prefers `AGENTS.md`; falls back to `CLAUDE.md` for compatibility.

### @include Directive

Files can reference other files with `@include <relative-path>`:

- Paths are resolved relative to the including file's directory
- Maximum depth: 10 levels
- Circular includes are detected and skipped
- Per-file size limit: 40 KB
- mtime-based cache avoids re-reading unchanged files

### YAML Frontmatter

Each file may start with a YAML frontmatter block:

```yaml
---
memory_type: project
priority: 10
---
```

Parsed fields: `memory_type`, `priority`, `scope`.

### Implementation

- `claudemd.rs` — Core loading, expansion, caching
- Injection point: `agents_md.rs::user_instructions_with_fs()` prepends claudemd content

---

## Notepad Section System

Inspired by oh-my-codex's notepad. A structured scratchpad stored in
`{memories_root}/notepad.md` with three sections:

### PRIORITY (≤500 chars)

The single most important thing the agent should keep in mind. Replaced
entirely on each write. Automatically injected into developer instructions
via `prompts.rs::build_memory_tool_developer_instructions()`.

### WORKING MEMORY

Timestamped session notes. Appended with `notepad_write_working`. Auto-pruned
by `notepad_prune` (default: entries older than 7 days).

Format: `[2026-04-27T10:30:00Z] Completed auth refactor`

### MANUAL

Permanent notes that are never auto-pruned. Appended with
`notepad_write_working` (the handler routes to the correct section).

### Crash Safety

All writes use atomic rename: write to `.notepad.md.tmp.{pid}`, then `rename()`
to `notepad.md`.

### Implementation

- `notepad.rs` — Section parsing, atomic writes, prune logic
- `prompts.rs` — PRIORITY injection into developer instructions

---

## Memory CRUD Built-in Tools

Eight built-in tools that give the agent active read/write access to the memory
subsystem. All gated behind `Feature::MemoryTool` → `ToolsConfig.memory_tools_enabled`.

### Memory Tools

| Tool | Description |
|------|-------------|
| `memory_read` | Read MEMORY.md index + relevant topics + notepad priority |
| `memory_write` | Write/update a topic file (with YAML frontmatter) |
| `memory_add_note` | Append a timestamped note to a topic (creates if missing) |
| `memory_search` | Search topics by query, return ranked matches |

### Notepad Tools

| Tool | Description |
|------|-------------|
| `notepad_read` | Read notepad (all or specific section) |
| `notepad_write_priority` | Replace PRIORITY section (≤500 chars) |
| `notepad_write_working` | Append timestamped entry to WORKING MEMORY |
| `notepad_prune` | Prune WORKING MEMORY entries older than N days |

### Implementation

- `memory_tools.rs` — Tool handlers (8 structs implementing `ToolHandler`)
- `tools/src/memory_tool.rs` — Tool specs (JSON schema definitions)
- `tool_registry_plan_types.rs` — `ToolHandlerKind` variants
- `tool_registry_plan.rs` — Spec + handler registration
- `tool_config.rs` — `memory_tools_enabled` flag
