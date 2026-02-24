/// Lifecycle hook system for Goose — E2 spike (SessionStart only).
///
/// Reads hook commands from GOOSE_SESSION_START_HOOK environment variable.
/// Each hook receives a JSON payload on stdin and can return JSON or plain text on stdout.
///
/// This spike validates: (a) hooks can be spawned from the session lifecycle,
/// (b) stdout can be captured, (c) captured text can reach the system prompt
/// via Agent::extend_system_prompt().
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Run all configured SessionStart hooks and collect their context injections.
/// Returns concatenated context injection text, or None if no hooks are configured.
pub fn run_session_start_hooks(session_id: &str) -> Option<String> {
    // Spike: read from env var (full config system comes in the real contribution)
    let hook_cmd = std::env::var("GOOSE_SESSION_START_HOOK").ok()?;
    if hook_cmd.trim().is_empty() {
        return None;
    }

    let payload = json!({
        "event": "session_start",
        "session_id": session_id,
    });

    run_hook(&hook_cmd, &payload, Duration::from_secs(10))
}

/// Execute a single hook command: send JSON payload on stdin, capture stdout.
/// Returns the context_injection field if the output is valid JSON, or the raw
/// stdout if it is plain text. Returns None on timeout or process error.
fn run_hook(cmd: &str, payload: &Value, timeout: Duration) -> Option<String> {
    // Parse command into program + args (simple shell split — full impl uses shlex)
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let (program, args) = parts.split_first()?;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Write payload to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
        // stdin dropped here → EOF sent to child
    }

    // Wait with timeout (std::process has no built-in timeout; use a thread)
    let output = wait_with_timeout(child, timeout)?;

    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return None;
    }

    // If JSON, extract context_injection; otherwise use raw stdout
    if let Ok(parsed) = serde_json::from_str::<Value>(&stdout) {
        let injection = parsed.get("context_injection")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| stdout.clone());
        Some(injection)
    } else {
        Some(stdout)
    }
}

/// Block until the child exits or the timeout elapses.
/// On timeout, kills the child and returns None.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::process::Output> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().ok();
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}
