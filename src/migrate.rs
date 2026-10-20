//! First start under the new name (peripheral-lite; lghub-lite up to v0.4.3): take over the
//! settings folder, the login entry and any old copy still running. The CLI, the background
//! watcher and the app call `run` before they touch the settings folder; once nothing is left
//! under the old name it only checks and returns.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{NAME, OLD_NAME, autostart, platform};

/// Move what lghub-lite left behind. `me` is the caller's instance name ("cli", "bg", "gui"):
/// only the app stops an old app, and the watcher does not start another watcher. Returns what
/// was done, one line each.
pub fn run(me: &str) -> Vec<String> {
    let (Some(new_dir), Some(old_dir)) = (platform::config_dir(), platform::config_dir_of(OLD_NAME)) else {
        return Vec::new();
    };
    if !pending(me, &old_dir, &new_dir) {
        return Vec::new();
    }
    // one process at a time: the app may start the new watcher, which runs this too
    let turn = platform::shared_lock("migrate");
    let locked = turn.as_ref().is_some_and(|t| t.lock(15_000));
    let done = if pending(me, &old_dir, &new_dir) {
        migrate(me, &old_dir, &new_dir)
    } else {
        Vec::new()
    };
    if locked && let Some(t) = &turn {
        t.unlock();
    }
    done
}

/// The old copies `me` stops: the watcher always, the app only when the new app starts.
fn old_copies(me: &str) -> &'static [&'static str] {
    if me == "gui" { &["bg", "gui"] } else { &["bg"] }
}

fn pending(me: &str, old_dir: &Path, new_dir: &Path) -> bool {
    (old_dir.is_dir() && only_locks(new_dir))
        || old_copies(me)
            .iter()
            .any(|n| platform::instance_running_of(OLD_NAME, n))
        || autostart::status_of(autostart::OLD_ID).ok().flatten().is_some()
}

fn migrate(me: &str, old_dir: &Path, new_dir: &Path) -> Vec<String> {
    let mut done = Vec::new();

    // Old copies first: they would keep talking to the devices, and their open files could
    // keep the folder from moving.
    let mut old_bg = false;
    for &name in old_copies(me) {
        if !platform::instance_running_of(OLD_NAME, name) {
            continue;
        }
        let exe = platform::exe(&format!("{OLD_NAME}-{name}"));
        match autostart::stop_running(&exe) {
            Ok(_) => {
                let until = Instant::now() + Duration::from_secs(3);
                while platform::instance_running_of(OLD_NAME, name) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(100));
                }
                done.push(format!("stopped the old {exe}"));
            }
            Err(e) => done.push(format!("could not stop the old {exe}: {e}")),
        }
        old_bg |= name == "bg";
    }

    if old_dir.is_dir() && only_locks(new_dir) {
        match move_settings(old_dir, new_dir) {
            Ok(()) => done.push(format!(
                "moved settings from {} to {}",
                old_dir.display(),
                new_dir.display()
            )),
            Err(e) => done.push(format!(
                "could not move all settings from {} to {}: {e}",
                old_dir.display(),
                new_dir.display()
            )),
        }
    }

    // The login entry: the same program and arguments under the new name, from this folder.
    let mut watcher = None;
    match autostart::status_of(autostart::OLD_ID) {
        Ok(Some(old)) => match rewrite(&old, old_dir, new_dir) {
            Some((file, args)) => match find_exe(&file) {
                Some(exe) => {
                    let cmd = format!("\"{}\"{args}", exe.display());
                    let r = match autostart::status() {
                        Ok(Some(_)) => Ok(()), // already set up under the new name
                        _ => autostart::enable(&cmd),
                    };
                    match r.and_then(|_| autostart::disable_of(autostart::OLD_ID)) {
                        Ok(()) => done.push(format!("start at login now runs {cmd}")),
                        Err(e) => done.push(format!("could not move the start-at-login entry: {e}")),
                    }
                    if file.contains("-bg") {
                        watcher = Some(cmd);
                    }
                }
                None => crate::verbose(format_args!("old login entry kept: {file} is not next to this program")),
            },
            None => crate::verbose(format_args!("old login entry kept: {old}")),
        },
        Ok(None) => {}
        Err(e) => crate::verbose(format_args!("old login entry: {e}")),
    }

    // A watcher that was running keeps running, as the new one.
    if old_bg && me != "bg" && !platform::instance_running("bg") {
        let cmd =
            watcher.or_else(|| find_exe(&platform::exe(&format!("{NAME}-bg"))).map(|p| format!("\"{}\"", p.display())));
        if let Some(cmd) = cmd {
            match autostart::start(&cmd) {
                Ok(()) => {
                    // let the caller see it running (the app checks right after this)
                    let until = Instant::now() + Duration::from_secs(3);
                    while !platform::instance_running("bg") && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    done.push(format!("started {cmd}"));
                }
                Err(e) => done.push(format!("could not start {cmd}: {e}")),
            }
        }
    }
    done
}

/// Missing, or holding only lock files (made before the move, where there is no runtime folder).
fn only_locks(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .all(|e| e.path().extension().is_some_and(|x| x == "lock")),
        Err(_) => true,
    }
}

