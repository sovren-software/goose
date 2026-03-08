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
`~/.engram/.claude/`. Goose accesses it exclusively through the CQI v1 bridge
(`augmentum-context-inject.sh`). No cognitive Python is vendored into the Goose repo.

**Rationale:** The cognitive layer must be runtime-agnostic. Claude Code and Goose sessions
run in parallel; both should benefit from the same memory and rules without duplication.
Vendoring intelligence into Goose would create two diverging cognitive stacks.

**Interface spec:** `~/.engram/.claude/docs/architecture/COGNITIVE-EXECUTION-BOUNDARY-ADR.md`

**Expected benefits:**
- Improvements to memory retrieval, vault injection, or rules scoring automatically improve
  both Claude Code and Goose sessions without any changes to this repo.
- The CQI bridge is a thin shell script — inspectable, debuggable, replaceable.

**Drawbacks / Known Limitations:**
- Cognitive tools (`memory-inject.py`, `vault-inject.py`, `rule-apply.py`) must be present
  on every machine where Goose is deployed. They live in `~/.dotfiles/` which is fleet-synced
  — but dotfiles sync must precede Goose deployment.
- The embed-server (`~/.engram/.claude/hooks/embed-server.py`) runs per Claude Code session
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

## Hook Protocol Reference

Input fields (stdin JSON). All field names are snake_case.

| Field | Description | Events |
|-------|-------------|--------|
| `hook_event_name` | PascalCase event type | All |
| `session_id` | Session UUID | All |
| `cwd` | Working directory path (string) | All |
| `user_prompt` | User's message text | UserPromptSubmit |
| `tool_name` | Tool being called | PreToolUse, PostToolUse, PostToolUseFailure |
| `tool_input` | Tool arguments (JSON object) | PreToolUse, PostToolUse, PostToolUseFailure |
| `tool_output` | Tool result (string) | PostToolUse |
| `tool_error` | Error message | PostToolUseFailure |
| `message_count` | Messages before compact | PreCompact |
| `before_count` | Messages before compact | PostCompact |
| `after_count` | Messages after compact | PostCompact |
| `manual` | Whether compact was user-triggered | PreCompact, PostCompact |
| `last_assistant_text` | Full text of agent's final reply | Stop |

Output (stdout JSON, all lowercase):

| Field | Description |
|-------|-------------|
| `additional_context` | Text injected as invisible user message |
| `decision` | `"block"` or `"allow"` (blockable events only) |
| `reason` | Optional string, logged but not injected |
| exit code 2 | Block (canonical mechanism; preferred over JSON decision) |

See `docs/hooks.md` for complete event reference and output protocol.

---

## Decision 4: Augmentum System MCP — Self-Healing Runtime (2026-02-25)

**Decision:** Add a 14-tool MCP server (`augmentum-system-mcp.py`) that exposes system diagnostics
and remediation operations to Goose, gated by the existing permit system. Add a DRAVE-structured
network recovery recipe and `aegis fix` CLI entry point.

**Motivating failure:** cc-xx-22 Surfshark VPN malfunction injected WireGuard routing rules that
routed all traffic through a dead tunnel. Resolution required manual `snap stop surfshark`,
`ip rule del`, and `systemctl restart NetworkManager` — violating the Augmentum OS UX standard
(zero terminal knowledge required from user).

**Rationale:**

- The permit system (`aegis-permit-check`, HMAC-signed, Rust binary) is the correct privilege gate —
  reuse it rather than inventing a new authorization layer.
- 60+ existing `aegis` CLI subcommands already wrap the necessary diagnostics. The MCP server is
  thin glue, not new logic.
- Goose's recipe system provides a structured place to encode DRAVE diagnosis/remediation workflows
  without hardcoding logic into the agent.

**Key decisions:**

