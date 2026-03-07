use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::types::HookCommandOutput;

/// Run a hook command as a direct subprocess.
///
/// Deadlock-safe: stdout and stderr are drained concurrently via spawned tasks,
/// and both drains start BEFORE stdin is written. This prevents circular deadlock
/// when the child echoes input back to stdout/stderr before consuming all stdin.
///
/// The child is placed in its own process group (unix) so terminal SIGINT does not
/// kill it — the cancellation token is the intended shutdown path.
pub async fn run_hook_command(
    command_line: &str,
    stdin_data: Option<&str>,
    timeout_secs: u64,
    working_dir: &Path,
    cancel_token: CancellationToken,
) -> Result<HookCommandOutput, String> {
    let timeout = if timeout_secs == 0 { 600 } else { timeout_secs };

    let mut command = build_shell_command(command_line);
    command.current_dir(working_dir);

    #[cfg(unix)]
    command.process_group(0);

    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to spawn hook command: {}", e))?;

    let stdin_handle = child.stdin.take();
    let stdout_handle = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture stdout".to_string())?;
    let stderr_handle = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture stderr".to_string())?;

    // Spawn stdout drain FIRST (before stdin write to prevent circular deadlock)
    let stdout_task = tokio::spawn(async move {
        let mut output = String::new();
        let mut reader = stdout_handle;
        let _ = reader.read_to_string(&mut output).await;
        output
    });

    let stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        let mut reader = stderr_handle;
        let _ = reader.read_to_string(&mut output).await;
        output
    });

    // Write stdin concurrently with drains
    let stdin_data_owned = stdin_data.map(|s| s.to_string());
    let stdin_task = tokio::spawn(async move {
        if let Some(data) = stdin_data_owned {
            if let Some(mut stdin) = stdin_handle {
                let _ =
                    tokio::time::timeout(Duration::from_secs(30), stdin.write_all(data.as_bytes()))
                        .await;
                drop(stdin);
            }
        }
    });

    // Wait for child with timeout + cancellation
    let (exit_code, timed_out) = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(timeout), child.wait()) => {
            match result {
                Ok(Ok(status)) => (status.code(), false),
                Ok(Err(e)) => {
                    return Err(format!("Failed waiting on hook command: {}", e));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    (None, true)
                }
            }
        }
        _ = cancel_token.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            (None, true)
        }
    };

    // Collect drain output with secondary timeout (prevents grandchild pipe hold)
    let drain_timeout = Duration::from_secs(5);
    let stdout_output = tokio::time::timeout(drain_timeout, stdout_task)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let stderr_output = tokio::time::timeout(drain_timeout, stderr_task)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    let _ = tokio::time::timeout(Duration::from_secs(1), stdin_task).await;

    Ok(HookCommandOutput {
        stdout: stdout_output,
        stderr: stderr_output,
        exit_code,
        timed_out,
    })
}

/// Build a platform-appropriate shell command.
fn build_shell_command(command_line: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/C", command_line]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("/bin/bash");
        cmd.args(["-c", command_line]);
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[cfg(not(windows))]
    #[tokio::test]
    async fn hook_receives_stdin_and_returns_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_hook_command(
            r#"cat"#,
            Some(r#"{"hook_event_name":"PreToolUse","session_id":"s1"}"#),
            10,
            dir.path(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.contains("PreToolUse"));
        assert!(!output.timed_out);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn hook_exit_2_is_captured() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_hook_command(
            "echo 'blocked' >&2; exit 2",
            None,
            10,
            dir.path(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(output.exit_code, Some(2));
        assert!(output.stderr.contains("blocked"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn hook_timeout_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_hook_command(
            "sleep 60",
            None,
            1,
            dir.path(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(output.timed_out);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn hook_cancellation_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_clone.cancel();
        });

        let output = run_hook_command(
            "sleep 60",
            None,
            600,
            dir.path(),
            cancel,
        )
        .await
        .unwrap();

        assert!(output.timed_out);
    }
}
