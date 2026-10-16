//! Windows: GetLocalTime, %APPDATA%, named mutexes, SetStdHandle, explorer.exe.

use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::null;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SYSTEMTIME, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};

pub fn local_time() -> [u16; 6] {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointer.
    unsafe { GetLocalTime(&mut t) };
    [t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond]
}

/// `%APPDATA%\lghub-lite`.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("lghub-lite"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn mutex_name(name: &str) -> Vec<u16> {
    wide(&format!("Local\\lghub-lite-{name}"))
}

/// Held for as long as this process runs the `name` instance ("bg", "gui").
pub struct InstanceLock(HANDLE);

// SAFETY: a mutex handle may be closed from any thread.
unsafe impl Send for InstanceLock {}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // SAFETY: our own handle, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Take the session-wide lock `name`; None if another process already holds it.
pub fn instance_lock(name: &str) -> Option<InstanceLock> {
    let n = mutex_name(name);
    // SAFETY: NUL-terminated name, default security.
    let h = unsafe { CreateMutexW(null(), 0, n.as_ptr()) };
    if h.is_null() {
        return None;
    }
    // SAFETY: thread-local error right after the call.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: our handle to the existing mutex.
        unsafe { CloseHandle(h) };
        return None;
    }
    Some(InstanceLock(h))
}

/// A lock that lghub-lite processes take in turns (a named mutex), e.g. around a conversation
/// with a device whose replies cannot be told apart.
pub struct SharedLock(HANDLE);

// SAFETY: a mutex handle may be used and closed from any thread; ownership is per thread, and
// `lock`/`unlock` are called in pairs on one thread.
unsafe impl Send for SharedLock {}

impl Drop for SharedLock {
    fn drop(&mut self) {
        // SAFETY: our own handle, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

pub fn shared_lock(name: &str) -> Option<SharedLock> {
    let n = mutex_name(name);
    // SAFETY: NUL-terminated name, default security; opens the mutex if it exists.
    let h = unsafe { CreateMutexW(null(), 0, n.as_ptr()) };
    (!h.is_null()).then_some(SharedLock(h))
}

impl SharedLock {
    /// Wait up to `ms` for the lock. A holder that exited without releasing it counts as released.
    pub fn lock(&self, ms: u32) -> bool {
        // SAFETY: valid mutex handle.
        let w = unsafe { WaitForSingleObject(self.0, ms) };
        w == WAIT_OBJECT_0 || w == WAIT_ABANDONED
    }

    pub fn unlock(&self) {
        // SAFETY: called only after a successful `lock` on this thread.
        unsafe { ReleaseMutex(self.0) };
    }
}

/// Is the `name` instance running in this session (without taking its lock)?
pub fn instance_running(name: &str) -> bool {
    let n = mutex_name(name);
    // SAFETY: NUL-terminated name; handle closed right away.
    let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, n.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { CloseHandle(h) };
    true
}

/// Send `println!`/`eprintln!` to `file` (std looks the handle up on every write). The file
/// must stay open for as long as output is written.
pub fn redirect_stdio(file: &File) {
    // SAFETY: valid file handle; the caller keeps the file alive.
    unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, file.as_raw_handle());
        SetStdHandle(STD_ERROR_HANDLE, file.as_raw_handle());
    }
}

/// Show `dir` in the file manager.
pub fn open_folder(dir: &Path) {
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}
