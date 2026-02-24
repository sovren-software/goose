//! Lifecycle hook system for Goose.
//!
//! Provides 5 lifecycle events that external hook processes can subscribe to:
//!
//! | Event | Mechanism | Hook Output |
//! |-------|-----------|-------------|
//! | `session_start` | Context injection via `extend_system_prompt` | `context_injection` |
//! | `prompt_submit` | Context injection (overwrites per turn) | `context_injection` |
//! | `pre_tool_use` | `ToolInspector` pipeline integration | `decision` (allow/block/require_approval) |
//! | `post_tool_use` | Fire-and-forget (`tokio::spawn`) | stdout ignored |
//! | `session_stop` | Best-effort on exit | stdout ignored |
//!
//! Hooks are configured in `config.yaml` under the `hooks` key.
//! All hooks receive JSON on stdin and return JSON on stdout.
//! Hook failures are fail-open: errors are logged but never propagate.

pub mod config;
pub mod executor;
pub mod inspector;

use serde_json::json;

use config::{load_hooks, HookEntry};
use executor::{run_context_hooks, run_fire_and_forget_hooks};

/// Run SessionStart hooks and return concatenated context injection text.
///
/// Called once when a session begins. The returned text is injected into
/// the system prompt via `Agent::extend_system_prompt()`.
pub async fn run_session_start_hooks(session_id: &str) -> Option<String> {
    let config = load_hooks();
    if config.session_start.is_empty() {
        // Fall back to env var for backward compatibility with E2 spike
        return run_session_start_hooks_env(session_id).await;
    }

    let payload = json!({
        "event": "session_start",
        "session_id": session_id,
    });

    run_context_hooks(&config.session_start, &payload).await
}

/// Run PromptSubmit hooks and return concatenated context injection text.
///
/// Called on each user message. The returned text overwrites the previous
/// injection (same key `"hook_prompt_submit"` in `extend_system_prompt`).
pub async fn run_prompt_submit_hooks(session_id: &str, prompt_text: &str) -> Option<String> {
    let config = load_hooks();
    if config.prompt_submit.is_empty() {
        return None;
    }

    let payload = json!({
        "event": "prompt_submit",
        "session_id": session_id,
        "prompt_text": prompt_text,
    });

    run_context_hooks(&config.prompt_submit, &payload).await
}

/// Run PostToolUse hooks as fire-and-forget.
///
/// Called after each tool call completes. Spawned in a `tokio::spawn` task
/// so it does not block the agent loop.
pub async fn run_post_tool_use_hooks(
    session_id: &str,
    tool_name: &str,
    tool_arguments: &serde_json::Value,
    tool_result: Option<&str>,
    tool_error: Option<&str>,
) {
    let config = load_hooks();
    let hooks: Vec<HookEntry> = config
        .post_tool_use
        .into_iter()
        .filter(|h| tool_name_matches(h, tool_name))
        .collect();

    if hooks.is_empty() {
        return;
    }

    let payload = json!({
        "event": "post_tool_use",
        "session_id": session_id,
        "tool_name": tool_name,
        "tool_arguments": tool_arguments,
        "tool_result": tool_result,
        "tool_error": tool_error,
    });

    run_fire_and_forget_hooks(&hooks, &payload).await;
}

/// Run SessionStop hooks as fire-and-forget with a short timeout.
///
/// Called when a session ends. Best-effort: if hooks don't complete
/// within their configured timeout, they are killed.
pub async fn run_session_stop_hooks(session_id: &str) {
    let config = load_hooks();
    if config.session_stop.is_empty() {
        return;
    }

    let payload = json!({
        "event": "session_stop",
        "session_id": session_id,
    });

    run_fire_and_forget_hooks(&config.session_stop, &payload).await;
}

/// Check if a hook's tool_name filter matches the given tool name.
fn tool_name_matches(hook: &HookEntry, tool_name: &str) -> bool {
    match &hook.tool_name {
        None => true, // No filter = match all
        Some(pattern) => regex::Regex::new(pattern)
            .map(|re| re.is_match(tool_name))
            .unwrap_or(true), // Invalid regex = fail-open
    }
}

/// Backward-compatible env var fallback for SessionStart hooks.
///
/// Supports `GOOSE_SESSION_START_HOOK` environment variable from the E2 spike.
/// This allows existing users to migrate gradually to the config-based system.
async fn run_session_start_hooks_env(session_id: &str) -> Option<String> {
    let hook_cmd = std::env::var("GOOSE_SESSION_START_HOOK").ok()?;
    if hook_cmd.trim().is_empty() {
        return None;
    }

    let payload = json!({
        "event": "session_start",
        "session_id": session_id,
    });

    let hook_entry = HookEntry {
        command: hook_cmd,
        timeout: 10,
        tool_name: None,
    };

    run_context_hooks(&[hook_entry], &payload).await
}
