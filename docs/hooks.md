# Lifecycle Hooks

Goose supports lifecycle hooks that allow external processes to integrate with the agent's execution flow. Hooks receive JSON payloads on stdin and return JSON on stdout.

## Configuration

Add hooks to your `config.yaml` under the `hooks` key:

```yaml
hooks:
  session_start:
    - command: "/path/to/start-hook.sh"
      timeout: 15
  prompt_submit:
    - command: "/path/to/inject.sh"
      timeout: 5
  pre_tool_use:
    - command: "/path/to/scanner.sh"
      timeout: 5
      tool_name: "developer__shell"
  post_tool_use:
    - command: "/path/to/logger.sh"
      timeout: 5
  session_stop:
    - command: "/path/to/cleanup.sh"
      timeout: 10
```

### Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `command` | Yes | — | Shell command to execute (parsed via shlex for proper quoting) |
| `timeout` | No | 10 | Timeout in seconds before the hook process is killed |
| `tool_name` | No | — | Regex filter for tool name (pre_tool_use and post_tool_use only) |

### Environment Variable (Legacy)

For backward compatibility, `GOOSE_SESSION_START_HOOK` is still supported as an environment variable. If set and no `session_start` hooks are in config, the env var hook runs instead.

## Events

### session_start

Fires once when a session begins. Use this for context injection (loading project context, rules, memory).

**Input:**
```json
{
  "event": "session_start",
  "session_id": "abc-123"
}
```

**Output:**
```json
{"context_injection": "Text to add to the system prompt"}
```

Plain text stdout is also accepted as a fallback (treated as context injection).

### prompt_submit

Fires on each user message before the agent processes it. Context injection overwrites the previous injection each turn (same prompt key).

**Input:**
```json
{
  "event": "prompt_submit",
  "session_id": "abc-123",
  "prompt_text": "list files in /tmp"
}
```

**Output:**
```json
{"context_injection": "Per-turn context to inject"}
```

### pre_tool_use

Fires before each tool call. Can block tool execution or require user approval. Integrates with Goose's tool inspection pipeline (runs after Security, Permission, and Repetition inspectors).

**Input:**
```json
{
  "event": "pre_tool_use",
  "tool_name": "developer__shell",
  "tool_arguments": {"command": "rm -rf /"}
}
```

**Output:**
```json
{"decision": "block", "reason": "Destructive command blocked by policy"}
```

Decision values:
- `"allow"` — permit the tool call (default if hook returns no decision)
- `"block"` — deny the tool call entirely
- `"require_approval"` — prompt the user for confirmation

When multiple pre_tool_use hooks are configured, the most restrictive decision wins. Execution short-circuits on `"block"`.

**tool_name filter:** When `tool_name` is set in the hook config, the hook only fires for tool calls matching the regex pattern. Example: `tool_name: "developer__shell"` only fires for shell commands.

### post_tool_use

Fires after each tool call completes. Fire-and-forget: stdout is ignored, errors are logged but not propagated.

**Input:**
```json
{
  "event": "post_tool_use",
  "session_id": "abc-123",
  "tool_name": "developer__shell",
  "tool_arguments": {"command": "ls /tmp"},
  "tool_result": "[\"file1.txt\", \"file2.txt\"]",
  "tool_error": null
}
```

### session_stop

Fires when a session ends. Fire-and-forget with best-effort execution.

**Input:**
```json
{
  "event": "session_stop",
  "session_id": "abc-123"
}
```

## Multiple Hooks Per Event

Multiple hooks can be configured for any event:

- **Context injection events** (session_start, prompt_submit): All hooks run sequentially. Non-empty injections are concatenated with newlines.
- **Decision events** (pre_tool_use): Hooks run sequentially. Most restrictive decision wins. Short-circuits on `"block"`.
- **Fire-and-forget events** (post_tool_use, session_stop): All hooks run. Errors are logged, not propagated.

## Failure Handling

All hook failures are **fail-open**: errors and timeouts are logged but never break the agent's normal operation.

- Hook process fails to spawn → no effect
- Hook times out → process is killed, no effect
- Hook returns non-zero exit code → no effect
- Hook returns invalid JSON → plain text treated as context injection (for context events); no effect (for decision events)

## Example Hook Scripts

### Session context loader

```bash
#!/bin/bash
# session-context.sh — inject project rules at session start
INPUT=$(cat)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')

# Load project-specific context
if [ -f ".goose/context.md" ]; then
    CONTEXT=$(cat .goose/context.md)
    echo "{\"context_injection\": $(echo "$CONTEXT" | jq -Rs .)}"
else
    echo "{}"
fi
```

### Security scanner

```bash
#!/bin/bash
# security-scanner.sh — block dangerous shell commands
INPUT=$(cat)
TOOL=$(echo "$INPUT" | jq -r '.tool_name // ""')
ARGS=$(echo "$INPUT" | jq -r '.tool_arguments.command // ""')

if [ "$TOOL" = "developer__shell" ]; then
    # Block rm -rf, format, and other destructive commands
    if echo "$ARGS" | grep -qE 'rm\s+-rf\s+/|mkfs\.|dd\s+if='; then
        echo '{"decision": "block", "reason": "Destructive command blocked by security policy"}'
        exit 0
    fi
fi

echo '{"decision": "allow"}'
```

### Audit logger

```bash
#!/bin/bash
# audit-logger.sh — log all tool calls for compliance
INPUT=$(cat)
TOOL=$(echo "$INPUT" | jq -r '.tool_name')
TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

echo "$INPUT" | jq -c --arg ts "$TIMESTAMP" '. + {timestamp: $ts}' >> ~/.goose/audit.jsonl
```
