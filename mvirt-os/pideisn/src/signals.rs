use crate::log_debug;
use nix::sys::signal::{SigHandler, Signal, signal};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

pub fn setup_signal_handlers() {
    // SIGCHLD handler - we'll reap children in the main loop
    // Set to SIG_IGN to auto-reap or handle manually
    unsafe {
        // We handle SIGCHLD manually in reap_children()
        let _ = signal(Signal::SIGCHLD, SigHandler::Handler(sigchld_handler));
    }
}

extern "C" fn sigchld_handler(_: i32) {
    // Just wake up the main loop - actual reaping happens in reap_children()
}

pub fn reap_children() {
    loop {
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, status)) => {
                log_debug!("Child {} exited with status {}", pid, status);
            }
            Ok(WaitStatus::Signaled(pid, signal, _)) => {
                log_debug!("Child {} killed by signal {:?}", pid, signal);
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
