mod config;
mod subprocess;
pub mod types;

pub use types::{HookEvent, HookOutcome};

use config::{HookAction, HooksConfig};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use types::{HookDecision, HookResult};

const MAX_CONTEXT_LEN: usize = 32_768;

/// Stable hook execution runtime. Routes lifecycle events to
/// user-configured shell scripts via direct subprocess execution.
/// Zero rmcp imports — decoupled from agent internals.
pub struct HookRuntime {
    config: HooksConfig,
}

impl HookRuntime {
    /// Load hook configuration. Returns a no-op runtime if config is absent.
    pub fn load(working_dir: &Path) -> Self {
        let config = HooksConfig::load_merged(working_dir).unwrap_or_else(|e| {
            tracing::debug!("No hooks config loaded: {}", e);
            HooksConfig::default()
        });
        Self { config }
    }

    /// Emit a lifecycle event. Runs all matching hooks, returns aggregated outcome.
    pub async fn emit(
        &self,
        event: HookEvent,
        working_dir: &Path,
        cancel_token: CancellationToken,
    ) -> HookOutcome {
        let event_configs = self.config.get_hooks_for_event(event.kind());
        if event_configs.is_empty() {
            tracing::info!("No hooks configured for event {}", event.kind());
            return HookOutcome::default();
        }
        tracing::info!(
            "Running {} hook group(s) for event {}",
            event_configs.len(),
            event.kind()
        );

        let stdin_json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!("Failed to serialize hook event: {}", e);
                return HookOutcome::default();
            }
        };

        let mut outcome = HookOutcome::default();
        let mut contexts = Vec::new();

        for event_config in event_configs {
            if !Self::matches_config(event_config, &event) {
                continue;
            }

            for action in &event_config.hooks {
                match action {
                    HookAction::Command { command, timeout } => {
                        let result = subprocess::run_hook_command(
                            command,
                            Some(&stdin_json),
                            *timeout,
                            working_dir,
                            cancel_token.clone(),
                        )
                        .await;

                        match result {
                            Ok(output) => {
                                tracing::info!(
                                    "Hook for {} exited {:?}, stdout {} bytes",
                                    event.kind(),
                                    output.exit_code,
                                    output.stdout.len()
                                );
                                if output.timed_out {
                                    tracing::warn!(
                                        "Hook timed out after {}s, failing open",
                                        timeout
                                    );
                                    continue;
                                }

                                match output.exit_code {
                                    Some(0) => {
                                        // Parse JSON result or treat as context
                                        if let Some(hook_result) =
                                            Self::parse_stdout(&output.stdout, event.is_blockable())
                                        {
                                            // Honor JSON decision:"block" at exit 0 (Claude Code compat)
                                            if hook_result.decision == Some(HookDecision::Block)
                                                && event.is_blockable()
                                            {
                                                outcome.blocked = true;
                                                tracing::info!(
                                                    "Hook blocked event {} (JSON decision)",
                                                    event.kind()
                                                );
                                                return outcome;
                                            }
                                            if let Some(ctx) = hook_result.additional_context {
                                                contexts.push(ctx);
                                            }
                                        }
                                    }
                                    Some(2) if event.is_blockable() => {
                                        outcome.blocked = true;
                                        tracing::info!(
                                            "Hook blocked event {} (exit 2)",
                                            event.kind()
                                        );
                                        return outcome;
                                    }
                                    Some(code) => {
                                        tracing::debug!(
                                            "Hook exited with code {}, failing open",
                                            code
                                        );
                                    }
                                    None => {
                                        tracing::debug!("Hook killed (no exit code), failing open");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Hook execution failed: {}, failing open", e);
                            }
                        }
                    }
                }
            }
        }

        if !contexts.is_empty() {
            tracing::info!(
                "Hook {} produced {} context chunk(s), total {} bytes",
                event.kind(),
                contexts.len(),
                contexts.iter().map(|c| c.len()).sum::<usize>()
            );
            let mut joined = contexts.join("\n");
            if joined.len() > MAX_CONTEXT_LEN {
                tracing::warn!(
                    "Hook context truncated from {} to {}",
                    joined.len(),
                    MAX_CONTEXT_LEN
                );
                joined.truncate(joined.floor_char_boundary(MAX_CONTEXT_LEN));
            }
            outcome.context = Some(joined);
        }

        outcome
    }

    /// Parse stdout from a hook that exited 0.
    fn parse_stdout(stdout: &str, is_blockable: bool) -> Option<HookResult> {
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Some(HookResult::default());
        }

        // Try JSON parse first
        if let Ok(result) = serde_json::from_str::<HookResult>(trimmed) {
            // Check for block decision in JSON
            if result.decision == Some(HookDecision::Block) && is_blockable {
                // This shouldn't happen for exit-0 (block should use exit 2),
                // but handle it for compatibility
                return Some(result);
            }
            return Some(result);
        }

        // Non-JSON stdout from exit-0 → surface as context
        let mut context = trimmed.to_string();
        if context.len() > MAX_CONTEXT_LEN {
            tracing::warn!(
                "Hook stdout truncated from {} to {}",
                context.len(),
                MAX_CONTEXT_LEN
            );
            context.truncate(context.floor_char_boundary(MAX_CONTEXT_LEN));
        }
        Some(HookResult {
            additional_context: Some(context),
            ..Default::default()
        })
    }

    fn matches_config(config: &config::HookEventConfig, event: &HookEvent) -> bool {
        let Some(pattern) = &config.matcher else {
            return true;
        };

        match event {
            HookEvent::PreToolUse { .. }
            | HookEvent::PostToolUse { .. }
            | HookEvent::PostToolUseFailure { .. } => Self::matches_tool(pattern, event),
            HookEvent::PreCompact { .. } | HookEvent::PostCompact { .. } => {
                (event.is_manual_compact() && pattern == "manual")
                    || (!event.is_manual_compact() && pattern == "auto")
            }
            _ => true,
        }
    }

    /// Match a tool invocation against a Claude Code-style matcher pattern.
    /// Supports:
    ///   "Bash" or "Bash(...)" — maps to shell / developer__shell
    ///   "tool_name" — direct tool name match
    fn matches_tool(pattern: &str, event: &HookEvent) -> bool {
        let tool_name = match event.tool_name() {
            Some(name) => name,
            None => return false,
        };

        let is_shell = tool_name == "shell" || tool_name == "developer__shell";

        // Claude Code "Bash" syntax
        if pattern == "Bash" {
            return is_shell;
        }

        // Claude Code "Bash(pattern)" syntax
        if let Some(inner) = pattern
            .strip_prefix("Bash(")
            .and_then(|s| s.strip_suffix(')'))
        {
            if !is_shell {
                return false;
            }
            let command_str = event
                .tool_input()
                .and_then(|v| v.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return Self::glob_match(inner, command_str);
        }

        // Direct tool name match
        tool_name == pattern
    }

    fn glob_match(pattern: &str, text: &str) -> bool {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(text))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_tool_bash_shorthand() {
        let event = HookEvent::PreToolUse {
            session_id: "s1".into(),
            tool_name: "developer__shell".into(),
            tool_input: json!({"command": "ls -la"}),
            cwd: "/tmp".into(),
        };
        assert!(HookRuntime::matches_tool("Bash", &event));
        assert!(HookRuntime::matches_tool("Bash(ls*)", &event));
        assert!(!HookRuntime::matches_tool("Bash(rm*)", &event));
    }

    #[test]
    fn matches_tool_shell_platform_name() {
        // Platform extensions register as "shell" (not "developer__shell").
        // Both forms must match Bash shorthand and Bash(glob) patterns.
        let event = HookEvent::PreToolUse {
            session_id: "s1".into(),
            tool_name: "shell".into(),
            tool_input: json!({"command": "cargo build"}),
            cwd: "/tmp".into(),
        };
        assert!(HookRuntime::matches_tool("Bash", &event));
        assert!(HookRuntime::matches_tool("Bash(cargo*)", &event));
        assert!(!HookRuntime::matches_tool("Bash(rm*)", &event));
        assert!(HookRuntime::matches_tool("shell", &event));
    }

    #[test]
    fn matches_tool_direct_name() {
        let event = HookEvent::PreToolUse {
            session_id: "s1".into(),
            tool_name: "slack__post_message".into(),
            tool_input: json!({}),
            cwd: "/tmp".into(),
        };
        assert!(HookRuntime::matches_tool("slack__post_message", &event));
        assert!(!HookRuntime::matches_tool("Bash", &event));
    }

    #[test]
    fn parse_stdout_json() {
        let result =
            HookRuntime::parse_stdout(r#"{"additional_context": "injected"}"#, false).unwrap();
        assert_eq!(result.additional_context.as_deref(), Some("injected"));
    }

    #[test]
    fn parse_stdout_plain_text() {
        let result = HookRuntime::parse_stdout("plain context text", false).unwrap();
        assert_eq!(
            result.additional_context.as_deref(),
            Some("plain context text")
        );
    }

    #[test]
    fn parse_stdout_empty() {
        let result = HookRuntime::parse_stdout("", false).unwrap();
        assert!(result.additional_context.is_none());
    }

    #[test]
    fn parse_stdout_block_decision_at_exit_0() {
        // A hook that exits 0 but returns decision:block — Claude Code compat.
        // emit() checks this field and sets outcome.blocked for blockable events.
        let result = HookRuntime::parse_stdout(
            r#"{"decision": "block", "reason": "no"}"#,
            true,
        )
        .unwrap();
        assert_eq!(result.decision, Some(HookDecision::Block));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn emit_honors_json_block_at_exit_0() {
        // Integration test: a hook that exits 0 with {"decision":"block"}
        // must set outcome.blocked = true. This was a dead code path before
        // the fix — parse_stdout returned the decision but emit() ignored it.
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();

        // Write a hook script that exits 0 with block decision
        let hook_script = dir.path().join("block-hook.sh");
        let mut f = std::fs::File::create(&hook_script).unwrap();
        writeln!(f, "#!/bin/bash").unwrap();
        writeln!(f, r#"echo '{{"decision":"block","reason":"test"}}'"#).unwrap();
        writeln!(f, "exit 0").unwrap();
        drop(f);
        std::fs::set_permissions(
            &hook_script,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();

        // Write a hooks.json that wires this script to UserPromptSubmit
        let config_path = dir.path().join("hooks.json");
        let config = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [{
                    "hooks": [{
                        "type": "command",
                        "command": hook_script.to_str().unwrap(),
                        "timeout": 5
                    }]
                }]
            }
        });
        std::fs::write(&config_path, config.to_string()).unwrap();

        let runtime = HookRuntime {
            config: serde_json::from_str(&config.to_string()).unwrap(),
        };

        let event = HookEvent::UserPromptSubmit {
            session_id: "test".into(),
            user_prompt: "hello".into(),
            cwd: dir.path().to_path_buf(),
        };

        let outcome = runtime
            .emit(event, dir.path(), CancellationToken::new())
            .await;
        assert!(outcome.blocked, "exit-0 JSON block decision must set outcome.blocked");
    }
}
