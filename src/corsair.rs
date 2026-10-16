//! Corsair "Bragi" protocol, used by Corsair's wireless headsets and their Slipstream dongles
//! (here the HS80 RGB Wireless: dongle 1b1c:0a73, headset 1b1c:0a71). Each device has numbered
//! properties that are read and written one at a time.
//!
//! Frames are 64 bytes on vendor page 0xFF42, usage 1, of interface 3:
//! - request (report ID 0x02): `[0x02, target, command, property (u16 LE), value LE…]`, where
//!   target 0x08 is the dongle and 0x09 the headset behind it; command 0x02 = get, 0x01 = set.
//! - reply (report ID 0x01): `[0x01, target - 8, command, status, value LE (4 bytes)…]`;
//!   status 0 = ok, 5 = property not supported, 9 = not allowed now. A get reply carries the
//!   value; a set reply's value bytes are stale and meaningless.
//!
//! [HW] The headset refuses sets (status 9) in hardware mode (property 3 = 1, its normal state)
//! and accepts them in software mode (3 = 2), which iCUE uses. A value set there stays after
//! switching back to hardware mode, so `Corsair::set` wraps a refused set in software mode and
//! always returns to hardware mode; the headset keeps running on its own afterwards.
//!
//! Replies do not echo the property, so requests go strictly one at a time and reports already
//! waiting are drained before each request.
//!
//! Property changes are pushed as events on the interface's page 0xFF42 usage 2 collection
//! (input only, report ID 0x03): `[0x03, target - 8, 0x01, property (u16 LE), value LE…]`.
//! [HW] seen for battery state (charger in/out) and mic mute. Turning the headset off sends
//! nothing: requests to it just go unanswered while the dongle keeps answering.
//!
//! [HW] Framing, targets, reply layout and the property values below were measured on an HS80 RGB
//! Wireless (dongle firmware 5.9.130, headset 5.8.48). [PUB] Property names from public
//! re-implementations (OpenRGB's Corsair peripheral V2 controller, Rexarn's HS80 notes); no
//! code copied. Sets are limited to the properties in `WRITABLE`.

use std::time::{Duration, Instant};

use crate::hid;
use crate::hidpp::{Error, hex};

pub const PAGE: u16 = 0xFF42;
const FRAME: usize = 64;
const REPORT_OUT: u8 = 0x02;
const REPORT_IN: u8 = 0x01;
const REPORT_EVENT: u8 = 0x03;
const EVENT_USAGE: u16 = 2;

pub const DONGLE: u8 = 0x08;
pub const HEADSET: u8 = 0x09;
const GET: u8 = 0x02;
const SET: u8 = 0x01;
/// Reply status for a property the device does not have. [HW]
const UNSUPPORTED: u8 = 0x05;
/// Reply status for a set the headset refuses in hardware mode. [HW]
const NOT_NOW: u8 = 0x09;
pub const MODE_HARDWARE: u32 = 1;
pub const MODE_SOFTWARE: u32 = 2;

/// Properties. [HW] = value seen on the HS80 and its meaning confirmed; [PUB] = name from public
/// sources, value seen but its meaning not independently confirmed.
pub mod prop {
    /// LED brightness 0..1000. [PUB] (HS80 read 330)
    pub const BRIGHTNESS: u8 = 0x02;
    /// 1 = hardware (self-operated) mode, 2 = software mode. [PUB]
    pub const MODE: u8 = 0x03;
    /// Inactivity timer on/off. [PUB]
    pub const SLEEP_ENABLED: u8 = 0x0D;
    /// Inactivity timeout in ms. [PUB] (HS80 read 900000 = 15 min, iCUE's default)
    pub const SLEEP_MS: u8 = 0x0E;
    /// Battery level in tenths of a percent. [HW]
    pub const BATTERY: u8 = 0x0F;
    /// Battery state, see `ChargeState`. [HW] 2 while running on battery.
    pub const BATTERY_STATE: u8 = 0x10;
    pub const VID: u8 = 0x11;
    pub const PID: u8 = 0x12;
    /// Firmware: bytes major, minor, patch (LE u32). [HW]
    pub const FIRMWARE: u8 = 0x13;
    /// Battery voltage in mV. [HW] (3660 at 31%)
    pub const VOLTAGE: u8 = 0x53;
    /// Mic muted (1) or open (0). [HW] read and pushed as an event; a set is refused with status 5
    /// even in software mode, so only the headset (boom, button) changes it.
    pub const MIC_MUTED: u8 = 0xA6;
}

