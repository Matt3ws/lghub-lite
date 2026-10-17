//! Login autostart via an XDG autostart entry, `~/.config/autostart/peripheral-lite.desktop`
//! (honoured by GNOME, KDE, XFCE, Cinnamon, MATE, LXQt). Only touched when the user runs
//! `peripheral-lite autostart on|off` or ticks Tray in the app.
//! [INF] Written without Linux hardware; covered by unit tests and CI only.

use std::io;
use std::path::PathBuf;

/// The entry's file name (without `.desktop`) up to v0.4.3.
pub const OLD_ID: &str = crate::OLD_NAME;

fn entry_path() -> io::Result<PathBuf> {
    entry_path_of(crate::NAME)
}

fn entry_path_of(id: &str) -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| io::Error::other("neither XDG_CONFIG_HOME nor HOME is set"))?;
    Ok(base.join("autostart").join(format!("{id}.desktop")))
}

/// The .desktop file for `command` (a command line with double-quoted paths).
fn entry(command: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=peripheral-lite\nComment=Logitech HID++ battery and settings (peripheral-lite)\nExec={command}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    )
}

pub fn enable(command: &str) -> io::Result<()> {
    let p = entry_path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::config::write_atomic(&p, &entry(command)).map_err(io::Error::other)
}

pub fn disable() -> io::Result<()> {
    disable_of(crate::NAME)
}

/// Remove the entry `id`; succeeds if it was not there.
pub fn disable_of(id: &str) -> io::Result<()> {
    match std::fs::remove_file(entry_path_of(id)?) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The registered command line, if any.
pub fn status() -> io::Result<Option<String>> {
    status_of(crate::NAME)
}

/// The command line in entry `id`, if any.
pub fn status_of(id: &str) -> io::Result<Option<String>> {
    match std::fs::read_to_string(entry_path_of(id)?) {
        Ok(text) => Ok(text.lines().find_map(|l| l.strip_prefix("Exec=")).map(str::to_string)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub use super::unix::start;

/// Stop every running process of this user named `exe_name` (SIGTERM). Returns how many.
pub fn stop_running(exe_name: &str) -> io::Result<u32> {
    let me = std::process::id();
    // SAFETY: plain getter.
    let uid = unsafe { libc::getuid() };
    let mut n = 0;
    for e in std::fs::read_dir("/proc")?.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid == me {
            continue;
        }
        // comm is truncated to 15 characters, so compare against the exe's file name
        let exe = std::fs::read_link(e.path().join("exe")).ok();
        let name = exe.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        if name != Some(exe_name) {
            continue;
        }
        let owner = std::fs::metadata(e.path())
            .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
            .ok();
        if owner != Some(uid) {
            continue;
        }
        // SAFETY: signalling a process of our own user.
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0 {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry() {
        let e = entry(r#""/x/peripheral-lite-gui" --tray"#);
        assert!(e.contains("\nExec=\"/x/peripheral-lite-gui\" --tray\n"));
    }
}
