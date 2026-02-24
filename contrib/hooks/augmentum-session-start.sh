#!/usr/bin/env bash
# augmentum-session-start.sh — SessionStart hook for Augmentum OS
#
# Reads context from Augmentum OS runtime paths and injects it into
# the agent's system prompt at session start.
#
# Wire up in ~/.config/goose/hooks.yaml:
#   hooks:
#     session_start:
#       - command: "/etc/augmentum/hooks/augmentum-session-start.sh"
#         timeout: 10
#
# Output: {"context_injection": "..."} or plain text

INPUT=$(cat)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)

parts=()

# --- Augmentum OS runtime context (/run/augmentum/) ---
if [[ -d /run/augmentum ]]; then
    # Active node identity
    if [[ -f /run/augmentum/node-id ]]; then
        node_id=$(cat /run/augmentum/node-id 2>/dev/null)
        [[ -n "$node_id" ]] && parts+=("Node: $node_id")
    fi

    # Safety/compliance state
    if [[ -f /run/augmentum/compliance.json ]]; then
        compliance=$(jq -r '.tier // empty' /run/augmentum/compliance.json 2>/dev/null)
        [[ -n "$compliance" ]] && parts+=("Compliance tier: $compliance")
    fi

    # Active permits (what this session is authorized to do)
    if [[ -f /run/augmentum/permits.json ]]; then
        permit_summary=$(jq -r '[.active[].scope] | join(", ")' /run/augmentum/permits.json 2>/dev/null)
        [[ -n "$permit_summary" ]] && parts+=("Active permits: $permit_summary")
    fi
fi

# --- Fleet model routing context ---
if command -v curl &>/dev/null; then
    gateway_models=$(curl -sf --max-time 2 \
        -H "Authorization: Bearer litellm-local-key" \
        http://localhost:4000/v1/models 2>/dev/null \
        | jq -r '[.data[].id] | join(", ")' 2>/dev/null)
    if [[ -n "$gateway_models" ]]; then
        parts+=("LiteLLM gateway online. Available models: $gateway_models")
    fi
fi

# --- Local git context (if in a project directory) ---
if command -v git &>/dev/null && git rev-parse --git-dir &>/dev/null 2>&1; then
    repo=$(basename "$(git rev-parse --show-toplevel 2>/dev/null)" 2>/dev/null)
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
    if [[ -n "$repo" && -n "$branch" ]]; then
        parts+=("Working repo: $repo on $branch")
    fi
fi

# Emit context injection (or nothing if empty)
if [[ ${#parts[@]} -gt 0 ]]; then
    context=$(printf '%s\n' "${parts[@]}" | paste -sd $'\n')
    jq -n --arg ctx "$context" '{"context_injection": $ctx}'
fi
