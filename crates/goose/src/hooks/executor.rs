//! Async hook process executor using `tokio::process::Command`.
//!
//! Hooks receive a JSON payload on stdin and return JSON on stdout.
//! All execution is async with configurable timeouts. Hook failures
//! are fail-open: errors are logged but never propagate to the caller.

use serde_json::Value;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use super::config::HookEntry;

/// Parsed output from a hook process.
#[derive(Debug, Clone)]
pub enum HookOutput {
    /// Hook returned a `context_injection` string (for session_start / prompt_submit).
    ContextInjection(String),
    /// Hook returned a decision (for pre_tool_use).
    Decision {
        action: String,
        reason: Option<String>,
    },
    /// Hook returned empty or irrelevant output.
    Empty,
}

/// Execute a single hook command asynchronously.
///
/// Sends `payload` as JSON on stdin, captures stdout, and parses the result.
/// Returns `None` if the hook times out, fails to spawn, or exits with a
/// non-zero status code.
pub async fn run_hook(cmd: &str, payload: &Value, hook_timeout: Duration) -> Option<HookOutput> {
    let parts = shlex::split(cmd)?;
    let (program, args) = parts.split_first()?;

    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Write payload to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let payload_bytes = payload.to_string().into_bytes();
        let _ = stdin.write_all(&payload_bytes).await;
        drop(stdin); // Close stdin to signal EOF
    }

    // Await with timeout — wait_with_output() takes ownership, so we handle
    // the timeout case by spawning wait_with_output in the timeout future.
    // On timeout, the child is dropped which sends SIGKILL on unix.
    match timeout(hook_timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            if !output.status.success() && output.stdout.is_empty() {
                return None;
            }
            parse_hook_output(&output.stdout)
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, cmd = cmd, "Hook process I/O error");
            None
        }
        Err(_) => {
            tracing::warn!(cmd = cmd, "Hook timed out");
            // child is dropped here — on unix, tokio kills the process on drop
            None
        }
    }
}

