//! Windows: GetLocalTime, %APPDATA%, named mutexes, SetStdHandle, explorer.exe, waveOut.

use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SYSTEMTIME, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Media::Audio::{
    CALLBACK_NULL, HWAVEOUT, WAVE_FORMAT_PCM, WAVEFORMATEX, WAVEHDR, WAVEOUTCAPSW, WHDR_DONE, waveOutClose,
    waveOutGetDevCapsW, waveOutGetNumDevs, waveOutOpen, waveOutPrepareHeader, waveOutReset, waveOutUnprepareHeader,
    waveOutWrite,
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

/// Audio outputs: (waveOut id, name). Windows cuts the names to 31 characters, e.g.
/// "Headphones (CORSAIR HS80 RGB Wi".
pub fn audio_outputs() -> Vec<(u32, String)> {
    // SAFETY: no arguments.
    let n = unsafe { waveOutGetNumDevs() };
    (0..n)
        .filter_map(|id| {
            let mut caps = WAVEOUTCAPSW::default();
            // SAFETY: valid out-pointer of the size given.
            let r = unsafe { waveOutGetDevCapsW(id as usize, &mut caps, size_of::<WAVEOUTCAPSW>() as u32) };
            let name = caps.szPname;
            let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            (r == 0).then(|| (id, String::from_utf16_lossy(&name[..len])))
        })
        .collect()
}

/// Play 16-bit mono PCM on output `id`, and return once it has played.
pub fn play_pcm(id: u32, rate: u32, samples: &[i16]) -> Result<(), String> {
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut h: HWAVEOUT = std::ptr::null_mut();
    // SAFETY: valid out-pointer and format; no callback.
    let r = unsafe { waveOutOpen(&mut h, id, &format, 0, 0, CALLBACK_NULL) };
    if r != 0 {
        return Err(format!("cannot open audio output {id} (error {r})"));
    }
    let mut hdr = WAVEHDR {
        lpData: samples.as_ptr() as *mut u8,
        dwBufferLength: size_of_val(samples) as u32,
        ..Default::default()
    };
    let size = size_of::<WAVEHDR>() as u32;
    let ms = samples.len() as u64 * 1000 / rate.max(1) as u64;
    let deadline = Instant::now() + Duration::from_millis(ms + 2000);
    // SAFETY: `hdr` and `samples` outlive the playback: the device is reset (which ends it)
    // before the header is unprepared and the device closed.
    let r = unsafe {
        let mut r = waveOutPrepareHeader(h, &mut hdr, size);
        if r == 0 {
            r = waveOutWrite(h, &mut hdr, size);
            // the driver sets WHDR_DONE from its own thread (WAVEHDR is packed: no references)
            let flags = std::ptr::addr_of!(hdr.dwFlags);
            while r == 0 && flags.read_unaligned() & WHDR_DONE == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            waveOutReset(h);
            waveOutUnprepareHeader(h, &mut hdr, size);
        }
        waveOutClose(h);
        r
    };
    if r == 0 {
        Ok(())
    } else {
        Err(format!("cannot play on audio output {id} (error {r})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_lock_is_exclusive() {
        // the watcher and the app take "sounds-<pid>" so only one of them plays the tones
        let name = format!("test-sounds-{}", std::process::id());
        let a = instance_lock(&name).expect("first lock");
        assert!(instance_lock(&name).is_none(), "second holder");
        assert!(instance_running(&name));
        drop(a);
        assert!(!instance_running(&name));
        assert!(instance_lock(&name).is_some(), "free again once the holder is gone");
    }

    #[test]
    fn audio_output_names_are_whole_and_cut_to_31() {
        // a machine without audio (a CI runner) lists none, which is fine
        for (id, name) in audio_outputs() {
            assert!(!name.is_empty() && name.chars().count() <= 31, "output {id}: {name:?}");
            assert!(!name.contains('\0'));
        }
    }

    #[test]
    fn playing_on_a_missing_output_fails_quietly() {
        let missing = audio_outputs().len() as u32 + 100;
        let start = Instant::now();
        let r = play_pcm(missing, 48_000, &[0; 480]);
        assert!(r.is_err_and(|e| e.contains(&missing.to_string())));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
