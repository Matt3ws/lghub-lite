//! peripheral-lite: talks HID++ directly to Logitech mice and headsets (Windows, macOS; Linux experimental).
//! Shared by the `peripheral-lite` CLI and the windowless `peripheral-lite-bg` watcher.
//! Not affiliated with or endorsed by Logitech. Protocol evidence: NOTES.md.

pub mod audio;
pub mod autostart;
pub mod battery;
pub mod centurion;
pub mod config;
pub mod corsair;
pub mod device;
pub mod hid;
pub mod hidpp;
pub mod migrate;
pub mod platform;
pub mod profiles;
pub mod sound;
pub mod watch;

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose(args: std::fmt::Arguments) {
    if VERBOSE.load(Ordering::Relaxed) {
        eprintln!("{args}");
    }
}

/// The project's name: settings folder, login entry, lock and mutex names.
pub const NAME: &str = "peripheral-lite";
/// The name up to v0.4.3; `migrate` moves what was kept under it.
pub const OLD_NAME: &str = "lghub-lite";

pub const LOGITECH_VID: u16 = 0x046D;
pub const CORSAIR_VID: u16 = 0x1B1C;
