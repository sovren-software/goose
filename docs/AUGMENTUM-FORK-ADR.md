---
title: Augmentum Goose Fork — Architecture Decision Record
date: 2026-02-24
status: active
domain: agent-runtime, hooks, augmentum-os
---

# Augmentum Goose Fork — Architecture Decision Record

## Context

Augmentum OS is migrating from Claude Code (proprietary, Anthropic subscription-required)
to Goose (Apache 2.0, Rust, self-hosted) as its primary AI agent runtime. This fork records
the key decisions made during that migration and documents the expected state of the system.

Upstream: `https://github.com/block/goose` (block/goose)
Fork: `https://github.com/sovren-software/goose` (sovren-software/goose)

---

## Decision 1: Fork Rather Than Contribute Upstream

**Decision:** Maintain a private fork (`sovren-software/goose`) rather than attempting to
land Augmentum-specific features in `block/goose` main.

**Rationale:**
- Upstream is a product-first codebase optimized for Block Inc.'s use cases (desktop app,
  SaaS integrations, enterprise auth). Our requirements (NixOS fleet deployment, LiteLLM
  routing, cognitive layer injection, permit enforcement) are orthogonal to their roadmap.
- A fork allows us to ship on our own timeline and with our own architectural constraints.
- Upstream sync (`git merge origin/main`) is low-friction — their changes are primarily
  in UI/apps layers that don't conflict with our Rust additions.

**Trade-offs:**
- We carry merge debt. Upstream breaking changes in `crates/goose/src/agents/agent.rs`
  (the most frequently modified file) require rebase work.
- Augmentum-specific behavior is invisible to upstream users and won't benefit from their
  review or testing.

**Mitigation:** Maintain Augmentum additions in `contrib/` (shell scripts, config, docs) and
keep Rust source changes minimal. Where upstream ships a feature we've built (hooks was the
first case), adopt theirs and delete ours.

---

## Decision 2: Adopt Upstream Hooks Rather Than Maintain Ours

