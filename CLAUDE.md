# Augmentum Agent (Goose Fork)

Sovren Software's fork of [block/goose](https://github.com/block/goose).
Upstream remote: `origin` → `https://github.com/block/goose.git`
Sovren remote: `sovren` → `https://github.com/sovren-software/goose.git`

---

## What This Fork Adds

### Lifecycle Hooks (upstream `hooks/claude-code-compatible` branch, merged)

16-event hook system wired into the agent lifecycle. Claude Code–compatible JSON config.
Adopted from upstream PR #7411 with full agent.rs integration.

```json
// ~/.config/goose/hooks.json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "/path/to/init.sh", "timeout": 15}]}],
    "PreToolUse": [{"matcher": "developer__shell", "hooks": [{"type": "command", "command": "/path/to/guard.sh", "timeout": 5}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "/path/to/cleanup.sh", "timeout": 10}]}]
  }
}
```

Key files:
- `crates/goose/src/hooks/` — hook system (types, config, executor via ExtensionManager)
- `docs/hooks.md` — user documentation
- `contrib/hooks/` — Augmentum OS production hook scripts
- `contrib/config/hooks.json` — reference config for Augmentum fleet

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
- Augmentum-specific work goes directly to `main` or topic branches pushed to `sovren`
- Upstream hooks merged from `origin/hooks/claude-code-compatible` (our initial impl replaced)

**Upstream sync:**
```bash
git fetch origin
git merge origin/main  # merge upstream changes onto our main
git push sovren main
```

---

## Architecture Notes

- Provider system: `crates/goose/src/providers/` — declarative JSON providers live in `declarative/`
- Hooks: `crates/goose/src/hooks/` — types, config, executor (routes through ExtensionManager/MCP)
- Hook wiring: `crates/goose/src/agents/agent.rs` — SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PreCompact, PostCompact, Stop
- Agent reply loop: `crates/goose/src/agents/agent.rs::reply()`

---

## Relationship to Augmentum OS

Goose is the AI agent runtime layer for Augmentum OS. Integration points:
- Hooks read from `/run/augmentum/` context at SessionStart
- CQI v1 bridge (UserPromptSubmit) injects memory, vault, and rules from the cognitive layer
- LiteLLM gateway (`localhost:4000`) provides fleet model routing
- Permit enforcement (PreToolUse) blocks tool calls outside session scopes
- Architecture boundary: `~/.dotfiles/.claude/docs/architecture/COGNITIVE-EXECUTION-BOUNDARY-ADR.md`
