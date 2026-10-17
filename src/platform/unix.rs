//! Linux and macOS: localtime_r, the settings folder (XDG on Linux, Application Support on
//! macOS), flock-based instance locks, dup2, xdg-open / open.
//! [INF] The Linux side was written without Linux hardware; covered by unit tests and CI only.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub fn local_time() -> [u16; 6] {
    // SAFETY: time(NULL) and localtime_r with valid out-pointers.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        [
            (tm.tm_year + 1900) as u16,
            (tm.tm_mon + 1) as u16,
            tm.tm_mday as u16,
            tm.tm_hour as u16,
            tm.tm_min as u16,
            tm.tm_sec as u16,
        ]
    }
}

/// `~/Library/Application Support/lghub-lite`.
#[cfg(target_os = "macos")]
pub fn config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join("Library/Application Support/lghub-lite"))
}

/// `$XDG_CONFIG_HOME/lghub-lite`, else `~/.config/lghub-lite`.
#[cfg(not(target_os = "macos"))]
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lghub-lite"))
}

/// Where lock files live: `$XDG_RUNTIME_DIR` (per user, per session; on macOS `$TMPDIR`,
/// which is per user), else the config dir.
fn lock_path(name: &str) -> Option<PathBuf> {
    let runtime = if cfg!(target_os = "macos") {
        "TMPDIR"
    } else {
        "XDG_RUNTIME_DIR"
    };
    let dir = std::env::var_os(runtime).map(PathBuf::from).or_else(config_dir)?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join(format!("lghub-lite-{name}.lock")))
}

/// Held for as long as this process runs the `name` instance ("bg", "gui"): an exclusive
/// flock on a lock file, released by the kernel when the process exits.
pub struct InstanceLock(#[allow(dead_code)] File);

/// Take the lock `name`; None if another process already holds it.
pub fn instance_lock(name: &str) -> Option<InstanceLock> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(name)?)
        .ok()?;
    // SAFETY: valid fd.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return None;
    }
    Some(InstanceLock(f))
}

/// A lock that lghub-lite processes take in turns (flock on a lock file), e.g. around a
/// conversation with a device whose replies cannot be told apart.
pub struct SharedLock(File);

pub fn shared_lock(name: &str) -> Option<SharedLock> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(name)?)
        .ok()?;
    Some(SharedLock(f))
}

impl SharedLock {
    /// Wait up to `ms` for the lock.
    pub fn lock(&self, ms: u32) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms as u64);
        loop {
            // SAFETY: valid fd.
            if unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn unlock(&self) {
        // SAFETY: valid fd.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Is the `name` instance running (without keeping its lock)?
pub fn instance_running(name: &str) -> bool {
    let Some(p) = lock_path(name) else { return false };
    let Ok(f) = OpenOptions::new().read(true).open(p) else {
        return false;
    };
    // SAFETY: valid fd; a lock we get is released again right away.
    unsafe {
        if libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 {
            libc::flock(f.as_raw_fd(), libc::LOCK_UN);
            false
        } else {
            true
        }
    }
}

/// Send stdout and stderr to `file`.
pub fn redirect_stdio(file: &File) {
    // SAFETY: valid fds; dup2 replaces fds 1 and 2.
    unsafe {
        libc::dup2(file.as_raw_fd(), 1);
        libc::dup2(file.as_raw_fd(), 2);
    }
}

/// Show `dir` in the file manager.
pub fn open_folder(dir: &Path) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(dir).spawn();
}

/// Audio outputs: none yet, the headset tones are Windows-only so far.
pub fn audio_outputs() -> Vec<(u32, String)> {
    Vec::new()
}

pub fn play_pcm(_id: u32, _rate: u32, _samples: &[i16]) -> Result<(), String> {
    Err("playing sounds is Windows-only so far".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_lock_is_exclusive() {
        let name = format!("test-{}", std::process::id());
        let a = instance_lock(&name).expect("first lock");
        assert!(instance_running(&name));
        drop(a);
        assert!(!instance_running(&name));
        if let Some(p) = lock_path(&name) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn local_time_is_plausible() {
        let [y, mo, d, h, mi, s] = local_time();
        assert!(y >= 2024 && (1..=12).contains(&mo) && (1..=31).contains(&d) && h < 24 && mi < 60 && s < 61);
    }
}