/// Move each entry of `old` into `new` (made if missing), then remove `old`. An entry that cannot
/// be moved (a file still open elsewhere) is copied and left in place; one already in `new` is
/// kept as it is.
fn move_settings(old: &Path, new: &Path) -> io::Result<()> {
    std::fs::create_dir_all(new)?;
    let mut first_err = None;
    for e in std::fs::read_dir(old)? {
        let e = e?;
        let to = new.join(e.file_name());
        if to.exists() {
            continue;
        }
        if std::fs::rename(e.path(), &to).is_err()
            && let Err(err) = copy_tree(&e.path(), &to)
        {
            first_err.get_or_insert(io::Error::new(err.kind(), format!("{}: {err}", e.path().display())));
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    let _ = std::fs::remove_dir(old); // only when empty: a copied file is still there
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        copy_tree(&e.path(), &to.join(e.file_name()))?;
    }
    Ok(())
}

/// An old login command's program under its new name, and its arguments with the settings
/// folder renamed: `"C:\x\lghub-lite-bg.exe" --config "<old>\presets.txt"` gives
/// `peripheral-lite-bg.exe` and ` --config "<new>\presets.txt"`. None for anything else.
fn rewrite(cmd: &str, old_dir: &Path, new_dir: &Path) -> Option<(String, String)> {
    let (exe, args) = cmd.strip_prefix('"')?.split_once('"')?;
    let file = exe.rsplit(['/', '\\']).next()?;
    let kind = file.strip_prefix(OLD_NAME)?;
    if !kind.starts_with("-bg") && !kind.starts_with("-gui") {
        return None;
    }
    let args = args.replace(&old_dir.display().to_string(), &new_dir.display().to_string());
    Some((format!("{NAME}{kind}"), args))
}

/// `file` next to this program; on macOS also inside or around the app bundle (the app sits in
/// peripheral-lite.app/Contents/MacOS, the CLI and the watcher next to peripheral-lite.app).
fn find_exe(file: &str) -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let mut places = vec![dir.to_path_buf()];
    if cfg!(target_os = "macos") {
        places.push(dir.join(format!("{NAME}.app/Contents/MacOS")));
        places.extend(dir.ancestors().nth(3).map(Path::to_path_buf));
    }
    places.into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{NAME}-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn login_commands_get_the_new_names() {
        let (old, new) = (
            Path::new("/home/a/.config/lghub-lite"),
            Path::new("/home/a/.config/peripheral-lite"),
        );
        assert_eq!(
            rewrite(r#""C:\Tools\lghub-lite\lghub-lite-bg.exe""#, old, new),
            Some(("peripheral-lite-bg.exe".into(), String::new()))
        );
        assert_eq!(
            rewrite(r#""/opt/x y/lghub-lite-gui" --tray"#, old, new),
            Some(("peripheral-lite-gui".into(), " --tray".into()))
        );
        let cmd = format!(r#""/opt/lghub-lite-bg" --config "{}/presets.txt""#, old.display());
        assert_eq!(
            rewrite(&cmd, old, new),
            Some((
                "peripheral-lite-bg".into(),
                format!(r#" --config "{}/presets.txt""#, new.display())
            ))
        );
        // a preset file the user keeps elsewhere stays where it is
        assert_eq!(
            rewrite(r#""/opt/lghub-lite-bg" --config "/srv/p.txt""#, old, new).map(|r| r.1),
            Some(r#" --config "/srv/p.txt""#.into())
        );
        assert_eq!(rewrite(r#""/opt/other-bg""#, old, new), None);
        assert_eq!(rewrite(r#""/opt/lghub-lite.exe" watch"#, old, new), None);
        assert_eq!(rewrite("/opt/lghub-lite-bg", old, new), None);
    }

    #[test]
    fn settings_move_to_the_new_folder() {
        let (old, new) = (temp("old"), temp("new"));
        std::fs::create_dir_all(old.join("skins")).unwrap();
        std::fs::write(old.join("presets.txt"), "40a9 dpi=1600\n").unwrap();
        std::fs::write(old.join("skins/blue.json"), "{}").unwrap();
        assert!(only_locks(&new));
        move_settings(&old, &new).unwrap();
        assert!(!old.exists());
        assert_eq!(
            std::fs::read_to_string(new.join("presets.txt")).unwrap(),
            "40a9 dpi=1600\n"
        );
        assert_eq!(std::fs::read_to_string(new.join("skins/blue.json")).unwrap(), "{}");
        assert!(!only_locks(&new));
        std::fs::remove_dir_all(&new).unwrap();
    }

    #[test]
    fn settings_move_into_a_folder_holding_only_locks() {
        let (old, new) = (temp("old2"), temp("new2"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("app.txt"), "skin dark\n").unwrap();
        std::fs::write(new.join("peripheral-lite-migrate.lock"), "").unwrap();
        assert!(only_locks(&new));
        move_settings(&old, &new).unwrap();
        assert_eq!(std::fs::read_to_string(new.join("app.txt")).unwrap(), "skin dark\n");
        assert!(!old.exists());
        std::fs::remove_dir_all(&new).unwrap();
    }

    #[test]
    fn copy_tree_copies_nested_files() {
        let (from, to) = (temp("from"), temp("to"));
        std::fs::create_dir_all(from.join("a/b")).unwrap();
        std::fs::write(from.join("a/b/c.csv"), "1,2\n").unwrap();
        copy_tree(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("a/b/c.csv")).unwrap(), "1,2\n");
        assert!(from.join("a/b/c.csv").exists());
        std::fs::remove_dir_all(&from).unwrap();
        std::fs::remove_dir_all(&to).unwrap();
    }
}