/// Headset properties lghub-lite may set, with their allowed range. Brightness [HW, owner saw the
/// LEDs follow]; the others are added here only once confirmed on hardware.
pub const WRITABLE: [(u8, u32, u32); 3] = [
    (prop::BRIGHTNESS, 0, 1000),
    // [HW] 1 -> 0 -> 1 read back
    (prop::SLEEP_ENABLED, 0, 1),
    // [HW] 15 -> 10 -> 15 min read back; 1 min .. 4 h
    (prop::SLEEP_MS, 60_000, 14_400_000),
];

/// Properties shown by `info` and the report: (target, property, name).
pub const KNOWN: [(u8, u8, &str); 12] = [
    (DONGLE, prop::PID, "dongle PID"),
    (DONGLE, prop::FIRMWARE, "dongle firmware"),
    (HEADSET, prop::PID, "headset PID"),
    (HEADSET, prop::FIRMWARE, "headset firmware"),
    (HEADSET, prop::BATTERY, "battery (0.1%)"),
    (HEADSET, prop::BATTERY_STATE, "battery state"),
    (HEADSET, prop::VOLTAGE, "battery mV"),
    (HEADSET, prop::MODE, "mode"),
    (HEADSET, prop::BRIGHTNESS, "brightness (0-1000)"),
    (HEADSET, prop::SLEEP_ENABLED, "sleep timer on"),
    (HEADSET, prop::SLEEP_MS, "sleep timer ms"),
    (HEADSET, prop::MIC_MUTED, "mic muted"),
];

/// Is this the Bragi command collection (page 0xFF42, usage 1, 64-byte reports)?
pub fn is_corsair(i: &hid::Info) -> bool {
    i.vid == crate::CORSAIR_VID && i.usage_page == PAGE && i.usage == 1 && i.out_len as usize == FRAME
}

/// Corsair Bragi interfaces present: (pid, product, collection).
pub fn interfaces() -> Vec<(u16, String, hid::Info)> {
    hid::enumerate(crate::CORSAIR_VID)
        .unwrap_or_default()
        .into_iter()
        .filter(is_corsair)
        .map(|i| (i.pid, i.product.clone(), i))
        .collect()
}

/// The event collection (page 0xFF42, usage 2) on the same interface as `cmd`.
pub fn event_collection(cmd: &hid::Info) -> Option<hid::Info> {
    hid::enumerate(crate::CORSAIR_VID)
        .unwrap_or_default()
        .into_iter()
        .find(|i| i.iface == cmd.iface && i.usage_page == PAGE && i.usage == EVENT_USAGE)
}

/// A pushed property change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Notice {
    pub target: u8,
    pub property: u16,
    pub value: u32,
}

pub fn parse_event(f: &[u8]) -> Option<Notice> {
    if f.len() < 9 || f[0] != REPORT_EVENT || f[2] != 0x01 {
        return None;
    }
    Some(Notice {
        target: f[1].wrapping_add(DONGLE),
        property: u16::from_le_bytes([f[3], f[4]]),
        value: u32::from_le_bytes([f[5], f[6], f[7], f[8]]),
    })
}

/// Selector text for a Corsair device: `1b1c:PID`, which `device::parse_selector` rejects, so it
/// never collides with a Logitech `PID:SLOT`.
pub fn selector(pid: u16) -> String {
    format!("{:04x}:{pid:04x}", crate::CORSAIR_VID)
}

