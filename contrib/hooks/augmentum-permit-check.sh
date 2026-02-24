#!/usr/bin/env bash
# augmentum-permit-check.sh — PreToolUse hook for Augmentum OS permit enforcement
#
# Reads active permits from /run/augmentum/permits.json and blocks tool calls
# that fall outside the session's authorized scopes.
#
# Wire up in ~/.config/goose/hooks.yaml:
#   hooks:
#     pre_tool_use:
#       - command: "~/.config/goose/hooks/augmentum-permit-check.sh"
#         timeout: 5
#
# Permit file format (/run/augmentum/permits.json):
#   {
#     "active": [
#       { "scope": "read", "paths": ["~/cDesign/**"] },
#       { "scope": "write", "paths": ["~/cDesign/goose/**"] },
#       { "scope": "shell", "commands": ["cargo *", "git *", "ls *"] },
#       { "scope": "network", "hosts": ["localhost", "*.github.com"] }
#     ]
#   }
#
# Output: {"decision": "allow"} or {"decision": "block", "reason": "..."}
#
# Fail-open: if permits file is missing, unreadable, or malformed, allows all.

set -euo pipefail

INPUT=$(cat)
PERMITS_FILE="/run/augmentum/permits.json"

# Fail-open: no permits file means no restrictions
if [[ ! -f "$PERMITS_FILE" ]]; then
    echo '{"decision": "allow"}'
    exit 0
fi

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)
TOOL_ARGS=$(echo "$INPUT" | jq -r '.tool_arguments // empty' 2>/dev/null)

# Fail-open: no tool name parsed
if [[ -z "$TOOL_NAME" ]]; then
    echo '{"decision": "allow"}'
    exit 0
fi

# Load active permit scopes
SCOPES=$(jq -r '[.active[].scope] | join(",")' "$PERMITS_FILE" 2>/dev/null)
if [[ -z "$SCOPES" ]]; then
    # No active permits = unrestricted
    echo '{"decision": "allow"}'
    exit 0
fi

# --- Shell command enforcement ---
if [[ "$TOOL_NAME" == "developer__shell" ]]; then
    if [[ ",$SCOPES," != *",shell,"* ]]; then
        jq -n --arg reason "Shell commands not permitted in current session (active scopes: $SCOPES)" \
            '{"decision": "block", "reason": $reason}'
        exit 0
    fi

    # Check command against allowed patterns
    COMMAND=$(echo "$TOOL_ARGS" | jq -r '.command // empty' 2>/dev/null)
    if [[ -n "$COMMAND" ]]; then
        ALLOWED=$(jq -r '
            [.active[] | select(.scope == "shell") | .commands[]?] | join("\n")
        ' "$PERMITS_FILE" 2>/dev/null)

        if [[ -n "$ALLOWED" ]]; then
            MATCHED=false
            while IFS= read -r pattern; do
                # shellcheck disable=SC2254
                case "$COMMAND" in
                    $pattern) MATCHED=true; break ;;
                esac
            done <<< "$ALLOWED"

            if [[ "$MATCHED" == "false" ]]; then
                jq -n --arg reason "Command not in allowed patterns" \
                    --arg cmd "$COMMAND" \
                    '{"decision": "block", "reason": ("\($reason): \($cmd)")}'
                exit 0
            fi
        fi
    fi
fi

# --- Write operation enforcement ---
if [[ "$TOOL_NAME" == "write_file" || "$TOOL_NAME" == "edit_file" || "$TOOL_NAME" == "patch_file" ]]; then
    if [[ ",$SCOPES," != *",write,"* ]]; then
        jq -n --arg reason "Write operations not permitted in current session (active scopes: $SCOPES)" \
            '{"decision": "block", "reason": $reason}'
        exit 0
    fi
fi

# --- Network enforcement ---
if [[ "$TOOL_NAME" == "fetch_url" || "$TOOL_NAME" == "web_search" ]]; then
    if [[ ",$SCOPES," != *",network,"* ]]; then
        jq -n --arg reason "Network access not permitted in current session (active scopes: $SCOPES)" \
            '{"decision": "block", "reason": $reason}'
        exit 0
    fi
fi

# Default: allow
echo '{"decision": "allow"}'