| Decision | Choice | Rationale |
|----------|--------|-----------|
| MCP server location | `augmentum-os` repo | Deployed by NixOS module; goose repo is Rust, MCP server is Python |
| Permit enforcement | Dual-layer (hook + MCP server) | Defense-in-depth; hook catches misconfigured MCP, MCP catches hook bypass |
| Stdio transport | `run_in_executor(None, readline)` | Portable across Python 3.8+; `connect_read_pipe` API varies by version |
| Network instructions | Baked Nix string, passed via `--system` | Avoids runtime YAML parsing; `--instructions` takes FILE not inline text |
| Permit commands | Per-tool (restart/stop/network_reset/network_flush/ip_rule_delete) | Enables fine-grained permit grants; wildcard permits remain possible with `*` command |

**Expected benefits:**

- Surfshark scenario (and class of VPN routing corruption failures) fully remediable via natural language
- `aegis fix "my internet is broken"` → Goose diagnoses, remediates, verifies without terminal
- Offline-capable: falls back to local llama-swap when LiteLLM gateway unreachable
- No new privilege model — existing permit infrastructure extended, not replaced
- Full HMAC audit trail for every write operation

**Trade-offs:**

- MCP server is Python, not Rust — adds `python3` runtime dependency to nodes that run Goose.
  Mitigated: python3 is already present on all NixOS nodes.
- 14 tools is a larger surface than strictly needed for the Surfshark scenario. Justified by the
  breadth of network failure classes (5 root causes) that share the same MCP infrastructure.

**Drawbacks / Known Limitations:**

- `goose run --recipe` and `--text` are mutually exclusive — `aegis fix` cannot both load a recipe
  AND pass user-provided text as the prompt. Current workaround: `--system` + `--text` with baked
  DRAVE instructions. The recipe YAML is deployed for documentation/reference; it cannot be used
  as a recipe and passed text simultaneously via the CLI.
- Goose extension config is written to `~/.config/goose/config.yaml` by the NixOS activation script.
  If the user has a pre-existing `config.yaml` from manual setup, the activation script appends the
  extension config in JSON format — which is valid YAML but not idiomatic. A second activation run
  will not duplicate it (grep guard), but the file format becomes mixed.
- The MCP server is not yet tested against Goose's live extension loader. Extension config field
  names (`type: stdio`, `cmd`, `args`, `timeout`) match the Goose declarative provider pattern but
  are unverified against the actual extension loading code path.
- `aegis-permit-check` must be in PATH when the augmentum-permit-check.sh hook runs. On nodes
  where `aegis.ai.brainTools.enable = false`, the binary will not be present; the hook degrades
  gracefully (the `command -v aegis-permit-check` guard is false, write ops are not checked by
  the hook). The MCP server's in-process check also fails gracefully: denied rather than allowed.

---

## Remaining Work

### Deployment (Immediate)

- [ ] **Release build verification** — `cargo build --release -p goose-cli` on cc-xx-22 (31GB). `cargo check` passes; release build not yet confirmed.
- [x] **hooks.json deployed** — handled by `goose.nix` activation script (writes to `~/.config/goose/hooks.json` if absent).
- [x] **NixOS derivation** — `goose.nix` deploys `goose-cli` package; `goose-mcp.nix` deploys MCP server and extension config.
- [ ] **MCP extension loader verification** — start a Goose session on a deployed node and confirm `augmentum-system__*` tools appear in `tools/list`. The extension config format is based on documentation; needs live confirmation.
- [ ] **Surfshark scenario smoke test** — simulate VPN routing corruption on a test node, run `aegis fix "fix my internet"`, verify remediation completes.
- [ ] **Persistent embed-server for Goose** — `memory-inject.py` degrades to FTS5-only when embed-server socket is absent. Wire `embed-server.py` as a systemd user service so Goose sessions get full vector search. (Claude Code sessions already handle this per-session.)

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
| Self-healing system ops | N/A (terminal-focused) | `augmentum-system` MCP (14 tools) | **Resolved** (2026-02-25) |
| Tier classification | Token-economy tiers by complexity | ~~Hardcoded BALANCED~~ | **Resolved** (2026-03-08) |
| Stop gate / completion enforcement | `stop-completion-check.sh` | ~~No equivalent~~ | **Resolved** (2026-03-08) |
| Auto-skill drafting | None | `session-receipt.sh` → `pending-skills/` | **New** (2026-03-08) |

