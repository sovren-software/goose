//! HookInspector — PreToolUse hooks via the `ToolInspector` trait.
//!
//! Integrates with Goose's existing tool inspection pipeline, running
//! after all built-in inspectors (Security, Permission, Repetition).
//! Hook decisions map directly to `InspectionAction`:
//!
//! - `"block"` → `InspectionAction::Deny`
//! - `"require_approval"` → `InspectionAction::RequireApproval`
//! - `"allow"` or any other value → `InspectionAction::Allow`
//!
//! Fail-open: hook errors or timeouts produce no inspection results,
//! allowing the tool to proceed.

use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::config::{load_hooks, HookEntry};
use super::executor::{run_hook, HookOutput};
use crate::config::GooseMode;
use crate::conversation::message::{Message, ToolRequest};
use crate::tool_inspection::{InspectionAction, InspectionResult, ToolInspector};

/// Tool inspector that delegates PreToolUse decisions to external hook processes.
pub struct HookInspector {
    hooks: Vec<HookEntry>,
    compiled_filters: Vec<Option<Regex>>,
    /// Session ID for PreToolUse payloads. Set via `set_session_id` after construction.
    session_id: Arc<RwLock<Option<String>>>,
}

impl HookInspector {
    /// Create a new HookInspector from the current hook configuration.
    pub fn from_config() -> Self {
        let config = load_hooks();
        let hooks = config.pre_tool_use;
        let compiled_filters = hooks
            .iter()
            .map(|h| h.tool_name.as_ref().and_then(|p| Regex::new(p).ok()))
            .collect();
        Self {
            hooks,
            compiled_filters,
            session_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Returns true if any PreToolUse hooks are configured.
    pub fn has_hooks(&self) -> bool {
        !self.hooks.is_empty()
    }

    /// Update the session ID included in PreToolUse hook payloads.
    ///
    /// Called by Agent when a session begins or when the session ID is first known.
    pub fn set_session_id(&self, id: &str) {
        if let Ok(mut guard) = self.session_id.write() {
            *guard = Some(id.to_string());
        }
    }
}

#[async_trait]
impl ToolInspector for HookInspector {
    fn name(&self) -> &'static str {
        "hook"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn inspect(
        &self,
        tool_requests: &[ToolRequest],
        _messages: &[Message],
        _goose_mode: GooseMode,
    ) -> Result<Vec<InspectionResult>> {
        let mut results = Vec::new();

        let current_session_id = self
            .session_id
            .read()
            .ok()
            .and_then(|g| g.clone());

        for request in tool_requests {
            // Skip requests where the tool call failed to parse — there is no
            // meaningful tool_name to filter on, and running hooks against an
            // empty name string would incorrectly match ".*" wildcard filters.
            let tool_call = match request.tool_call.as_ref() {
                Ok(tc) => tc,
                Err(_) => continue,
            };

            let tool_name: &str = tool_call.name.as_ref();
            let tool_arguments = tool_call
                .arguments
                .as_ref()
                .map(|args| serde_json::Value::Object(args.clone()));

            let mut blocked = false;

            for (hook, filter) in self.hooks.iter().zip(&self.compiled_filters) {
                // Skip if filter exists and doesn't match the tool name
                if let Some(re) = filter {
                    if !re.is_match(tool_name) {
                        continue;
                    }
                }

                let payload = json!({
                    "event": "pre_tool_use",
                    "session_id": current_session_id,
                    "tool_name": tool_name,
                    "tool_arguments": tool_arguments,
                });
                let hook_timeout = Duration::from_secs(hook.timeout);

                match run_hook(&hook.command, &payload, hook_timeout).await {
                    Some(HookOutput::Decision { action, reason }) => {
                        let inspection_action = match action.as_str() {
                            "block" => InspectionAction::Deny,
                            "require_approval" => InspectionAction::RequireApproval(reason.clone()),
                            _ => InspectionAction::Allow,
                        };

                        // Only emit result for non-Allow decisions
                        if inspection_action != InspectionAction::Allow {
                            results.push(InspectionResult {
                                tool_request_id: request.id.clone(),
                                action: inspection_action,
                                reason: reason.unwrap_or_default(),
                                confidence: 1.0,
                                inspector_name: "hook".to_string(),
                                finding_id: None,
                            });
                        }

                        // Short-circuit on block
                        if action == "block" {
                            blocked = true;
                            break;
                        }
                    }
                    _ => {
                        // Fail-open: hook error, timeout, or no decision → Allow
                    }
                }
            }

            if blocked {
                // Already emitted Deny result; skip remaining hooks for this request
                continue;
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolRequestParams;
    use rmcp::object;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_test_hook(name: &str, script: &str) -> String {
        let path = format!("/tmp/goose-test-inspector-{}.sh", name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn cleanup_test_hook(path: &str) {
        let _ = fs::remove_file(path);
    }

    fn make_tool_request(name: &str) -> ToolRequest {
        ToolRequest {
            id: format!("req_{}", name),
            tool_call: Ok(CallToolRequestParams {
                meta: None,
                task: None,
                name: name.to_owned().into(),
                arguments: Some(object!({})),
            }),
            metadata: None,
            tool_meta: None,
        }
    }

    #[tokio::test]
    async fn test_hook_inspector_deny() {
        let path = write_test_hook(
            "deny",
            "#!/bin/bash\necho '{\"decision\": \"block\", \"reason\": \"policy violation\"}'",
        );
        let inspector = HookInspector {
            hooks: vec![HookEntry {
                command: path.clone(),
                timeout: 5,
                tool_name: None,
            }],
            compiled_filters: vec![None],
            session_id: Arc::new(RwLock::new(None)),
        };

        let requests = vec![make_tool_request("write_file")];
        let results = inspector
            .inspect(&requests, &[], GooseMode::Auto)
            .await
            .unwrap();
        cleanup_test_hook(&path);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action, InspectionAction::Deny);
        assert_eq!(results[0].reason, "policy violation");
        assert_eq!(results[0].inspector_name, "hook");
    }

    #[tokio::test]
    async fn test_hook_inspector_require_approval() {
        let path = write_test_hook(
            "approve",
            "#!/bin/bash\necho '{\"decision\": \"require_approval\", \"reason\": \"needs review\"}'",
        );
        let inspector = HookInspector {
            hooks: vec![HookEntry {
                command: path.clone(),
                timeout: 5,
                tool_name: None,
            }],
            compiled_filters: vec![None],
            session_id: Arc::new(RwLock::new(None)),
        };

        let requests = vec![make_tool_request("shell")];
        let results = inspector
            .inspect(&requests, &[], GooseMode::Auto)
            .await
            .unwrap();
        cleanup_test_hook(&path);

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].action,
            InspectionAction::RequireApproval(_)
        ));
    }

    #[tokio::test]
    async fn test_hook_inspector_failopen() {
        let path = write_test_hook("failopen", "#!/bin/bash\nexit 1");
        let inspector = HookInspector {
            hooks: vec![HookEntry {
                command: path.clone(),
                timeout: 5,
                tool_name: None,
            }],
            compiled_filters: vec![None],
            session_id: Arc::new(RwLock::new(None)),
        };

        let requests = vec![make_tool_request("read_file")];
        let results = inspector
            .inspect(&requests, &[], GooseMode::Auto)
            .await
            .unwrap();
        cleanup_test_hook(&path);

        assert!(
            results.is_empty(),
            "Hook failure should produce no results (fail-open)"
        );
    }

    #[tokio::test]
    async fn test_tool_name_regex_match() {
        let path = write_test_hook(
            "regex-match",
            "#!/bin/bash\necho '{\"decision\": \"block\", \"reason\": \"write blocked\"}'",
        );
        let filter = Regex::new("write_file").ok();
        let inspector = HookInspector {
            hooks: vec![HookEntry {
                command: path.clone(),
                timeout: 5,
                tool_name: Some("write_file".to_string()),
            }],
            compiled_filters: vec![filter],
            session_id: Arc::new(RwLock::new(None)),
        };

        // Should match
        let write_req = vec![make_tool_request("write_file")];
        let results = inspector
            .inspect(&write_req, &[], GooseMode::Auto)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "write_file should be blocked");

        // Should NOT match
        let read_req = vec![make_tool_request("read_file")];
        let results = inspector
            .inspect(&read_req, &[], GooseMode::Auto)
            .await
            .unwrap();
        cleanup_test_hook(&path);

        assert!(results.is_empty(), "read_file should not be blocked");
    }

    #[tokio::test]
    async fn test_tool_name_regex_wildcard() {
        let path = write_test_hook("wildcard", "#!/bin/bash\necho '{\"decision\": \"block\"}'");
        let filter = Regex::new(".*").ok();
        let inspector = HookInspector {
            hooks: vec![HookEntry {
                command: path.clone(),
                timeout: 5,
                tool_name: Some(".*".to_string()),
            }],
            compiled_filters: vec![filter],
            session_id: Arc::new(RwLock::new(None)),
        };

        let requests = vec![make_tool_request("anything")];
        let results = inspector
            .inspect(&requests, &[], GooseMode::Auto)
            .await
            .unwrap();
        cleanup_test_hook(&path);

        assert_eq!(results.len(), 1, ".* should match any tool");
    }

    #[tokio::test]
    async fn test_multiple_hooks_most_restrictive() {
        let allow_path = write_test_hook(
            "multi-allow",
            "#!/bin/bash\necho '{\"decision\": \"allow\"}'",
        );
        let block_path = write_test_hook(
            "multi-block",
            "#!/bin/bash\necho '{\"decision\": \"block\", \"reason\": \"blocked\"}'",
        );
        let inspector = HookInspector {
            hooks: vec![
                HookEntry {
                    command: allow_path.clone(),
                    timeout: 5,
                    tool_name: None,
                },
                HookEntry {
                    command: block_path.clone(),
                    timeout: 5,
                    tool_name: None,
                },
            ],
            compiled_filters: vec![None, None],
            session_id: Arc::new(RwLock::new(None)),
        };

        let requests = vec![make_tool_request("shell")];
        let results = inspector
            .inspect(&requests, &[], GooseMode::Auto)
            .await
            .unwrap();
        cleanup_test_hook(&allow_path);
        cleanup_test_hook(&block_path);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action, InspectionAction::Deny);
    }
}