**Decision (Phase 4, 2026-02-24):** Replace our 5-event hook implementation
(`executor.rs`, `inspector.rs`) with upstream's 16-event implementation from
`origin/hooks/claude-code-compatible` (PR #7411).

**Rationale:**

| Aspect | Our Implementation | Upstream Implementation |
|--------|-------------------|------------------------|
| Events | 5 | 16 (PostCompact, SubagentStart/Stop, Notification, etc.) |
| Execution | Direct `tokio::process::Command` | Via `developer__shell` MCP tool |
| Config | Flat YAML, snake_case events | JSON, PascalCase, typed actions |
| Matchers | Regex on tool_name | `Bash(glob)` syntax + direct name |
| Agent wiring | Injected into ToolInspectionManager | Wired directly in `reply_internal()` |
| Output truncation | None | 32KB cap with floor_char_boundary |
| PostCompact hook | Not implemented | Built-in |
| Project hooks | No | Yes (`allow_project_hooks` flag) |

Our implementation proved the concept and validated the architecture (5 bug fixes, 30 tests).
Their implementation is the production solution. Maintaining parallel implementations of the
same subsystem creates ongoing merge friction with no benefit.

**Merge strategy:** Pre-clean our Rust implementation (revert to `origin/main`) before
merging their branch — eliminates conflicts in all shared files.

**Expected benefits:**
- 11 additional event types at zero maintenance cost
- PostCompact hook (was on our v2.1 roadmap) available immediately
- MCP-routed execution is architecturally consistent — hooks use the same dispatch path as
  tool calls, preventing subtle execution environment mismatches
- Claude Code–compatible config format allows contrib hooks to work with both runtimes
- Community-maintained: upstream review, testing, and improvements accrue to us

**Drawbacks / Known Limitations:**
- Execution now depends on `developer__shell` MCP extension being loaded. If the extension
  is absent (unusual config, headless mode without extensions), hooks silently skip.
- Timeout default is 600s (10 minutes) upstream vs. our previous 10s. The contrib hooks
  set explicit lower timeouts, but an unconfigured hook could block for 10 minutes.
- MCP routing adds ~1-2ms overhead per hook invocation vs. direct subprocess. Negligible
  for our use case but worth noting.
- Their executor does not support `async: true` (fire-and-forget). Session-stop telemetry
  runs synchronously with the Stop event. Acceptable given our 10s timeout.

---

## Decision 3: Cognitive Layer Stays in Dotfiles, Not in Goose

**Decision:** All behavioral intelligence (memory, vault, rules, skills) remains in
`~/.dotfiles/.claude/`. Goose accesses it exclusively through the CQI v1 bridge
(`augmentum-context-inject.sh`). No cognitive Python is vendored into the Goose repo.

**Rationale:** The cognitive layer must be runtime-agnostic. Claude Code and Goose sessions
run in parallel; both should benefit from the same memory and rules without duplication.
Vendoring intelligence into Goose would create two diverging cognitive stacks.

**Interface spec:** `~/.dotfiles/.claude/docs/architecture/COGNITIVE-EXECUTION-BOUNDARY-ADR.md`

**Expected benefits:**
- Improvements to memory retrieval, vault injection, or rules scoring automatically improve
  both Claude Code and Goose sessions without any changes to this repo.
- The CQI bridge is a thin shell script — inspectable, debuggable, replaceable.

**Drawbacks / Known Limitations:**
- Cognitive tools (`memory-inject.py`, `vault-inject.py`, `rule-apply.py`) must be present
  on every machine where Goose is deployed. They live in `~/.dotfiles/` which is fleet-synced
  — but dotfiles sync must precede Goose deployment.
- The embed-server (`~/.dotfiles/.claude/hooks/embed-server.py`) runs per Claude Code session
  but not per Goose session. Goose sessions degrade to FTS5-only memory search without it.
  A persistent embed-server as a systemd user service would fix this.

---

## Augmentum-Specific Additions (What the Fork Adds)

### Lifecycle Hooks

Adopted from upstream PR #7411. See `docs/hooks.md` for full protocol documentation.

**Files:** `crates/goose/src/hooks/{types,config,mod}.rs`

### Augmentum Fleet Provider

Declarative JSON provider routing all model calls through LiteLLM gateway at `localhost:4000`.

**File:** `crates/goose/src/providers/declarative/augmentum.json`

**Setup:**
```bash
goose configure  # select "Augmentum Fleet"
# OR: export LITELLM_API_KEY=litellm-local-key
```

### Contrib Hooks (Augmentum OS Integration)

Production hook scripts implementing the Augmentum OS policy layer.

| Script | Event | Purpose |
|--------|-------|---------|
| `augmentum-session-start.sh` | SessionStart | Node identity, fleet models, git context |
| `augmentum-context-inject.sh` | UserPromptSubmit | CQI v1 bridge (memory + vault + rules) |
| `augmentum-permit-check.sh` | PreToolUse | Session scope enforcement |
| `augmentum-pre-tool-use.sh` | PreToolUse (shell only) | Command audit logger |
| `augmentum-session-stop.sh` | Stop | Session telemetry |

**Install:**
```bash
mkdir -p ~/.config/goose/hooks
cp contrib/hooks/*.sh ~/.config/goose/hooks/
chmod +x ~/.config/goose/hooks/*.sh
cp contrib/config/hooks.json ~/.config/goose/hooks.json
```

---

## Hook Protocol Reference (Upstream)

Input field names for `developer__shell`-routed hooks:

| Field | Description | Events |
|-------|-------------|--------|
| `hook_event_name` | PascalCase event type | All |
| `session_id` | Session UUID | All |
| `cwd` | Working directory | All |
| `user_prompt` | User's message text | UserPromptSubmit |
| `tool_name` | Tool being called | PreToolUse, PostToolUse |
| `tool_input` | Tool arguments (JSON object) | PreToolUse, PostToolUse |
| `tool_output` | Tool result | PostToolUse |
| `tool_error` | Error message | PostToolUseFailure |

Output:

| Field | Description |
|-------|-------------|
| `additionalContext` | Text injected as invisible user message |
| `decision` | `"Block"` or `"Allow"` (blockable events) |
| exit code 2 | Block (alternative to JSON decision) |

---

## Remaining Work

### Deployment (Immediate)

- [ ] **Release build verification** — `cargo build --release -p goose-cli` on cc-xx-22 (31GB). `cargo check` passes; release build not yet confirmed.
- [ ] **hooks.json deployed** — copy `contrib/config/hooks.json` to `~/.config/goose/hooks.json` and install hook scripts on CCX.
- [ ] **NixOS derivation** — package `goose-cli` in `augmentum-os` flake for fleet deployment. Use `pkgs-unstable.rustPlatform` (requires recent rustc).

### v2 Feature Roadmap

Priority order (per session plan, revised after Phase 4):

**v2.2 — Plan Mode equivalent (Effort: high, 3 days)**

Goose has no structured-thinking mode. Claude Code's `/plan` workflow enters a thinking phase before acting. Implement via a system prompt modifier triggered by complexity heuristics — or via a dedicated "planning" model alias in LiteLLM (e.g., `deepseek-r1-free` for thinking, smaller model for execution).

**v2.4 — Worktree subagents (Effort: medium, 2 days)**

Agent isolation via git worktrees. Claude Code's `isolation: "worktree"` spawns subagents in temporary branches. Goose equivalent: a `pre_tool_use` hook that detects subagent spawning intent and sets up isolation automatically, or a recipe pattern.

**v2.3 — Per-turn model routing (Effort: high, 1 week)**

Dynamic model selection per turn: fast model for simple responses, reasoning model for complex analysis, code model for tool-heavy turns. Requires a dispatch-advisor classifier reading the user prompt + conversation state and setting `GOOSE_MODEL` per turn via a `UserPromptSubmit` hook.

### Known Gaps vs Claude Code

| Feature | Claude Code | Goose (current) | Gap Severity |
|---------|-------------|-----------------|--------------|
| Plan Mode | `/plan` + approval gate | None | High |
| Worktree isolation | `isolation: "worktree"` | None | Medium |
| Multi-model per-turn | Routing via LiteLLM | Single model per session | Medium |
| Skill system | `~/.claude/skills/` auto-activation | Manual AGENTS.md / recipes | Low (CQI partially covers) |
| Persistent embed-server | Per-session subprocess | Not wired for Goose | Low (FTS5 fallback active) |
| PostCompact context reinject | Via PostCompact hook | Available via upstream hooks | Resolved (hook wired) |

---

## Upstream Sync Policy

```bash
# Sync upstream main changes
git fetch origin
git merge origin/main --no-edit
git push sovren main

# Monitor upstream hooks PR
# https://github.com/block/goose/pull/7411 (hooks/claude-code-compatible)
# Watch for merges to origin/main — already adopted, no further action needed
```

Upstream branches to watch:
- `origin/main` — merge periodically (monthly or when significant features land)
- `origin/hooks/claude-code-compatible` — already merged (Phase 4). Monitor for follow-up commits.

---

## Relationship to Other ADRs

- `~/.dotfiles/.claude/docs/architecture/COGNITIVE-EXECUTION-BOUNDARY-ADR.md` — boundary definition, CQI v1 spec
- `~/.dotfiles/.claude/docs/architecture/NERVOUS-SYSTEM-ARCHITECTURE.md` — cognitive layer hook pipeline internals
- `~/cDesign/augmentum-os/docs/core/TWO-LAYER-ARCHITECTURE.md` — L0/L1 split; Goose lives in L1
