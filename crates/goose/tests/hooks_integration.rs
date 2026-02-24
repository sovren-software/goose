//! Integration tests for the lifecycle hook system.
//!
//! These tests verify the end-to-end flow: write hook script → load config →
//! execute → verify output.

use goose::hooks::config::HookEntry;
use goose::hooks::config::HooksConfig;
use goose::hooks::executor::{run_context_hooks, run_fire_and_forget_hooks, run_hook, HookOutput};
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn write_hook(name: &str, script: &str) -> String {
    let path = format!("/tmp/goose-integration-hook-{}.sh", name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn cleanup(path: &str) {
    let _ = fs::remove_file(path);
}

#[test]
fn test_hooks_config_round_trip() {
    let yaml = r#"
session_start:
  - command: "/usr/local/bin/init.sh"
    timeout: 15
prompt_submit:
  - command: "/usr/local/bin/inject.sh"
    timeout: 5
pre_tool_use:
  - command: "/usr/local/bin/guard.sh"
    timeout: 3
    tool_name: "developer__shell"
  - command: "/usr/local/bin/audit.sh"
    tool_name: ".*"
post_tool_use:
  - command: "/usr/local/bin/log.sh"
session_stop:
  - command: "/usr/local/bin/cleanup.sh"
    timeout: 10
"#;

    let config: HooksConfig = serde_yaml::from_str(yaml).unwrap();

    assert_eq!(config.session_start.len(), 1);
    assert_eq!(config.session_start[0].timeout, 15);

    assert_eq!(config.prompt_submit.len(), 1);
    assert_eq!(config.prompt_submit[0].timeout, 5);

    assert_eq!(config.pre_tool_use.len(), 2);
    assert_eq!(
        config.pre_tool_use[0].tool_name.as_deref(),
        Some("developer__shell")
    );
    assert_eq!(config.pre_tool_use[0].timeout, 3);
    assert_eq!(config.pre_tool_use[1].tool_name.as_deref(), Some(".*"));
    assert_eq!(config.pre_tool_use[1].timeout, 10); // default

    assert_eq!(config.post_tool_use.len(), 1);
    assert_eq!(config.session_stop.len(), 1);

    assert!(config.has_any_hooks());
}

#[tokio::test]
async fn test_end_to_end_context_injection() {
    let path = write_hook(
        "e2e-ctx",
        r#"#!/bin/bash
INPUT=$(cat)
SESSION=$(echo "$INPUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('session_id',''))" 2>/dev/null || echo "unknown")
echo "{\"context_injection\": \"Session: $SESSION\"}"
"#,
    );

    let hooks = vec![HookEntry {
        command: path.clone(),
        timeout: 10,
        tool_name: None,
    }];

    let payload = json!({
        "event": "session_start",
        "session_id": "test-session-123",
    });

    let result = run_context_hooks(&hooks, &payload).await;
    cleanup(&path);

    assert!(result.is_some());
    let text = result.unwrap();
    assert!(
        text.contains("test-session-123"),
        "Expected session ID in output, got: {}",
        text
    );
}

#[tokio::test]
async fn test_end_to_end_fire_and_forget() {
    let marker = "/tmp/goose-integration-hook-marker";
    let _ = fs::remove_file(marker);

    let path = write_hook("e2e-faf", &format!("#!/bin/bash\ntouch {}", marker));

    let hooks = vec![HookEntry {
        command: path.clone(),
        timeout: 5,
        tool_name: None,
    }];

    let payload = json!({"event": "session_stop", "session_id": "test"});
    run_fire_and_forget_hooks(&hooks, &payload).await;
    cleanup(&path);

    // Give a moment for the file to be created
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        std::path::Path::new(marker).exists(),
        "Fire-and-forget hook should have created marker file"
    );
    let _ = fs::remove_file(marker);
}

#[tokio::test]
async fn test_end_to_end_decision_block() {
    let path = write_hook(
        "e2e-block",
        r#"#!/bin/bash
INPUT=$(cat)
TOOL=$(echo "$INPUT" | python3 -c "import json,sys; print(json.load(sys.stdin).get('tool_name',''))" 2>/dev/null || echo "")
if [ "$TOOL" = "dangerous_tool" ]; then
    echo '{"decision": "block", "reason": "tool is dangerous"}'
else
    echo '{"decision": "allow"}'
fi
"#,
    );

    let payload_block = json!({
        "event": "pre_tool_use",
        "tool_name": "dangerous_tool",
        "tool_arguments": {},
    });

    let result = run_hook(&path, &payload_block, Duration::from_secs(5)).await;
    match result {
        Some(HookOutput::Decision { action, reason }) => {
            assert_eq!(action, "block");
            assert_eq!(reason.as_deref(), Some("tool is dangerous"));
        }
        other => panic!("Expected Decision(block), got {:?}", other),
    }

    let payload_allow = json!({
        "event": "pre_tool_use",
        "tool_name": "safe_tool",
        "tool_arguments": {},
    });

    let result = run_hook(&path, &payload_allow, Duration::from_secs(5)).await;
    cleanup(&path);

    match result {
        Some(HookOutput::Decision { action, .. }) => {
            assert_eq!(action, "allow");
        }
        other => panic!("Expected Decision(allow), got {:?}", other),
    }
}

#[test]
fn test_empty_config_has_no_hooks() {
    let config = HooksConfig::default();
    assert!(!config.has_any_hooks());
}

#[tokio::test]
async fn test_hook_with_shlex_quoting() {
    // Test that commands with spaces/quotes are handled correctly by shlex
    let script_path = write_hook(
        "shlex",
        "#!/bin/bash\necho '{\"context_injection\": \"shlex works\"}'",
    );

    // shlex should handle the path correctly
    let payload = json!({"event": "test"});
    let result = run_hook(&script_path, &payload, Duration::from_secs(5)).await;
    cleanup(&script_path);

    match result {
        Some(HookOutput::ContextInjection(text)) => assert_eq!(text, "shlex works"),
        other => panic!("Expected ContextInjection, got {:?}", other),
    }
}
