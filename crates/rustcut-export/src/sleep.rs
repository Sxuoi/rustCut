//! EXP-14: block system sleep while a batch runs. Always released on drop.
//!
//! * Windows: a dedicated thread holds `SetThreadExecutionState(ES_CONTINUOUS |
//!   ES_SYSTEM_REQUIRED)` (the state is per thread) until the guard drops.
//! * macOS: `caffeinate -i -w <our pid>` as a child process.
//! * Linux: `systemd-inhibit` (if present) wrapping a `cat` whose stdin we hold.
//!
//! Failure to block sleep is never an error: [`SleepGuard::is_active`] reports it.

/// Keeps the system awake while alive.
#[derive(Debug)]
pub struct SleepGuard {
    inner: imp::Inner,
}

impl SleepGuard {
    /// Start blocking sleep (best effort).
    pub fn acquire() -> Self {
        SleepGuard {
            inner: imp::acquire(),
        }
    }

    /// Whether sleep is actually blocked.
    pub fn is_active(&self) -> bool {
        imp::is_active(&self.inner)
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{Sender, channel};
    use std::thread::JoinHandle;

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
    }

    #[derive(Debug)]
    pub struct Inner {
        release: Option<Sender<()>>,
        thread: Option<JoinHandle<()>>,
        active: bool,
    }

    pub fn acquire() -> Inner {
        let (tx, rx) = channel::<()>();
        let (ready_tx, ready_rx) = channel::<bool>();
        let thread = std::thread::Builder::new()
            .name("rustcut-sleep-block".into())
            .spawn(move || {
                // SAFETY: plain Win32 call without pointers; affects only this thread.
                let ok =
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) } != 0;
                let _ = ready_tx.send(ok);
                if ok {
                    let _ = rx.recv();
                    // SAFETY: as above; clears the requirement.
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                }
            });
        match thread {
            Ok(t) => {
                let active = ready_rx.recv().unwrap_or(false);
                Inner {
                    release: Some(tx),
                    thread: Some(t),
                    active,
                }
            }
            Err(_) => Inner {
                release: None,
                thread: None,
                active: false,
            },
        }
    }

    pub fn is_active(i: &Inner) -> bool {
        i.active
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            drop(self.release.take());
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::process::{Child, Command, Stdio};

    #[derive(Debug)]
    pub struct Inner(Option<Child>);

    pub fn acquire() -> Inner {
        let child = Command::new("caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        Inner(child)
    }

    pub fn is_active(i: &Inner) -> bool {
        i.0.is_some()
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            if let Some(mut c) = self.0.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::process::{Child, ChildStdin, Command, Stdio};

    #[derive(Debug)]
    pub struct Inner {
        child: Option<Child>,
        stdin: Option<ChildStdin>,
    }

    pub fn acquire() -> Inner {
        let spawned = Command::new("systemd-inhibit")
            .args([
                "--what=sleep",
                "--who=RustCut",
                "--why=Exporting clips",
                "--mode=block",
                "cat",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(mut c) => {
                let stdin = c.stdin.take();
                Inner {
                    child: Some(c),
                    stdin,
                }
            }
            Err(_) => Inner {
                child: None,
                stdin: None,
            },
        }
    }

    pub fn is_active(i: &Inner) -> bool {
        i.child.is_some()
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            // Closing stdin makes `cat` exit, which ends systemd-inhibit and the lock.
            drop(self.stdin.take());
            if let Some(mut c) = self.child.take() {
                let _ = c.wait();
            }
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    #[derive(Debug)]
    pub struct Inner;
    pub fn acquire() -> Inner {
        Inner
    }
    pub fn is_active(_: &Inner) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release_do_not_panic() {
        let g = SleepGuard::acquire();
        #[cfg(windows)]
        assert!(g.is_active());
        let _ = g.is_active();
        drop(g);
        let g2 = SleepGuard::acquire();
        drop(g2);
    }
}