---

## Upstream Sync Policy

```bash
# Sync upstream main changes
git fetch upstream
git merge upstream/main --no-edit
git push origin main   # origin = sovren-software/goose
```

Upstream branches to watch:
- `upstream/main` — merge periodically (monthly or when significant features land)
- `upstream/hooks/claude-code-compatible` — our hook system was re-architected independently (Decision 5); this branch is no longer relevant

---

## Decision 5: HookRuntime Re-Architecture — Direct Subprocess Execution (2026-03-07)

**Decision:** Replace the MCP-routed hook executor (Decision 2) with a `HookRuntime` that
executes hooks as direct subprocesses. Decouple hooks from `rmcp` types and `ExtensionManager`.

**Motivating problem:** Upstream merged rmcp 0.16 -> 1.1.0, restructured agent.rs (SmartApprove,
PermissionInspector, extension consolidation), and moved to builder-pattern APIs. Our hooks code
constructed `CallToolRequestParams` struct literals and routed through `ExtensionManager.dispatch_tool_call()`,
creating merge conflicts at every upstream sync. 106 commits behind with 3 breaking changes.

**Architecture:**

| Aspect | Old (Decision 2) | New (Decision 5) |
|--------|------------------|------------------|
| Execution | Via `developer__shell` MCP tool | Direct `tokio::process::Command` subprocess |
| rmcp coupling | `CallToolRequestParams` struct literals | Zero rmcp imports in hooks module |
| Agent coupling | `ExtensionManager` reference required | Only `CancellationToken` |
| Action types | Command + McpTool | Command only |
| Agent surface | ~380 lines across 3 files | ~43 lines across 3 files |
| Interface | `Hooks::run(invocation, ext_mgr, ...)` | `HookRuntime::emit(event, working_dir, cancel)` |

**Key design decisions:**
1. **Drop McpTool action type** — all hooks execute as direct subprocesses. MCP routing was the
   primary source of coupling. Can be re-added inside HookRuntime without changing the emit() API.
2. **8 event types** — SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure,
   PreCompact, PostCompact, Stop. Additional events are enum variants, not interface changes.
3. **HookEvent is the stable contract** — the only type that crosses the hooks/agent boundary.
   Serializable to the same JSON format contrib hooks expect.
4. **Config format unchanged** — same hooks.json, same matcher syntax. Zero contrib hook changes.

**Expected benefits:**
- Upstream merges require zero hook-related conflict resolution
- 89% reduction in agent wiring code (380 -> 43 lines)
- No dependency on `developer__shell` extension being loaded
- Subprocess isolation (own process group) prevents terminal SIGINT from killing hooks
- Deadlock-safe stdin/stdout handling (drain tasks spawned before stdin write)

**Trade-offs:**
- Lost McpTool action type. No current hooks use it; can be re-added if needed.
- Direct subprocess means hooks run in a different environment than MCP tools (no MCP
  transport, no extension context). For shell scripts this is identical; for hypothetical
  MCP-native hooks it would differ.

---

## Decision 6: Hermes-Parity Quality Improvements (2026-03-08)

**Decision:** Close three enforcement gaps identified by comparing Goose quality controls
against Hermes Agent and Claude Code:

1. **Adaptive tier classification** — replace hardcoded `TIER=balanced` in
   `augmentum-context-inject.sh` with word-count thresholds matching `token-economy.sh`.
2. **Keyword stop gate** — add Phase 2 keyword scan to `augmentum-session-stop.sh`,
   enabled by adding `last_assistant_text` to `HookEvent::Stop` in Rust.
3. **Auto-skill drafting** — extend `session-receipt.sh` to draft JSON stubs for
   high-signal patterns (`new_rule`, `hook_modified`, `large_session`) into a
   `pending-skills/` review queue.

**Rationale:**