/// The PID in a `1b1c:PID` selector.
pub fn parse_selector(s: &str) -> Option<u16> {
    let (vid, pid) = s.split_once(':')?;
    let vid = u16::from_str_radix(vid.trim_start_matches("0x"), 16).ok()?;
    (vid == crate::CORSAIR_VID)
        .then(|| u16::from_str_radix(pid.trim_start_matches("0x"), 16).ok())
        .flatten()
}

pub fn get_frame(target: u8, property: u8) -> [u8; FRAME] {
    let mut f = [0u8; FRAME];
    f[..4].copy_from_slice(&[REPORT_OUT, target, GET, property]);
    f
}

/// [HW] The property is two bytes here; with the value right after a one-byte property the
/// headset refuses the set.
pub fn set_frame(target: u8, property: u8, value: u32) -> [u8; FRAME] {
    let mut f = [0u8; FRAME];
    f[..5].copy_from_slice(&[REPORT_OUT, target, SET, property, 0]);
    f[5..9].copy_from_slice(&value.to_le_bytes());
    f
}

/// The reply to a `command` sent to `target`: (status, value), or None for unrelated reports.
fn parse_reply(f: &[u8], target: u8, command: u8) -> Option<(u8, u32)> {
    if f.len() < 8 || f[0] != REPORT_IN || f[1] != target.wrapping_sub(DONGLE) || f[2] != command {
        return None;
    }
    Some((f[3], u32::from_le_bytes([f[4], f[5], f[6], f[7]])))
}

fn check(status: u8, value: u32, property: u8) -> Result<u32, Error> {
    match status {
        0 => Ok(value),
        UNSUPPORTED => Err(Error::NoFeature(property as u16)),
        NOT_NOW => Err(Error::Other(format!(
            "property {property:#04x}: not allowed in this mode (status 0x09)"
        ))),
        s => Err(Error::Other(format!(
            "property {property:#04x}: Corsair status {s:#04x}"
        ))),
    }
}

/// An open Bragi channel to a dongle and the headset behind it.
pub struct Corsair {
    h: hid::Handle,
    pub trace: bool,
    pub timeout: Duration,
    /// Taken around each request. Every open handle sees every reply, and replies do not say
    /// which property they answer, so two lghub-lite processes asking at once (the app and the
    /// background watcher) could take each other's answers. [HW: the app's polls showed up in
    /// another process's reads]
    turn: Option<crate::platform::SharedLock>,
}

impl Corsair {
    pub fn open(info: &hid::Info, trace: bool) -> Result<Corsair, Error> {
        let h = hid::Handle::open(info).map_err(|e| Error::Io(format!("open {}: {e}", info.path)))?;
        Ok(Corsair {
            h,
            trace,
            timeout: Duration::from_millis(1000),
            turn: crate::platform::shared_lock(&format!("corsair-{:04x}", info.pid)),
        })
    }

    fn read(&mut self, ms: u32) -> Result<Option<Vec<u8>>, Error> {
        let r = hid::read_any(&mut [&mut self.h], ms).map_err(|e| Error::Io(e.to_string()))?;
        let r = r.map(|(_, b)| b);
        if self.trace
            && let Some(b) = &r
        {
            eprintln!("<- {}", hex(&b[..b.len().min(16)]));
        }
        Ok(r)
    }

    /// Send one request and wait for its reply: (status, value), holding the shared lock.
    fn request(&mut self, f: &[u8; FRAME]) -> Result<(u8, u32), Error> {
        let turn = self.turn.take();
        if let Some(t) = &turn
            && !t.lock(3000)
        {
            self.turn = turn;
            return Err(Error::Other(
                "headset busy (another lghub-lite is talking to it)".into(),
            ));
        }
        let r = self.exchange(f);
        if let Some(t) = &turn {
            t.unlock();
        }
        self.turn = turn;
        r
    }

