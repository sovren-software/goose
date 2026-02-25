#!/usr/bin/env bash
# augmentum-permit-check.sh — PreToolUse hook for Augmentum OS permit enforcement
#
# Reads active permits from /run/augmentum/permits.json and blocks tool calls
# that fall outside the session's authorized scopes.
#
# Wire up in ~/.config/goose/hooks.json:
#   "PreToolUse": [{"hooks": [{"type": "command",
#     "command": "~/.config/goose/hooks/augmentum-permit-check.sh", "timeout": 5}]}]
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
# Output: exit 0 = allow, exit 2 = block (upstream hook protocol)
#
# Fail-open: if permits file is missing, unreadable, or malformed, allows all.

set -euo pipefail

INPUT=$(cat)
PERMITS_FILE="/run/augmentum/permits.json"

# Fail-open: no permits file means no restrictions
if [[ ! -f "$PERMITS_FILE" ]]; then
    exit 0
fi

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

# Fail-open: no tool name parsed
if [[ -z "$TOOL_NAME" ]]; then
    exit 0
fi

# Load active permit scopes
SCOPES=$(jq -r '[.active[].scope] | join(",")' "$PERMITS_FILE" 2>/dev/null)
if [[ -z "$SCOPES" ]]; then
    # No active permits = unrestricted
    exit 0
fi

# --- Shell command enforcement ---
if [[ "$TOOL_NAME" == "developer__shell" ]]; then
    if [[ ",$SCOPES," != *",shell,"* ]]; then
        # Exit 2 = block in upstream hook protocol
        exit 2
    fi

    # Check command against allowed patterns
    COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null)
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
                exit 2
            fi
        fi
    fi
fi

# --- Write operation enforcement ---
if [[ "$TOOL_NAME" == "write_file" || "$TOOL_NAME" == "edit_file" || "$TOOL_NAME" == "patch_file" ]]; then
    if [[ ",$SCOPES," != *",write,"* ]]; then
        exit 2
    fi
fi

# --- Network enforcement ---
if [[ "$TOOL_NAME" == "fetch_url" || "$TOOL_NAME" == "web_search" ]]; then
    if [[ ",$SCOPES," != *",network,"* ]]; then
        exit 2
    fi
fi

# --- MCP extension tool enforcement ---
# augmentum-system__* write tools are gated by aegis-permit-check (Rust binary).
# The MCP server itself also checks permits; this hook is defense-in-depth at
# the Goose layer before the call reaches the MCP server.
# Command names must match what the MCP server passes to aegis-permit-check.
if command -v aegis-permit-check &>/dev/null; then
    case "$TOOL_NAME" in
        augmentum-system__service_restart)
            aegis-permit-check "infrastructure" "restart" 2>/dev/null || exit 2 ;;
        augmentum-system__service_stop)
            aegis-permit-check "infrastructure" "stop" 2>/dev/null || exit 2 ;;
        augmentum-system__network_reset_interface)
            aegis-permit-check "infrastructure" "network_reset" 2>/dev/null || exit 2 ;;
        augmentum-system__network_flush_routes)
            aegis-permit-check "infrastructure" "network_flush" 2>/dev/null || exit 2 ;;
        augmentum-system__ip_rule_delete)
            aegis-permit-check "infrastructure" "ip_rule_delete" 2>/dev/null || exit 2 ;;
    esac
fi

# Default: allow (exit 0)
exit 0
