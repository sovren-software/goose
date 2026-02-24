#!/usr/bin/env bash
# augmentum-context-inject.sh — CQI v1 bridge for Goose sessions
#
# Cognitive Query Interface bridge: calls the three cognitive layer tools
# (memory-inject.py, vault-inject.py, rule-apply.py) and aggregates their
# output into a single context injection for the Goose agent runtime.
#
# This script is the formal boundary between the cognitive layer (dotfiles)
# and the execution layer (Goose). See:
#   ~/.dotfiles/.claude/docs/architecture/COGNITIVE-EXECUTION-BOUNDARY-ADR.md
#
# Wire up in ~/.config/goose/hooks.yaml:
#   hooks:
#     session_start:
#       - command: "~/.config/goose/hooks/augmentum-context-inject.sh"
#         timeout: 15
#     prompt_submit:
#       - command: "~/.config/goose/hooks/augmentum-context-inject.sh"
#         timeout: 10
#
# CQI v1 Input (JSON on stdin):
#   {
#     "type": "context_query",      (optional — defaults to context_query)
#     "session_id": "string",
#     "prompt": "string",           (optional for SessionStart)
#     "tier": "terse|balanced|thorough",  (optional — defaults to balanced)
#     "cwd": "/path/to/working/dir",
#     "runtime": "goose"
#   }
#
# Also accepts raw Goose hook input:
#   {
#     "event": "session_start|prompt_submit",
#     "session_id": "string",
#     "prompt_text": "string"
#   }
#
# Output: {"context_injection": "...", "sources": {...}, "tier": "..."}
#
# Dependencies: python3, jq
# Cognitive tools: ~/.dotfiles/.claude/hooks/{memory-inject,vault-inject,rule-apply}.py
# All tools gracefully degrade when embed-server is absent (FTS5/keyword-only).

set -euo pipefail

# --- Read input ---
INPUT=$(cat)

# --- Locate cognitive tools ---
DOTFILES="${HOME}/.dotfiles/.claude"
MEMORY_INJECT="${DOTFILES}/hooks/memory-inject.py"
VAULT_INJECT="${DOTFILES}/hooks/vault-inject.py"
RULE_APPLY="${DOTFILES}/hooks/rule-apply.py"

# Fail-open: if cognitive tools don't exist, emit nothing
if [[ ! -f "$MEMORY_INJECT" && ! -f "$VAULT_INJECT" && ! -f "$RULE_APPLY" ]]; then
    exit 0
fi

