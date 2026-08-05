//! Real-PTY evidence for Crossterm TUI setup and restoration.
//!
//! The parent test spawns ignored helper tests from this same integration-test
//! binary with all three standard streams attached to a Unix pseudoterminal.
//! This exercises the real Crossterm backend while keeping fault injection out
//! of production code.

#![cfg(all(unix, not(target_os = "aix")))]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::pty::{openpty, Winsize};
use nix::sys::termios::{tcgetattr, LocalFlags, Termios};

use polkagent_cli::tui::app::{enter_tui, exit_tui};

const CHILD_ENV: &str = "POLKAGENT_INTERNAL_TUI_PTY_CHILD";
const TIMEOUT: Duration = Duration::from_secs(10);
const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049l";
const ENABLE_MOUSE_CAPTURE: &[u8] = b"\x1b[?1000h";
const DISABLE_MOUSE_CAPTURE: &[u8] = b"\x1b[?1000l";
const ENABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004h";
const DISABLE_BRACKETED_PASTE: &[u8] = b"\x1b[?2004l";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

struct PtyResult {
    status: ExitStatus,
    output: Vec<u8>,
    initial_termios: Termios,
    final_termios: Termios,
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn byte_position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn wait_with_timeout(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll PTY child") {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out PTY child");
            let _status = child.wait().expect("reap timed-out PTY child");
            panic!("PTY child did not exit within {TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_available(master: &mut File) -> Vec<u8> {
    fcntl(master.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
        .expect("make PTY master nonblocking");
    let mut output = Vec::new();
    loop {
        let mut chunk = [0_u8; 4096];
        match master.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("read PTY output: {error}"),
        }
    }
    output
}

fn run_pty_child(test_name: &str) -> PtyResult {
    let winsize = Winsize {
        ws_row: 24,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(Some(&winsize), None).expect("open PTY");
    let mut master = File::from(pty.master);
    let slave = File::from(pty.slave);
    let initial_termios = tcgetattr(&slave).expect("read initial PTY termios");

    let stdin = slave.try_clone().expect("clone PTY slave for stdin");
    let stdout = slave.try_clone().expect("clone PTY slave for stdout");
    let stderr = slave.try_clone().expect("clone PTY slave for stderr");
    let mut child = Command::new(std::env::current_exe().expect("integration-test executable"))
        .args(["--ignored", "--exact", test_name, "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("RUST_BACKTRACE", "0")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn PTY child");

    let status = wait_with_timeout(&mut child);
    let final_termios = tcgetattr(&slave).expect("read restored PTY termios");
    let output = read_available(&mut master);
    PtyResult {
        status,
        output,
        initial_termios,
        final_termios,
    }
}

fn assert_restored(case: &str, result: &PtyResult) {
    let escaped = String::from_utf8_lossy(&result.output);
    assert!(result.status.success(), "{case} child failed: {escaped}");
    assert!(
        contains_bytes(&result.output, ENTER_ALTERNATE_SCREEN),
        "{case} did not enter the alternate screen: {escaped:?}"
    );
    assert!(
        contains_bytes(&result.output, LEAVE_ALTERNATE_SCREEN),
        "{case} did not leave the alternate screen: {escaped:?}"
    );
    assert!(
        contains_bytes(&result.output, ENABLE_MOUSE_CAPTURE),
        "{case} did not enable mouse capture: {escaped:?}"
    );
    assert!(
        contains_bytes(&result.output, DISABLE_MOUSE_CAPTURE),
        "{case} did not disable mouse capture: {escaped:?}"
    );
    assert!(
        contains_bytes(&result.output, ENABLE_BRACKETED_PASTE),
        "{case} did not enable bracketed paste: {escaped:?}"
    );
    assert!(
        contains_bytes(&result.output, DISABLE_BRACKETED_PASTE),
        "{case} did not disable bracketed paste: {escaped:?}"
    );
    assert!(
        contains_bytes(&result.output, SHOW_CURSOR),
        "{case} did not show the cursor: {escaped:?}"
    );
    let enter_position = byte_position(&result.output, ENTER_ALTERNATE_SCREEN)
        .expect("alternate-screen entry already asserted");
    let leave_position = byte_position(&result.output, LEAVE_ALTERNATE_SCREEN)
        .expect("alternate-screen exit already asserted");
    let mouse_enable_position =
        byte_position(&result.output, ENABLE_MOUSE_CAPTURE).expect("mouse enable already asserted");
    let mouse_disable_position = byte_position(&result.output, DISABLE_MOUSE_CAPTURE)
        .expect("mouse disable already asserted");
    let paste_enable_position = byte_position(&result.output, ENABLE_BRACKETED_PASTE)
        .expect("bracketed-paste enable already asserted");
    let paste_disable_position = byte_position(&result.output, DISABLE_BRACKETED_PASTE)
        .expect("bracketed-paste disable already asserted");
    let show_cursor_position =
        byte_position(&result.output, SHOW_CURSOR).expect("cursor show already asserted");
    assert!(
        enter_position < leave_position,
        "{case} left the alternate screen before entering it: {escaped:?}"
    );
    assert!(
        mouse_enable_position < mouse_disable_position,
        "{case} disabled mouse capture before enabling it: {escaped:?}"
    );
    assert!(
        paste_enable_position < paste_disable_position,
        "{case} disabled bracketed paste before enabling it: {escaped:?}"
    );
    assert!(
        leave_position < mouse_disable_position
            && mouse_disable_position < paste_disable_position
            && paste_disable_position < show_cursor_position,
        "{case} emitted restoration escapes out of order: {escaped:?}"
    );
    assert_termios_restored(case, &result.initial_termios, &result.final_termios);
}

fn assert_termios_restored(case: &str, initial: &Termios, final_state: &Termios) {
    assert_eq!(
        final_state.input_flags, initial.input_flags,
        "{case}: input flags"
    );
    assert_eq!(
        final_state.output_flags, initial.output_flags,
        "{case}: output flags"
    );
    assert_eq!(
        final_state.control_flags, initial.control_flags,
        "{case}: control flags"
    );
    // Darwin may set PENDIN after tcsetattr to request a kernel-side reprint
    // of pending input. It is transient and does not describe raw/canonical,
    // echo, signal, or extension behavior, so compare all other local flags.
    assert_eq!(
        final_state.local_flags - LocalFlags::PENDIN,
        initial.local_flags - LocalFlags::PENDIN,
        "{case}: local flags"
    );
    assert_eq!(
        final_state.control_chars, initial.control_chars,
        "{case}: control characters"
    );
}

#[test]
fn real_pty_restores_terminal_on_explicit_exit_error_and_panic() {
    for (case, child_test) in [
        ("explicit exit", "pty_child_explicit_exit"),
        ("ordinary error", "pty_child_error_drop"),
        ("panic unwind", "pty_child_panic_drop"),
    ] {
        assert_restored(case, &run_pty_child(child_test));
    }
}

fn running_as_pty_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

#[test]
#[ignore = "spawned by the real-PTY parent test"]
fn pty_child_explicit_exit() {
    if !running_as_pty_child() {
        return;
    }
    let mut terminal = enter_tui().expect("enter TUI");
    terminal
        .backend_mut()
        .write_all(b"normal path")
        .expect("write through backend");
    exit_tui(&mut terminal).expect("explicit TUI restoration");
}

fn fail_after_entering_tui() -> anyhow::Result<()> {
    let mut terminal = enter_tui()?;
    terminal.backend_mut().write_all(b"error path")?;
    anyhow::bail!("injected event-loop error")
}

#[test]
#[ignore = "spawned by the real-PTY parent test"]
fn pty_child_error_drop() {
    if !running_as_pty_child() {
        return;
    }
    let error = fail_after_entering_tui().expect_err("injected error should propagate");
    assert!(error.to_string().contains("injected event-loop error"));
}

#[test]
#[ignore = "spawned by the real-PTY parent test"]
fn pty_child_panic_drop() {
    if !running_as_pty_child() {
        return;
    }
    let panic = std::panic::catch_unwind(|| {
        let mut terminal = enter_tui().expect("enter TUI");
        terminal
            .backend_mut()
            .write_all(b"panic path")
            .expect("write through backend");
        panic!("injected TUI panic");
    });
    assert!(panic.is_err(), "injected panic should unwind");
}
