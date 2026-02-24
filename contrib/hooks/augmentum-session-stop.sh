#!/usr/bin/env bash
# augmentum-session-stop.sh — SessionStop hook for Augmentum OS
#
# Performs session cleanup and writes a summary entry to the session log.
# Best-effort: failures are silent (this runs during shutdown).
#
# Wire up in ~/.config/goose/hooks.yaml:
#   hooks:
#     session_stop:
#       - command: "~/.config/goose/hooks/augmentum-session-stop.sh"
#         timeout: 10
#
# Session log: ~/.local/share/goose/sessions.jsonl
# Audit log: ~/.local/share/goose/tool-audit.jsonl (read for summary)

set -euo pipefail

INPUT=$(cat)

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/goose"
SESSION_LOG="$DATA_DIR/sessions.jsonl"
AUDIT_LOG="$DATA_DIR/tool-audit.jsonl"

mkdir -p "$DATA_DIR"

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // "unknown"' 2>/dev/null)
TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Count tool calls from this session's audit log
TOOL_COUNT=0
if [[ -f "$AUDIT_LOG" ]]; then
    TOOL_COUNT=$(grep -c "\"session_id\":\"$SESSION_ID\"" "$AUDIT_LOG" 2>/dev/null || echo 0)
fi

# Write session summary
jq -n --arg ts "$TIMESTAMP" \
      --arg sid "$SESSION_ID" \
      --argjson tools "$TOOL_COUNT" \
      '{"timestamp": $ts, "session_id": $sid, "event": "session_stop", "tool_calls": $tools}' \
    >> "$SESSION_LOG" 2>/dev/null

# Clean up any session-scoped temp files
rm -f "/tmp/.goose-session-$SESSION_ID"* 2>/dev/null

exit 0
