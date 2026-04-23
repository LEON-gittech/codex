<p align="center">
  <img src="./.github/open-codexcli-icon.png" alt="Open Codex CLI icon" width="180" />
</p>

<h1 align="center">Open Codex CLI</h1>

<p align="center">
  A community-maintained Codex CLI fork that stays close to upstream while making room for openly developed CLI improvements.
</p>

<p align="center">
  <code>codex</code> remains the command name, and <code>@openai/codex</code> remains the compatibility target for the current CLI surface.
</p>

---

## Motivation

Codex CLI is open source, but upstream code contributions are currently invitation-only. The upstream repository states this clearly in [docs/contributing.md](./docs/contributing.md): external pull requests that have not been explicitly invited will be closed without review.

That policy is understandable from the perspective of the upstream maintainers, but it also leaves a gap for developers who want to iterate in public, ship focused CLI improvements, and maintain a fork that can accept normal community collaboration. This repository exists to fill that gap.

The goal of **Open Codex CLI** is not to diverge for the sake of divergence. The goal is to keep a small, intentional delta on top of upstream Codex CLI, make that delta easy to understand, and keep the fork mergeable as upstream evolves.

## Current Delta vs. Latest Upstream Codex CLI

This fork is currently based on the latest upstream `openai/codex` and adds a small set of focused CLI improvements from recent fork-specific commits:

### 1. Better transcript contrast in the TUI for Zellij

From commit `598bebc6b`:

- improves visual distinction between user-authored content and assistant-rendered content when Codex CLI is used inside `zellij`
- adjusts the TUI styling path used by user message rendering for the `zellij` case
- targets a real readability issue in `zellij`; this is not the same problem in a normal terminal session or in `tmux`

This is a usability-focused patch for the `zellij` environment: the goal is to reduce ambiguity in the chat history without changing the underlying interaction model.

### 2. Stale turn output protection in the TUI

From commits `642d306a7` and `6c27de579`:

- adds turn-aware filtering for streamed assistant output
- prevents stale deltas from older turns from leaking into the currently active turn
- hardens replay and status handling around message deltas, reasoning deltas, and turn completion events
- adds regression coverage for the stale-turn cases that motivated the fix

This is a correctness-focused patch: the UI should not render output from the wrong turn, even when retry/replay/stream timing gets messy.

## Maintenance Philosophy

This fork is maintained with a conservative strategy:

- keep the fork close to upstream `openai/codex`
- merge upstream regularly rather than carrying a long-lived reimplementation
- keep fork-specific patches small, testable, and easy to reason about
- prefer user-facing CLI quality improvements over broad architectural churn
- document motivation, tradeoffs, and intended maintenance cost in the repo itself

In practice, maintenance will follow a straightforward loop:

1. track the latest upstream Codex CLI changes
2. merge upstream into this fork on a regular basis
3. re-validate the fork-specific delta
4. keep or refine only the patches that still provide clear value

The standard for changes here is simple: if a patch is not worth carrying across upstream merges, it does not belong in the fork.

## Roadmap

The near-term roadmap is intentionally focused on a few CLI-facing improvements:

### 1. Status line throughput visibility

Improve the Codex CLI status line so it can surface token throughput directly, instead of only showing coarse task state. The aim is to make model responsiveness easier to judge in real time.

### 2. Session export

Implement a Claude Code-style export flow for the current session, so a user can export the active session record in a reusable format. The goal is to make debugging, sharing, and archival much easier.

### 3. Better memory mechanics

Improve the Codex memory mechanism so it is easier to understand, easier to inspect, and more useful over long-running usage. The focus here is not just more memory, but better memory behavior.

### 4. Better Zellij ergonomics

Continue improving the Codex CLI experience under `zellij`, especially around rendering, layout, contrast, and other interaction details that behave differently from plain terminal sessions or `tmux`.

## Community

Issues and pull requests are welcome in this fork.

If you have a bug report, a CLI usability problem, a design idea, or a concrete patch, please open an issue or submit a PR. Small, focused, well-explained changes are preferred over broad, unrelated edits.

The intent of this repository is to keep development open and reviewable in public, even while the upstream repository remains invitation-only for external code contributions.

## Compatibility Notes

This fork keeps the current Codex CLI naming surface intact:

- command name: `codex`
- package naming target: `@openai/codex`

That means the README, docs, and fork messaging are intentionally about the **project identity and maintenance model**, not a wholesale rename of the CLI interface.

## Quickstart

If you want to use this fork from source, build the Rust workspace and install the resulting binary locally.

```shell
# Clone the fork and build the CLI
git clone https://github.com/LEON-gittech/codex.git
cd codex/codex-rs
cargo build --release
```

Then choose one of these install modes:

### Option A: replace your local `codex`

```shell
mkdir -p ~/.local/bin
install -m 755 target/release/codex ~/.local/bin/codex
```

### Option B: install this fork as `codex-dev`

```shell
mkdir -p ~/.local/bin
install -m 755 target/release/codex ~/.local/bin/codex-dev
```

After that, run either `codex` or `codex-dev`, depending on which install path you chose.

## Docs

- [Contributing](./docs/contributing.md)
- [Installing & building](./docs/install.md)
- [Open source fund](./docs/open-source-fund.md)

## License

This repository is licensed under the [Apache-2.0 License](./LICENSE).
