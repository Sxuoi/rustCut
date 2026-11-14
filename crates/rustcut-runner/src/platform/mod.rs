//! Platform-specific process plumbing. Each platform module provides:
//!
//! * `configure(&mut Command)` — creation flags / pre-exec hooks before spawning;
//! * `adopt_child(&Child) -> io::Result<()>` — work after spawning (Windows: join the job and
//!   resume the suspended child);
//! * `is_running(pid) -> bool` and `kill_if(pid, is_sidecar) -> io::Result<StaleProcess>`.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub(crate) use win::*;