    fn exchange(&mut self, f: &[u8; FRAME]) -> Result<(u8, u32), Error> {
        // Replies carry no property: drop anything already waiting so it cannot be taken for
        // the reply to this request.
        while self.read(0)?.is_some() {}
        if self.trace {
            eprintln!("-> {}", hex(&f[..8]));
        }
        self.h.write(f, 1000).map_err(|e| Error::Io(e.to_string()))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            let ms = left.as_millis().min(u32::MAX as u128 - 1) as u32;
            if let Some(r) = self.read(ms)?
                && let Some(v) = parse_reply(&r, f[1], f[2])
            {
                return Ok(v);
            }
        }
    }

    pub fn get(&mut self, target: u8, property: u8) -> Result<u32, Error> {
        let (status, value) = self.request(&get_frame(target, property))?;
        check(status, value, property)
    }

    fn set_once(&mut self, property: u8, value: u32) -> Result<u8, Error> {
        Ok(self.request(&set_frame(HEADSET, property, value))?.0)
    }

    /// Set a headset property from `WRITABLE` and return the value read back. A set refused in
    /// hardware mode is repeated in software mode, and hardware mode is restored whatever happens.
    pub fn set(&mut self, property: u8, value: u32) -> Result<u32, Error> {
        let &(_, lo, hi) = WRITABLE
            .iter()
            .find(|w| w.0 == property)
            .ok_or_else(|| Error::Other(format!("property {property:#04x} is not writable")))?;
        if !(lo..=hi).contains(&value) {
            return Err(Error::Other(format!("value {value} outside {lo}..={hi}")));
        }
        let mut status = self.set_once(property, value)?;
        // anything but software mode counts as hardware mode, its normal state
        if status == NOT_NOW && self.get(HEADSET, prop::MODE)? != MODE_SOFTWARE {
            check(self.set_once(prop::MODE, MODE_SOFTWARE)?, 0, prop::MODE)?;
            let r = self.set_once(property, value);
            let back = self.set_once(prop::MODE, MODE_HARDWARE);
            status = r?;
            check(back?, 0, prop::MODE)?;
        }
        check(status, 0, property)?;
        let now = self.get(HEADSET, property)?;
        if now != value {
            return Err(Error::Other(format!(
                "property {property:#04x}: set {value}, but it reads {now}"
            )));
        }
        Ok(now)
    }

    /// The headset's battery. Fails when the headset is off or out of range (the dongle still
    /// answers for itself).
    pub fn battery(&mut self) -> Result<Battery, Error> {
        let tenths = self.get(HEADSET, prop::BATTERY)?;
        let state = self.get(HEADSET, prop::BATTERY_STATE)?;
        // the voltage is extra information; a headset without it still has a battery
        let mv = self.get(HEADSET, prop::VOLTAGE).ok();
        Battery::new(tenths, state, mv)
    }

    /// Auto-off after `minutes` idle, 0 = never: the timer switch (0x0D) and, when on, its
    /// timeout (0x0E, ms).
    pub fn set_auto_off(&mut self, minutes: u32) -> Result<(), Error> {
        if minutes == 0 {
            return self.set(prop::SLEEP_ENABLED, 0).map(drop);
        }
        self.set(prop::SLEEP_MS, minutes.saturating_mul(60_000))?;
        self.set(prop::SLEEP_ENABLED, 1).map(drop)
    }

    /// The headset's adjustable settings; a property the headset lacks is None.
    pub fn settings(&mut self) -> Result<Settings, Error> {
        let opt = |r: Result<u32, Error>| match r {
            Ok(v) => Ok(Some(v)),
            Err(Error::NoFeature(_)) => Ok(None),
            Err(e) => Err(e),
        };
        let brightness = opt(self.get(HEADSET, prop::BRIGHTNESS))?.map(|v| v.min(1000) as u16);
        let sleep_on = opt(self.get(HEADSET, prop::SLEEP_ENABLED))?;
        let sleep_ms = opt(self.get(HEADSET, prop::SLEEP_MS))?;
        let mic_muted = opt(self.get(HEADSET, prop::MIC_MUTED))?.map(|v| v != 0);
        Ok(Settings {
            brightness,
            sleep_minutes: match (sleep_on, sleep_ms) {
                (Some(0), _) => Some(0),
                (_, Some(ms)) => Some(ms / 60_000),
                _ => None,
            },
            mic_muted,
        })
    }
}

