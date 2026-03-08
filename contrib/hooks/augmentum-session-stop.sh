#!/usr/bin/env bash
# augmentum-session-stop.sh — SessionStop hook for Augmentum OS
#
# Phase 1: Keyword stop gate — blocks if last assistant message contains
#           unfinished-work indicators (ported from Claude Code stop-completion-check.sh)
# Phase 2: Session cleanup and summary logging
#
# Wire up in ~/.config/goose/hooks.json:
#   "Stop": [{"hooks": [{"type": "command",
#     "command": "~/.config/goose/hooks/augmentum-session-stop.sh", "timeout": 10}]}]
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
LAST_MSG=$(echo "$INPUT" | jq -r '.last_assistant_text // empty' 2>/dev/null)
TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# === Phase 1: Keyword stop gate ===
if [[ -n "$LAST_MSG" ]]; then
    LOWER_MSG=$(echo "$LAST_MSG" | tr '[:upper:]' '[:lower:]')

    # Meta-discussion guard: skip if message discusses the hook/completion system itself.
    # Catches: direct hook naming AND describing hook behavior (e.g. "hook scans for next steps").
    IS_META=false
    if [[ "$LOWER_MSG" == *"stop hook"* ]] || [[ "$LOWER_MSG" == *"stop gate"* ]] || \
       [[ "$LOWER_MSG" == *"completion check"* ]] || [[ "$LOWER_MSG" == *"keyword scan"* ]] || \
       [[ "$LOWER_MSG" == *"hook scan"* ]] || [[ "$LOWER_MSG" == *"hook block"* ]] || \
       [[ "$LOWER_MSG" == *"hook detect"* ]] || [[ "$LOWER_MSG" == *"hook fires"* ]] || \
       [[ "$LOWER_MSG" == *"hook check"* ]]; then
        IS_META=true
    fi

    # Code-fence guard: skip if blocked keyword appears only inside a code fence context
    if [[ "$LOWER_MSG" == *'```'*"next step"*'```'* ]] || \
       [[ "$LOWER_MSG" == *'```'*"todo"*'```'* ]]; then
        IS_META=true
    fi

    if [[ "$IS_META" == "false" ]]; then
        # Continuation cap: max 3 blocks per session
        BLOCK_COUNTER_FILE="$DATA_DIR/.stop-blocks-${SESSION_ID}"
        BLOCK_COUNT=0
        if [[ -f "$BLOCK_COUNTER_FILE" ]]; then
            BLOCK_COUNT=$(cat "$BLOCK_COUNTER_FILE" 2>/dev/null) || BLOCK_COUNT=0
        fi

        if [[ "$BLOCK_COUNT" -lt 3 ]]; then
            MATCH=""

            # TODO (case-sensitive, search original message)
            if [[ "$LAST_MSG" == *"TODO"* ]]; then
                MATCH="TODO found"
            fi

            # next step(s) (case-insensitive, glob — avoids ERE apostrophe issues)
            if [[ -z "$MATCH" ]] && [[ "$LOWER_MSG" == *"next step"* ]]; then
                MATCH="'next step(s)' found"
            fi

            # still need(s) to
            if [[ -z "$MATCH" ]] && \
               [[ "$LOWER_MSG" == *"still need to"* || "$LOWER_MSG" == *"still needs to"* ]]; then
                MATCH="'still need(s) to' found"
            fi

            # haven't yet / hasn't yet (glob avoids ERE apostrophe escaping issues)
            if [[ -z "$MATCH" ]] && \
               [[ "$LOWER_MSG" == *"haven't yet"* || "$LOWER_MSG" == *"hasn't yet"* ]]; then
                MATCH="'haven't/hasn't yet' found"
            fi

            # remaining: / remaining items
            if [[ -z "$MATCH" ]] && \
               [[ "$LOWER_MSG" == *"remaining:"* || "$LOWER_MSG" == *"remaining items"* ]]; then
                MATCH="'remaining' found"
            fi

            # will continue / let me continue / i'll continue
            if [[ -z "$MATCH" ]] && \
               [[ "$LOWER_MSG" == *"will continue"* || "$LOWER_MSG" == *"let me continue"* || \
                  "$LOWER_MSG" == *"i'll continue"* ]]; then
                MATCH="'will/let me/i'll continue' found"
            fi

            if [[ -n "$MATCH" ]]; then
                BLOCK_COUNT=$((BLOCK_COUNT + 1))
                echo "$BLOCK_COUNT" > "$BLOCK_COUNTER_FILE"
                jq -cn --arg reason "Unfinished work detected: $MATCH" \
                    '{"decision": "block", "reason": $reason}'
                exit 0
            fi
        fi
    fi
fi

# === Phase 2: Session logging ===

TOOL_COUNT=0
if [[ -f "$AUDIT_LOG" ]]; then
    TOOL_COUNT=$(grep -c "\"session_id\":\"$SESSION_ID\"" "$AUDIT_LOG" 2>/dev/null) || TOOL_COUNT=0
fi

jq -cn --arg ts "$TIMESTAMP" \
       --arg sid "$SESSION_ID" \
       --argjson tools "$TOOL_COUNT" \
       '{"timestamp": $ts, "session_id": $sid, "event": "session_stop", "tool_calls": $tools}' \
    >> "$SESSION_LOG" 2>/dev/null

rm -f "/tmp/.goose-session-$SESSION_ID"* 2>/dev/null

exit 0