The CQI bridge was serving uniform BALANCED context to every prompt regardless of complexity.
A 3-word "fix it" prompt has different context needs than a 50-word architecture question.
Hermes Agent's adaptive-context pattern addresses exactly this waste; our token-economy
thresholds already defined the right boundaries — they just weren't applied to Goose.

The stop gate gap was structural: `HookEvent::Stop` had no `last_assistant_text` field, so
keyword scanning was architecturally impossible without reading session files at stop time
(fragile, slow). Adding the field at the emission site (`agent.rs:1855`) was a zero-logic
wiring change — the text was already collected at `agent.rs:1351`.

Auto-skill drafting closes the gap between pattern detection (already wired since 2026-03-02)
and artifact generation. The session-receipt hook detected `new_rule` and `hook_modified` events
and logged them to `pattern-feedback.jsonl`, then discarded them. Hermes generates skill stubs
automatically from detected patterns. The fix is a third Python stage that produces idempotent
draft JSONs for human review.

**Key decisions:**

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Tier thresholds | <10 terse, 10-49 balanced, 50+ thorough | Match `token-economy.sh` exactly |
| Tier measurement | `wc -w` word count | Simple, no subjectivity, <1ms overhead |
| Pattern matching style | Glob (`==`) not ERE (`=~`) | Apostrophes in `haven't`/`i'll` are fragile in bash ERE |
| Meta-discussion guard | Pattern family (`hook scan`, `hook block`, etc.) | Descriptive references bypass same as direct naming |
| Skill drafts location | `data/pending-skills/` (gitignored) | Runtime queue; human review required before any persistence |
| Idempotency | `sha256(session_id:entity_name)[:12]` filename | Re-running receipt for same session produces no duplicates |

**Expected benefits:**
- Short Goose prompts use TERSE budget (less context injected, faster turns)
- Complex Goose prompts get THOROUGH budget (full memory + vault + rules)
- Unfinished-work indicators in last assistant message surface back to agent
- Session patterns generate reviewable drafts rather than silently expiring in logs

**Trade-offs:**
- `wc -w` spawns a subprocess on every UserPromptSubmit. Measured at <1ms; negligible.
- Keyword stop gate has false-positive risk on technical messages containing "next step"
  literally (e.g., a tutorial step numbered "next step 3"). Mitigated by meta-discussion
  guard and 3-continuation cap per session.

**Drawbacks / Known Limitations:**
- Stop gate Phase 1 (contract-based verification via frame-prompt sidecar) not ported —
  requires Claude Code–specific sidecar infrastructure. Keyword scan only for Goose v1.
- Stop gate Phase 3 (semantic check via LiteLLM) not ported — adds latency and a network
  dependency in the Stop hook path. Out of scope for v1.
- `pending-skills/` queue has no processing step. Drafts accumulate until manually reviewed.
  A future `/review-pending` skill or weekly-consolidation step should process the queue.
- Code-fence guard is shallow: checks whether blocked keyword appears between backtick
  fences, but does not parse multi-line code blocks correctly. Sufficient for common cases.

**Files changed:**
- `crates/goose/src/hooks/types.rs` — `last_assistant_text: String` added to `Stop` variant
- `crates/goose/src/agents/agent.rs` — field wired at emission site
- `contrib/hooks/augmentum-context-inject.sh` — word-count tier classification
- `contrib/hooks/augmentum-session-stop.sh` — keyword stop gate (Phase 2)
- `~/.engram/.claude/hooks/session-receipt.sh` — auto-skill drafting stage

---

## Relationship to Other ADRs

- `~/.engram/.claude/docs/architecture/COGNITIVE-EXECUTION-BOUNDARY-ADR.md` — boundary definition, CQI v1 spec
- `~/.engram/.claude/docs/architecture/NERVOUS-SYSTEM-ARCHITECTURE.md` — cognitive layer hook pipeline internals
- `~/cDesign/augmentum-os/docs/core/TWO-LAYER-ARCHITECTURE.md` — L0/L1 split; Goose lives in L1
