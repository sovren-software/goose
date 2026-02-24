#!/usr/bin/env bash
# augmentum-pre-tool-use.sh — PreToolUse hook for shell command audit logging
#
# Logs all developer__shell tool calls to an audit trail. Does not block
# any operations — purely observational.
#
# Wire up in ~/.config/goose/hooks.json:
#   "PreToolUse": [{"matcher": "developer__shell",
#     "hooks": [{"type": "command",
#       "command": "~/.config/goose/hooks/augmentum-pre-tool-use.sh", "timeout": 5}]}]
#
# Audit log: ~/.local/share/goose/tool-audit.jsonl
# Each line is a JSON object with timestamp, session_id, tool_name, and command.
#
# Output: exit 0 (always — this hook only observes)

set -euo pipefail

INPUT=$(cat)

AUDIT_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/goose"
AUDIT_LOG="$AUDIT_DIR/tool-audit.jsonl"

# Ensure audit directory exists
mkdir -p "$AUDIT_DIR"

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // "unknown"' 2>/dev/null)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // "unknown"' 2>/dev/null)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null)
TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Write audit entry — safe JSON construction via jq (compact for JSONL)
jq -cn --arg ts "$TIMESTAMP" \
       --arg sid "$SESSION_ID" \
       --arg tool "$TOOL_NAME" \
       --arg cmd "$COMMAND" \
       '{"timestamp": $ts, "session_id": $sid, "tool_name": $tool, "command": $cmd}' \
    >> "$AUDIT_LOG" 2>/dev/null

# Always allow — exit 0 is allow in upstream hook protocol
exit 0