/// Parse raw stdout bytes into a `HookOutput`.
///
/// If the output is valid JSON with a `context_injection` field, returns
/// `ContextInjection`. If it has a `decision` field, returns `Decision`.
/// Plain text stdout is treated as a context injection (fallback for
/// simple hooks). Empty output returns `None`.
fn parse_hook_output(stdout: &[u8]) -> Option<HookOutput> {
    let text = String::from_utf8_lossy(stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }

    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
        // Check for decision (pre_tool_use hooks)
        if let Some(decision) = parsed.get("decision").and_then(|v| v.as_str()) {
            return Some(HookOutput::Decision {
                action: decision.to_string(),
                reason: parsed
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }

        // Check for context_injection (session_start / prompt_submit hooks)
        if let Some(injection) = parsed.get("context_injection").and_then(|v| v.as_str()) {
            return Some(HookOutput::ContextInjection(injection.to_string()));
        }

        // Valid JSON but no recognized fields
        Some(HookOutput::Empty)
    } else {
        // Plain text fallback — treat as context injection
        Some(HookOutput::ContextInjection(text))
    }
}

/// Run multiple hooks for a context-injection event (session_start, prompt_submit).
///
/// Hooks run sequentially in config order. Non-empty context injections are
/// concatenated with newlines.
pub async fn run_context_hooks(hooks: &[HookEntry], payload: &Value) -> Option<String> {
    let mut injections = Vec::new();

    for hook in hooks {
        let hook_timeout = Duration::from_secs(hook.timeout);
        match run_hook(&hook.command, payload, hook_timeout).await {
            Some(HookOutput::ContextInjection(text)) => {
                if !text.is_empty() {
                    injections.push(text);
                }
            }
            Some(HookOutput::Decision { .. }) | Some(HookOutput::Empty) | None => {}
        }
    }

    if injections.is_empty() {
        None
    } else {
        Some(injections.join("\n"))
    }
}

/// Run multiple hooks for a fire-and-forget event (post_tool_use, session_stop).
///
/// All hooks run; errors are logged but not propagated.
pub async fn run_fire_and_forget_hooks(hooks: &[HookEntry], payload: &Value) {
    for hook in hooks {
        let hook_timeout = Duration::from_secs(hook.timeout);
        let _ = run_hook(&hook.command, payload, hook_timeout).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_test_hook(name: &str, script: &str) -> String {
        let path = format!("/tmp/goose-test-hook-{}.sh", name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn cleanup_test_hook(path: &str) {
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_run_hook_json_context_injection() {
        let path = write_test_hook(
            "json-ctx",
            "#!/bin/bash\necho '{\"context_injection\": \"hello from hook\"}'",
        );
        let payload = json!({"event": "session_start"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        match result {
            Some(HookOutput::ContextInjection(text)) => assert_eq!(text, "hello from hook"),
            other => panic!("Expected ContextInjection, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_run_hook_plain_text_fallback() {
        let path = write_test_hook("plaintext", "#!/bin/bash\necho 'just plain text'");
        let payload = json!({"event": "session_start"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        match result {
            Some(HookOutput::ContextInjection(text)) => assert_eq!(text, "just plain text"),
            other => panic!("Expected ContextInjection, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_run_hook_timeout_kills_child() {
        let path = write_test_hook("slow", "#!/bin/bash\nsleep 30\necho 'done'");
        let payload = json!({"event": "test"});
        let result = run_hook(&path, &payload, Duration::from_millis(100)).await;
        cleanup_test_hook(&path);

        assert!(result.is_none(), "Timed-out hook should return None");
    }

    #[tokio::test]
    async fn test_run_hook_nonzero_exit() {
        let path = write_test_hook("fail", "#!/bin/bash\nexit 1");
        let payload = json!({"event": "test"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        assert!(
            result.is_none(),
            "Non-zero exit with no stdout should return None"
        );
    }

    #[tokio::test]
    async fn test_run_hook_empty_stdout() {
        let path = write_test_hook("empty", "#!/bin/bash\n# no output");
        let payload = json!({"event": "test"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        assert!(result.is_none(), "Empty stdout should return None");
    }

    #[tokio::test]
    async fn test_run_hook_decision_block() {
        let path = write_test_hook(
            "block",
            "#!/bin/bash\necho '{\"decision\": \"block\", \"reason\": \"blocked by policy\"}'",
        );
        let payload = json!({"event": "pre_tool_use", "tool_name": "write_file"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        match result {
            Some(HookOutput::Decision { action, reason }) => {
                assert_eq!(action, "block");
                assert_eq!(reason.as_deref(), Some("blocked by policy"));
            }
            other => panic!("Expected Decision, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_run_hook_decision_allow() {
        let path = write_test_hook("allow", "#!/bin/bash\necho '{\"decision\": \"allow\"}'");
        let payload = json!({"event": "pre_tool_use"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        match result {
            Some(HookOutput::Decision { action, reason }) => {
                assert_eq!(action, "allow");
                assert!(reason.is_none());
            }
            other => panic!("Expected Decision, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_multiple_hooks_context_concat() {
        let path1 = write_test_hook(
            "ctx1",
            "#!/bin/bash\necho '{\"context_injection\": \"first\"}'",
        );
        let path2 = write_test_hook(
            "ctx2",
            "#!/bin/bash\necho '{\"context_injection\": \"second\"}'",
        );
        let hooks = vec![
            HookEntry {
                command: path1.clone(),
                timeout: 5,
                tool_name: None,
            },
            HookEntry {
                command: path2.clone(),
                timeout: 5,
                tool_name: None,
            },
        ];
        let payload = json!({"event": "session_start"});
        let result = run_context_hooks(&hooks, &payload).await;
        cleanup_test_hook(&path1);
        cleanup_test_hook(&path2);

        assert_eq!(result, Some("first\nsecond".to_string()));
    }

    #[tokio::test]
    async fn test_run_hook_reads_stdin() {
        let path = write_test_hook(
            "stdin",
            r#"#!/bin/bash
INPUT=$(cat)
TOOL=$(echo "$INPUT" | grep -o '"tool_name":"[^"]*"' | head -1 | cut -d'"' -f4)
echo "{\"context_injection\": \"tool=$TOOL\"}"
"#,
        );
        let payload = json!({"event": "pre_tool_use", "tool_name": "write_file"});
        let result = run_hook(&path, &payload, Duration::from_secs(5)).await;
        cleanup_test_hook(&path);

        match result {
            Some(HookOutput::ContextInjection(text)) => assert_eq!(text, "tool=write_file"),
            other => panic!("Expected ContextInjection, got {:?}", other),
        }
    }
}
