# Augmentum Agent (Goose Fork)

Sovren Software's fork of [block/goose](https://github.com/block/goose).
Upstream remote: `origin` → `https://github.com/block/goose.git`
Sovren remote: `sovren` → `https://github.com/sovren-software/goose.git`

---

## What This Fork Adds

### Lifecycle Hooks (`feat/lifecycle-hooks`, merged to `main` on this fork)

5-event hook system wired into the agent lifecycle. Claude Code–compatible YAML config.

```yaml
# ~/.config/goose/hooks.yaml
hooks:
  session_start:
    - command: "/path/to/init.sh"
      timeout: 15
  pre_tool_use:
    - command: "/path/to/guard.sh"
      tool_name: "developer__shell"
  session_stop:
    - command: "/path/to/cleanup.sh"
```

Key files:
- `crates/goose/src/hooks/` — hook system (config, executor, inspector)
- `crates/goose/tests/hooks_integration.rs` — 6 integration tests
- `docs/hooks.md` — user documentation

### Augmentum Fleet Provider

`crates/goose/src/providers/declarative/augmentum.json` — declarative provider
that routes through our LiteLLM gateway at `localhost:4000`.

**Setup:**
```bash
# Store the gateway key in Goose's keychain
goose configure  # select "Augmentum Fleet", enter litellm-local-key when prompted
# OR set env var:
export LITELLM_API_KEY=litellm-local-key
```

**Models available (via LiteLLM aliases):**
- Free local: `qwen3-8b`, `qwen2.5-coder-14b`
- Free OR: `deepseek-r1-free`, `qwen3-235b-thinking-free`
- Budget paid: `qwen3-235b`, `deepseek-v3.2`, `gemini-2.5-flash`
- Premium: `kimi-k2`, `o4-mini`, `api-sonnet`, `api-opus`

---

## Development

**Build:** See "Rust Builds on ccxx" memory rule — route heavy builds to cc-xx-22.
```bash
# cargo check is safe on ccxx (14GB)
CARGO_BUILD_JOBS=2 cargo check -p goose
# Full build → cc-xx-22
ssh cc-xx-22 "cd ~/cDesign/goose && cargo build --release -p goose-cli"
```

**Branch strategy:**
- `main` — our fork's main, tracks upstream + Sovren additions
- `feat/lifecycle-hooks` — source branch for hooks (already in our main via commit 37337a3)
- Augmentum-specific work goes directly to `main` or topic branches pushed to `sovren`

**Upstream sync:**
```bash
git fetch origin
git merge origin/main  # merge upstream changes onto our main
git push sovren main
```

---

## Architecture Notes

- Provider system: `crates/goose/src/providers/` — declarative JSON providers live in `declarative/`
- Hook inspector integrates into `ToolInspectionManager` — see `tool_inspection.rs`
- Session lifecycle wiring: `crates/goose-server/src/routes/agent.rs` (SessionStart/Stop)
- Agent reply loop: `crates/goose/src/agents/agent.rs::reply()`

---

## Relationship to Augmentum OS

Goose is the AI agent runtime layer for Augmentum OS. Integration points:
- Hooks read from `/run/augmentum/` context at session start (planned)
- LiteLLM gateway (`localhost:4000`) provides fleet model routing
- Session hooks enforce Augmentum OS policies via `pre_tool_use`
