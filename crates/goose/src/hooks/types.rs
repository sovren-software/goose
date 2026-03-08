use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Lifecycle events emitted by the agent. This is the ONLY type
/// that crosses the hooks/agent boundary. Zero rmcp imports.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "hook_event_name", rename_all = "PascalCase")]
pub enum HookEvent {
    SessionStart {
        session_id: String,
        cwd: PathBuf,
    },
    UserPromptSubmit {
        session_id: String,
        user_prompt: String,
        cwd: PathBuf,
    },
    PreToolUse {
        session_id: String,
        tool_name: String,
        tool_input: Value,
        cwd: PathBuf,
    },
    PostToolUse {
        session_id: String,
        tool_name: String,
        tool_input: Value,
        tool_output: String,
        cwd: PathBuf,
    },
    PostToolUseFailure {
        session_id: String,
        tool_name: String,
        tool_input: Value,
        tool_error: String,
        cwd: PathBuf,
    },
    PreCompact {
        session_id: String,
        message_count: usize,
        manual: bool,
        cwd: PathBuf,
    },
    PostCompact {
        session_id: String,
        before_count: usize,
        after_count: usize,
        manual: bool,
        cwd: PathBuf,
    },
    Stop {
        session_id: String,
        last_assistant_text: String,
        cwd: PathBuf,
    },
}

impl HookEvent {
    /// Returns the event kind string matching config keys.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStart { .. } => "SessionStart",
            Self::UserPromptSubmit { .. } => "UserPromptSubmit",
            Self::PreToolUse { .. } => "PreToolUse",
            Self::PostToolUse { .. } => "PostToolUse",
            Self::PostToolUseFailure { .. } => "PostToolUseFailure",
            Self::PreCompact { .. } => "PreCompact",
            Self::PostCompact { .. } => "PostCompact",
            Self::Stop { .. } => "Stop",
        }
    }

    /// Whether this event type supports blocking (exit 2).
    pub fn is_blockable(&self) -> bool {
        matches!(
            self,
            Self::UserPromptSubmit { .. } | Self::PreToolUse { .. } | Self::PreCompact { .. } | Self::Stop { .. }
        )
    }

    /// Returns the tool_name for tool-related events.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::PreToolUse { tool_name, .. }
            | Self::PostToolUse { tool_name, .. }
            | Self::PostToolUseFailure { tool_name, .. } => Some(tool_name),
            _ => None,
        }
    }

    /// Returns the tool_input for tool-related events.
    pub fn tool_input(&self) -> Option<&Value> {
        match self {
            Self::PreToolUse { tool_input, .. }
            | Self::PostToolUse { tool_input, .. }
            | Self::PostToolUseFailure { tool_input, .. } => Some(tool_input),
            _ => None,
        }
    }

    /// Returns manual flag for compact events.
    pub fn is_manual_compact(&self) -> bool {
        match self {
            Self::PreCompact { manual, .. } | Self::PostCompact { manual, .. } => *manual,
            _ => false,
        }
    }
}

/// Result of running all hooks for an event.
#[derive(Debug, Default)]
pub struct HookOutcome {
    /// true if ANY hook returned Block (exit code 2).
    pub blocked: bool,
    /// Concatenated additional_context from all hooks.
    pub context: Option<String>,
}

/// Deserialized from hook stdout JSON.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct HookResult {
    #[serde(default)]
    pub decision: Option<HookDecision>,

    #[serde(default)]
    #[allow(dead_code)]
    pub reason: Option<String>,

    #[serde(default, alias = "additionalContext")]
    pub additional_context: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HookDecision {
    Allow,
    Block,
}

/// Raw output from a hook subprocess.
#[derive(Debug)]
pub(crate) struct HookCommandOutput {
    pub stdout: String,
    #[allow(dead_code)]
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_kind_matches_config_keys() {
        let event = HookEvent::SessionStart {
            session_id: "s1".into(),
            cwd: "/tmp".into(),
        };
        assert_eq!(event.kind(), "SessionStart");
        assert!(!event.is_blockable());
    }

    #[test]
    fn blockable_events() {
        let event = HookEvent::PreToolUse {
            session_id: "s1".into(),
            tool_name: "test".into(),
            tool_input: Value::Null,
            cwd: "/tmp".into(),
        };
        assert!(event.is_blockable());
        assert_eq!(event.tool_name(), Some("test"));
    }

    #[test]
    fn hook_event_serializes_with_tag() {
        let event = HookEvent::PreToolUse {
            session_id: "s1".into(),
            tool_name: "developer__shell".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            cwd: "/tmp".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["hook_event_name"], "PreToolUse");
        assert_eq!(json["session_id"], "s1");
        assert_eq!(json["tool_name"], "developer__shell");
    }

    #[test]
    fn hook_event_fields_are_snake_case() {
        // Contrib hooks parse stdin JSON with jq using snake_case field names.
        // This test guards against serde rename_all accidentally changing them.
        let event = HookEvent::PostToolUse {
            session_id: "s1".into(),
            tool_name: "developer__shell".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            tool_output: "files".into(),
            cwd: "/tmp".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("session_id"), "expected snake_case session_id");
        assert!(obj.contains_key("tool_name"), "expected snake_case tool_name");
        assert!(obj.contains_key("tool_input"), "expected snake_case tool_input");
        assert!(obj.contains_key("tool_output"), "expected snake_case tool_output");
        assert!(obj.contains_key("cwd"), "expected lowercase cwd");
        assert!(!obj.contains_key("SessionId"), "PascalCase field names would break contrib hooks");
    }

    #[test]
    fn stop_event_includes_last_assistant_text() {
        let event = HookEvent::Stop {
            session_id: "s1".into(),
            last_assistant_text: "I completed the task.".into(),
            cwd: "/tmp".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["hook_event_name"], "Stop");
        assert_eq!(json["last_assistant_text"], "I completed the task.");
        assert!(json.get("session_id").is_some());
    }

    #[test]
    fn hook_result_accepts_camel_case_context() {
        // Contrib hooks emit "additionalContext" (camelCase, Claude Code convention).
        // The serde alias must accept both forms.
        let json = r#"{"additionalContext": "from camelCase"}"#;
        let result: HookResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.additional_context.as_deref(), Some("from camelCase"));
    }

    #[test]
    fn hook_result_deserializes_from_json() {
        let json = r#"{"decision": "block", "reason": "not allowed", "additional_context": "ctx"}"#;
        let result: HookResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.decision, Some(HookDecision::Block));
        assert_eq!(result.reason.as_deref(), Some("not allowed"));
        assert_eq!(result.additional_context.as_deref(), Some("ctx"));
    }
}