# --- Parse input (accept both CQI v1 format and raw Goose hook format) ---
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
# CQI v1 uses "prompt"; Goose PromptSubmit uses "prompt_text"
PROMPT=$(echo "$INPUT" | jq -r '.prompt // .prompt_text // empty' 2>/dev/null)
TIER=$(echo "$INPUT" | jq -r '.tier // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

# Default tier
TIER="${TIER:-balanced}"
TIER_UPPER=$(echo "$TIER" | tr '[:lower:]' '[:upper:]')

# Default cwd to current directory
CWD="${CWD:-$(pwd)}"

# Export session ID for cognitive tools that check the env var
export CLAUDE_SESSION_ID="${SESSION_ID}"
export PWD="${CWD}"

# --- Build tool input payload ---
# All three tools expect: {"prompt": "...", "session_id": "...", "cwd": "..."}
TOOL_INPUT=$(jq -n \
    --arg prompt "$PROMPT" \
    --arg session_id "$SESSION_ID" \
    --arg cwd "$CWD" \
    '{"prompt": $prompt, "session_id": $session_id, "cwd": $cwd}')

# --- Set up token economy state for tier awareness ---
# The cognitive tools read tier from a state file keyed by session_id.
# For Goose sessions, create this state file so tools respect the tier.
if [[ -n "$SESSION_ID" ]]; then
    STATE_DIR="${HOME}/.claude/state"
    mkdir -p "$STATE_DIR"
    STATE_FILE="${STATE_DIR}/token-economy-${SESSION_ID}.json"
    if [[ ! -f "$STATE_FILE" ]]; then
        jq -n --arg tier "$TIER_UPPER" '{"last_tier": $tier}' > "$STATE_FILE"
    fi
fi

# --- Call cognitive tools and collect output ---
MEMORY_OUTPUT=""
VAULT_OUTPUT=""
RULES_OUTPUT=""
MEMORY_COUNT=0
VAULT_COUNT=0
RULES_COUNT=0

# Memory injection (if prompt is non-empty — skip for bare SessionStart)
if [[ -n "$PROMPT" && -f "$MEMORY_INJECT" ]]; then
    MEMORY_RAW=$(echo "$TOOL_INPUT" | python3 "$MEMORY_INJECT" 2>/dev/null) || true
    if [[ -n "$MEMORY_RAW" ]]; then
        MEMORY_OUTPUT=$(echo "$MEMORY_RAW" | jq -r '.additionalContext // empty' 2>/dev/null)
        if [[ -n "$MEMORY_OUTPUT" ]]; then
            # Count entities from the yaml block (lines starting with "    - name:")
            MEMORY_COUNT=$(echo "$MEMORY_OUTPUT" | grep -c '^\- \*\*' 2>/dev/null || echo 0)
        fi
    fi
fi

# Vault injection (if prompt is non-empty and has enough words)
if [[ -n "$PROMPT" && -f "$VAULT_INJECT" ]]; then
    VAULT_RAW=$(echo "$TOOL_INPUT" | python3 "$VAULT_INJECT" 2>/dev/null) || true
    if [[ -n "$VAULT_RAW" ]]; then
        VAULT_OUTPUT=$(echo "$VAULT_RAW" | jq -r '.additionalContext // empty' 2>/dev/null)
        if [[ -n "$VAULT_OUTPUT" ]]; then
            # Count notes from the yaml block (lines with "    - path:")
            VAULT_COUNT=$(echo "$VAULT_OUTPUT" | grep -c 'path:' 2>/dev/null || echo 0)
        fi
    fi
fi

# Rule injection (if prompt is non-empty)
if [[ -n "$PROMPT" && -f "$RULE_APPLY" ]]; then
    RULES_RAW=$(echo "$TOOL_INPUT" | python3 "$RULE_APPLY" 2>/dev/null) || true
    if [[ -n "$RULES_RAW" ]]; then
        RULES_OUTPUT=$(echo "$RULES_RAW" | jq -r '.additionalContext // empty' 2>/dev/null)
        if [[ -n "$RULES_OUTPUT" ]]; then
            # Count rules (numbered entries like "1. **...")
            RULES_COUNT=$(echo "$RULES_OUTPUT" | grep -cE '^[0-9]+\.' 2>/dev/null || echo 0)
        fi
    fi
fi

# --- Aggregate outputs ---
PARTS=()

if [[ -n "$MEMORY_OUTPUT" ]]; then
    PARTS+=("$MEMORY_OUTPUT")
fi

if [[ -n "$VAULT_OUTPUT" ]]; then
    PARTS+=("$VAULT_OUTPUT")
fi

if [[ -n "$RULES_OUTPUT" ]]; then
    PARTS+=("$RULES_OUTPUT")
fi

# If no cognitive output at all, emit nothing
if [[ ${#PARTS[@]} -eq 0 ]]; then
    exit 0
fi

# Join parts with separator
COMBINED=""
for i in "${!PARTS[@]}"; do
    if [[ $i -gt 0 ]]; then
        COMBINED="${COMBINED}

---

"
    fi
    COMBINED="${COMBINED}${PARTS[$i]}"
done

# --- Emit CQI v1 output ---
# Uses context_injection (our fork) — when upstream merges, switch to additionalContext
jq -n \
    --arg ctx "$COMBINED" \
    --argjson mem "$MEMORY_COUNT" \
    --argjson vault "$VAULT_COUNT" \
    --argjson rules "$RULES_COUNT" \
    --arg tier "$TIER" \
    '{
        "context_injection": $ctx,
        "sources": {
            "memory": $mem,
            "vault": $vault,
            "rules": $rules
        },
        "tier": $tier
    }'
