# Lifecycle Hooks

Goose supports lifecycle hooks that allow external processes to integrate with the agent's execution flow. Hooks receive JSON payloads on stdin and return JSON on stdout.

## Configuration

Add hooks to `~/.config/goose/hooks.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/start-hook.sh", "timeout": 15 }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/inject.sh", "timeout": 10 }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "developer__shell",
        "hooks": [
          { "type": "command", "command": "/path/to/guard.sh", "timeout": 5 }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/cleanup.sh", "timeout": 10 }
        ]
      }
    ]
  }
}
```

### Config Locations

| Location | Scope | Loaded |
|----------|-------|--------|
| `~/.config/goose/hooks.json` | Global (all sessions) | Always |
| `.goose/settings.json` | Project (working dir) | When `allow_project_hooks: true` in global |
| `.claude/settings.json` | Project (Claude Code compat) | When `allow_project_hooks: true` in global |

Project hooks are merged with global hooks (both run).

### Hook Action Types

| Type | Description |
|------|-------------|
| `command` | Shell command. Receives JSON on stdin, returns JSON on stdout. |
| `mcp_tool` | MCP tool invocation via ExtensionManager. |

### Fields (Command Action)

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `command` | Yes | — | Shell command to execute |
| `timeout` | No | 600 | Timeout in seconds before the hook is killed |

### Matcher (PreToolUse, PostToolUse, PostToolUseFailure)

The optional `matcher` field filters which tool calls trigger the hook:

- `"developer__shell"` — direct tool name match
- `"Bash"` — Claude Code compatibility alias for developer__shell
- `"Bash(git *)"` — matches shell commands matching the glob pattern

## Events

### SessionStart

Fires once on the first user message in a session. Context returned is injected as an invisible user message.

**Input:**
```json
{
  "hook_event_name": "SessionStart",
  "session_id": "abc-123",
  "cwd": "/home/user/project"
}
```

**Output:**
```json
{"additionalContext": "Text to inject into the conversation"}
```

Plain text stdout is also accepted (treated as context injection).

### UserPromptSubmit

Fires on each user message before the agent processes it. Can block the prompt or inject context.

**Input:**
```json
{
  "hook_event_name": "UserPromptSubmit",
  "session_id": "abc-123",
  "user_prompt": "list files in /tmp",
  "cwd": "/home/user/project"
}
```

**Output:**
```json
{"additionalContext": "Per-turn context to inject"}
```

Exit code 2 blocks the prompt entirely.

### PreToolUse

Fires before each tool call. Can block tool execution.

**Input:**
```json
{
  "hook_event_name": "PreToolUse",
  "session_id": "abc-123",
  "tool_name": "developer__shell",
  "tool_input": {"command": "rm -rf /"},
  "cwd": "/home/user/project"
}
```

**Blocking:** Exit code 2 blocks the tool call. Or return JSON:
```json
{"decision": "Block"}
```

### PostToolUse

Fires after each successful tool call. Can inject context.

**Input:**
```json
{
  "hook_event_name": "PostToolUse",
  "session_id": "abc-123",
  "tool_name": "developer__shell",
  "tool_input": {"command": "ls /tmp"},
  "tool_output": ["file1.txt", "file2.txt"],
  "cwd": "/home/user/project"
}
```

### PostToolUseFailure

Fires after a failed tool call.

**Input:**
```json
{
  "hook_event_name": "PostToolUseFailure",
  "session_id": "abc-123",
  "tool_name": "developer__shell",
  "tool_input": {"command": "invalid"},
  "tool_error": "Command not found",
  "cwd": "/home/user/project"
}
```

### PreCompact / PostCompact

Fire before and after conversation compaction (auto or manual).

**Input (PreCompact):**
```json
{
  "hook_event_name": "PreCompact",
  "session_id": "abc-123",
  "pre_compact_message_count": 42,
  "manual_compact": false,
  "cwd": "/home/user/project"
}
```

**PostCompact** adds `post_compact_message_count`.

Matcher values: `"manual"` or `"auto"` to filter by compaction type.

### Stop

Fires when the agent reply stream finishes.

**Input:**
```json
{
  "hook_event_name": "Stop",
  "session_id": "abc-123",
  "cwd": "/home/user/project"
}
```

## Output Protocol

| Exit Code | Meaning |
|-----------|---------|
| 0 | Allow. Parse stdout as JSON. Non-JSON stdout becomes `additionalContext`. |
| 2 | Block (for blockable events: PreToolUse, UserPromptSubmit, Stop, etc.) |
| Other | Fail-open. Hook error logged, execution continues. |

**JSON output fields:**

| Field | Type | Description |
|-------|------|-------------|
| `additionalContext` | string | Context to inject into the conversation |
| `decision` | `"Allow"` or `"Block"` | Override for blockable events |

## Multiple Hooks Per Event

Multiple hook actions run sequentially within each event config. Execution short-circuits on block. Context from all hooks is concatenated.

## Failure Handling

All hook failures are **fail-open**: errors and timeouts are logged but never break the agent's normal operation.

## Augmentum OS Hooks

See `contrib/hooks/` for production hook implementations:

- `augmentum-session-start.sh` — node identity, fleet models, git context
- `augmentum-context-inject.sh` — CQI v1 bridge (memory, vault, rules injection)
- `augmentum-permit-check.sh` — session scope enforcement from `/run/augmentum/permits.json`
- `augmentum-pre-tool-use.sh` — shell command audit logger
- `augmentum-session-stop.sh` — session telemetry

Install: `cp contrib/config/hooks.json ~/.config/goose/hooks.json`