/// A Corsair headset's adjustable settings as read from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// LED brightness 0..=1000.
    pub brightness: Option<u16>,
    /// Auto-off after this many idle minutes; 0 = never.
    pub sleep_minutes: Option<u32>,
    pub mic_muted: Option<bool>,
}

/// Is `property` settable (in `WRITABLE`)?
pub fn writable(property: u8) -> bool {
    WRITABLE.iter().any(|w| w.0 == property)
}

/// [PUB] OpenRGB's values; [HW] 2 seen while running on battery.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChargeState {
    Charging,
    Discharging,
    Full,
    Unknown(u32),
}

impl ChargeState {
    pub fn from(v: u32) -> ChargeState {
        match v {
            1 => ChargeState::Charging,
            2 => ChargeState::Discharging,
            3 => ChargeState::Full,
            v => ChargeState::Unknown(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    pub tenths: u16,
    pub state: ChargeState,
    pub mv: Option<u16>,
}

impl Battery {
    pub fn new(tenths: u32, state: u32, mv: Option<u32>) -> Result<Battery, Error> {
        if tenths > 1000 {
            return Err(Error::BadReply("battery level above 100%"));
        }
        Ok(Battery {
            tenths: tenths as u16,
            state: ChargeState::from(state),
            mv: mv.and_then(|v| u16::try_from(v).ok()).filter(|&v| v > 0),
        })
    }

    pub fn percent(&self) -> u8 {
        ((self.tenths + 5) / 10) as u8
    }
}

impl std::fmt::Display for Battery {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}%", self.percent())?;
        match self.state {
            ChargeState::Charging => f.write_str(" charging")?,
            ChargeState::Discharging => f.write_str(" discharging")?,
            ChargeState::Full => f.write_str(" full")?,
            ChargeState::Unknown(v) => write!(f, " (state {v})")?,
        }
        match self.mv {
            Some(mv) => write!(f, " ({mv} mV)"),
            None => Ok(()),
        }
    }
}

/// Firmware property as `major.minor.patch`.
pub fn firmware_text(v: u32) -> String {
    let b = v.to_le_bytes();
    format!("{}.{}.{}", b[0], b[1], b[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(head: &[u8]) -> [u8; 64] {
        let mut f = [0u8; 64];
        f[..head.len()].copy_from_slice(head);
        f
    }

    #[test]
    fn get_frame_layout() {
        let f = get_frame(HEADSET, prop::BATTERY);
        assert_eq!(&f[..4], &[0x02, 0x09, 0x02, 0x0F]);
        assert!(f[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn set_frames_measured_on_an_hs80() {
        // [HW] accepted in software mode: brightness 1000, then software and hardware mode
        assert_eq!(
            &set_frame(HEADSET, prop::BRIGHTNESS, 1000)[..9],
            &[0x02, 0x09, 0x01, 0x02, 0x00, 0xE8, 0x03, 0x00, 0x00]
        );
        assert_eq!(
            &set_frame(HEADSET, prop::MODE, MODE_SOFTWARE)[..6],
            &[0x02, 0x09, 0x01, 0x03, 0x00, 0x02]
        );
        // [HW] refused in hardware mode; a set reply's value bytes are stale
        let refused = frame(&[0x01, 0x01, 0x01, 0x09, 0x4A, 0x01, 0x00, 0x00]);
        let (s, v) = parse_reply(&refused, HEADSET, SET).unwrap();
        assert!(check(s, v, prop::BRIGHTNESS).is_err());
        let ok = frame(&[0x01, 0x01, 0x01, 0x00, 0x4A, 0x01, 0x00, 0x00]);
        assert_eq!(parse_reply(&ok, HEADSET, SET), Some((0, 330)));
        assert!(writable(prop::BRIGHTNESS));
        assert!(!writable(prop::MODE) && !writable(prop::BATTERY));
    }

    #[test]
    fn replies_measured_on_an_hs80() {
        // [HW] dongle PID, headset battery / state / voltage / firmware, unsupported property
        let pid = frame(&[0x01, 0x00, 0x02, 0x00, 0x73, 0x0A, 0x00, 0x00, 0x1B]);
        assert_eq!(parse_reply(&pid, DONGLE, GET), Some((0, 0x0A73)));
        assert_eq!(parse_reply(&pid, HEADSET, GET), None);
        let bat = frame(&[0x01, 0x01, 0x02, 0x00, 0x36, 0x01, 0x00, 0x00]);
        assert_eq!(parse_reply(&bat, HEADSET, GET), Some((0, 310)));
        let fw = frame(&[0x01, 0x01, 0x02, 0x00, 0x05, 0x08, 0x30, 0x00]);
        assert_eq!(firmware_text(parse_reply(&fw, HEADSET, GET).unwrap().1), "5.8.48");
        let none = frame(&[0x01, 0x01, 0x02, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x34, 0x8C, 0xF4]);
        let (s, v) = parse_reply(&none, HEADSET, GET).unwrap();
        assert_eq!(check(s, v, 0x46), Err(Error::NoFeature(0x46)));
        // a set reply is not a get reply
        assert_eq!(parse_reply(&frame(&[0x01, 0x01, 0x01, 0x00]), HEADSET, GET), None);

        let b = Battery::new(310, 2, Some(3660)).unwrap();
        assert_eq!(b.percent(), 31);
        assert_eq!(b.to_string(), "31% discharging (3660 mV)");
        assert_eq!(Battery::new(1000, 3, None).unwrap().to_string(), "100% full");
        assert!(Battery::new(1001, 2, None).is_err());
    }

    #[test]
    fn events_measured_on_an_hs80() {
        // [HW] mic muted, then charger plugged in
        let mute = frame(&[0x03, 0x01, 0x01, 0xA6, 0x00, 0x01, 0x00, 0x00, 0x00, 0x34, 0x8C, 0xF4]);
        assert_eq!(
            parse_event(&mute),
            Some(Notice {
                target: HEADSET,
                property: prop::MIC_MUTED as u16,
                value: 1
            })
        );
        let charging = frame(&[0x03, 0x01, 0x01, 0x10, 0x00, 0x01, 0x00, 0x00, 0x00]);
        let n = parse_event(&charging).unwrap();
        assert_eq!((n.property, ChargeState::from(n.value)), (0x10, ChargeState::Charging));
        // a get reply is not an event
        assert_eq!(parse_event(&frame(&[0x01, 0x01, 0x02, 0x00, 0x36, 0x01])), None);
    }

    #[test]
    fn selectors() {
        assert_eq!(selector(0x0A73), "1b1c:0a73");
        assert_eq!(parse_selector("1b1c:0a73"), Some(0x0A73));
        assert_eq!(parse_selector("1B1C:0A73"), Some(0x0A73));
        assert_eq!(parse_selector("c54d:1"), None);
        assert_eq!(parse_selector("0a73"), None);
        // and the Logitech parser does not take a Corsair selector
        assert_eq!(crate::device::parse_selector("1b1c:0a73"), None);
    }

    #[test]
    fn only_the_ff42_command_collection_matches() {
        let info = |vid, page, usage, out| hid::Info {
            path: String::new(),
            vid,
            pid: 0x0A73,
            usage_page: page,
            usage,
            in_len: 64,
            out_len: out,
            feat_len: 0,
            product: "CORSAIR HS80 RGB Wireless Gaming Receiver".into(),
            iface: String::new(),
        };
        // [HW] 0a73 interface 3: FF42/1 64/64 yes; FF42/2 (input only), FF58/1, 000C/1 no
        assert!(is_corsair(&info(0x1B1C, 0xFF42, 1, 64)));
        assert!(!is_corsair(&info(0x1B1C, 0xFF42, 2, 0)));
        assert!(!is_corsair(&info(0x1B1C, 0xFF58, 1, 64)));
        assert!(!is_corsair(&info(0x1B1C, 0x000C, 1, 0)));
        assert!(!is_corsair(&info(0x046D, 0xFF42, 1, 64)));
    }
}
