//! Subprocess management utilities for harness adapters.
//!
//! Provides [`ChildProcessRunner`] for spawning and managing harness
//! subprocesses, and [`kill_tree`] for cleaning up process trees.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{BufReader, AsyncBufReadExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::HarnessError;

/// A spawned child process with piped I/O handles.
pub struct SpawnedChild {
    /// The child process handle.
    pub child: Child,
    /// Piped stdin for writing to the process.
    pub stdin: ChildStdin,
    /// Buffered reader over piped stdout.
    pub stdout: BufReader<ChildStdout>,
    /// Buffered reader over piped stderr.
    pub stderr: BufReader<ChildStderr>,
}

/// Builder/runner for harness child processes.
///
/// Configures environment, working directory, timeout, and arguments
/// for spawning harness subprocesses.
pub struct ChildProcessRunner {
    executable: PathBuf,
    args: Vec<String>,
    env: HashMap<String, String>,
    env_remove: Vec<String>,
    working_dir: Option<PathBuf>,
    timeout: Duration,
}

impl ChildProcessRunner {
    /// Create a new runner for the given executable.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            env: HashMap::new(),
            env_remove: Vec::new(),
            working_dir: None,
            timeout: Duration::from_secs(300),
        }
    }

    /// Set the operation timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Remove an environment variable from the child's inherited environment.
    #[must_use]
    pub fn without_env(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }

    /// Set the working directory for the child process.
    #[must_use]
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Add command-line arguments.
    #[must_use]
    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Run a one-shot command, wait for it to complete, and return stdout.
    ///
    /// Applies the configured timeout. Returns an error if the process
    /// fails to spawn, exits with a non-zero status, or times out.
    pub async fn run_one_shot(
        &self,
        extra_args: &[impl AsRef<OsStr>],
    ) -> Result<String, HarnessError> {
        let mut cmd = self.build_command();
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let child = cmd.spawn().map_err(|e| HarnessError::SpawnFailed {
            message: format!("{}: {e}", self.executable.display()),
        })?;

        let result = tokio::time::timeout(self.timeout, child.wait_with_output()).await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    String::from_utf8(output.stdout).map_err(|e| HarnessError::ParseError {
                        message: format!("non-UTF-8 stdout: {e}"),
                    })
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(HarnessError::SpawnFailed {
                        message: format!(
                            "{} exited with {}: {stderr}",
                            self.executable.display(),
                            output.status,
                        ),
                    })
                }
            }
            Ok(Err(e)) => Err(HarnessError::IoError {
                message: format!("waiting for process: {e}"),
            }),
            #[allow(clippy::cast_possible_truncation)]
            Err(_) => Err(HarnessError::Timeout {
                elapsed_ms: self.timeout.as_millis() as u64,
            }),
        }
    }

    /// Spawn a persistent child process with piped stdin/stdout/stderr.
    ///
    /// The caller is responsible for managing the child's lifecycle.
    pub fn spawn_persistent(
        &self,
        extra_args: &[impl AsRef<OsStr>],
    ) -> Result<SpawnedChild, HarnessError> {
        let mut cmd = self.build_command();
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| HarnessError::SpawnFailed {
            message: format!("{}: {e}", self.executable.display()),
        })?;

        let stdin = child.stdin.take().ok_or_else(|| HarnessError::Internal {
            message: "failed to capture child stdin".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| HarnessError::Internal {
            message: "failed to capture child stdout".to_string(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| HarnessError::Internal {
            message: "failed to capture child stderr".to_string(),
        })?;

        Ok(SpawnedChild {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr: BufReader::new(stderr),
        })
    }

    fn build_command(&self) -> Command {
        let mut cmd = Command::new(&self.executable);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for k in &self.env_remove {
            cmd.env_remove(k);
        }
        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }
        // Don't inherit the parent's stdin by default for safety.
        cmd.stdin(std::process::Stdio::null());
        cmd
    }
}

/// Send SIGKILL to a process and all its descendants.
///
/// Uses `kill(-pid, SIGKILL)` to send the signal to the entire process group.
/// Falls back to killing just the given PID if the process group kill fails.
#[allow(clippy::cast_possible_wrap)]
pub fn kill_tree(pid: u32) -> Result<(), HarnessError> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let nix_pid = Pid::from_raw(pid as i32);

    // Try to kill the process group first (negative PID).
    let group_result = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);

    if group_result.is_ok() {
        return Ok(());
    }

    // Fall back to killing just the process.
    kill(nix_pid, Signal::SIGKILL).map_err(|e| HarnessError::Internal {
        message: format!("failed to kill process {pid}: {e}"),
    })
}

/// Read lines from a `BufReader<ChildStdout>` or `BufReader<ChildStderr>`
/// until EOF, passing each line to the provided callback.
pub async fn drain_lines<R>(
    reader: &mut BufReader<R>,
    mut on_line: impl FnMut(String),
) -> Result<(), HarnessError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.map_err(|e| HarnessError::IoError {
            message: format!("reading subprocess output: {e}"),
        })?;
        if n == 0 {
            break;
        }
        // Trim the trailing newline before passing to callback.
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        on_line(trimmed.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_process_runner_builder() {
        let runner = ChildProcessRunner::new("/usr/bin/echo")
            .with_timeout(Duration::from_secs(10))
            .with_env("FOO", "bar")
            .without_env("HOME")
            .with_working_dir("/tmp")
            .with_args(["hello", "world"]);

        assert_eq!(runner.executable, PathBuf::from("/usr/bin/echo"));
        assert_eq!(runner.timeout, Duration::from_secs(10));
        assert_eq!(runner.env.get("FOO"), Some(&"bar".to_string()));
        assert!(runner.env_remove.contains(&"HOME".to_string()));
        assert_eq!(runner.working_dir, Some(PathBuf::from("/tmp")));
        assert_eq!(runner.args, vec!["hello".to_string(), "world".to_string()]);
    }

    #[tokio::test]
    async fn run_one_shot_echo() {
        let runner = ChildProcessRunner::new("/bin/echo")
            .with_timeout(Duration::from_secs(5));
        let output = runner.run_one_shot(&["hello"]).await.expect("echo should work");
        assert_eq!(output.trim(), "hello");
    }

    #[tokio::test]
    async fn run_one_shot_failure() {
        let runner = ChildProcessRunner::new("/bin/false")
            .with_timeout(Duration::from_secs(5));
        let empty: &[&str] = &[];
        let err = runner.run_one_shot(empty).await.unwrap_err();
        assert!(matches!(err, HarnessError::SpawnFailed { .. }));
    }

    #[tokio::test]
    async fn run_one_shot_not_found() {
        let runner = ChildProcessRunner::new("/nonexistent/binary")
            .with_timeout(Duration::from_secs(5));
        let empty: &[&str] = &[];
        let err = runner.run_one_shot(empty).await.unwrap_err();
        assert!(matches!(err, HarnessError::SpawnFailed { .. }));
    }

    #[tokio::test]
    async fn spawn_persistent_and_read() {
        let runner = ChildProcessRunner::new("/bin/echo")
            .with_timeout(Duration::from_secs(5));
        let mut spawned = runner
            .spawn_persistent(&["persistent-test"])
            .expect("spawn should work");

        let mut line = String::new();
        spawned
            .stdout
            .read_line(&mut line)
            .await
            .expect("read should work");
        assert_eq!(line.trim(), "persistent-test");

        let status = spawned.child.wait().await.expect("wait should work");
        assert!(status.success());
    }
}
