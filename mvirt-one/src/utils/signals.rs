//! Signal handling utilities for uos.
//! Ported from pideisn.

use log::debug;
use nix::sys::signal::{SigHandler, Signal, signal};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

/// Setup signal handlers for PID 1 operation.
pub fn setup_signal_handlers() {
    unsafe {
        // We handle SIGCHLD manually in reap_children()
        let _ = signal(Signal::SIGCHLD, SigHandler::Handler(sigchld_handler));
    }
}

extern "C" fn sigchld_handler(_: i32) {
    // Just wake up the main loop - actual reaping happens in reap_children()
}

/// Reap zombie children.
/// Call this periodically in the main loop.
pub fn reap_children() {
    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, status)) => {
                debug!("Child {} exited with status {}", pid, status);
            }
            Ok(WaitStatus::Signaled(pid, signal, _)) => {
                debug!("Child {} killed by signal {:?}", pid, signal);
            }
            Ok(WaitStatus::StillAlive) => {
                // No more children to reap
                break;
            }
            Ok(_) => {
                // Other status (stopped, continued) - ignore
            }
            Err(nix::errno::Errno::ECHILD) => {
                // No children
                break;
            }
            Err(_) => {
                break;
            }
        }
    }
}

/// Information about a child process exit.
#[derive(Debug, Clone)]
pub struct ChildExit {
    pub pid: i32,
    pub exit_code: i32,
}

/// Wait for a specific child process to exit.
/// This is a blocking call and should be spawned in a separate task.
pub fn wait_for_child(pid: i32) -> ChildExit {
    let pid_obj = Pid::from_raw(pid);
    let status = waitpid(pid_obj, None);

    let exit_code = match status {
        Ok(WaitStatus::Exited(_, code)) => code,
        Ok(WaitStatus::Signaled(_, sig, _)) => 128 + (sig as i32),
        _ => 255,
    };

    ChildExit { pid, exit_code }
}
