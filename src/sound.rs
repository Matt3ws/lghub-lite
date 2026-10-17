//! lghub-lite's own headset tones, made here (no sound files).
//!
//! A Corsair headset held in software mode (lights fully off, see `Corsair::set_lights`) no
//! longer plays its mute and low-battery prompts: they are recordings the firmware plays only in
//! hardware mode, and in software mode it just reports the mute change. [HW owner report
//! 2026-10-17; PUB the HS80 MAX firmware notes] So the watcher plays these instead, on the
//! headset's own audio output. Windows only so far (`platform::play_pcm`).

use crate::platform;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Muted,
    Unmuted,
    LowBattery,
    VeryLowBattery,
}

impl Tone {
    pub fn parse(s: &str) -> Option<Tone> {
        match s {
            "mute" | "muted" => Some(Tone::Muted),
            "unmute" | "unmuted" => Some(Tone::Unmuted),
            "low" => Some(Tone::LowBattery),
            "verylow" => Some(Tone::VeryLowBattery),
            _ => None,
        }
    }

    /// (frequency in Hz, ms); 0 Hz is a pause.
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            // falling for mute, rising for unmute
            Tone::Muted => &[(1320.0, 80), (0.0, 25), (880.0, 130)],
            Tone::Unmuted => &[(880.0, 80), (0.0, 25), (1320.0, 130)],
            Tone::LowBattery => &[(660.0, 120), (0.0, 90), (660.0, 120)],
            Tone::VeryLowBattery => &[(520.0, 100), (0.0, 70), (520.0, 100), (0.0, 70), (520.0, 100)],
        }
    }
}

/// Battery at or below this is low, and at or below `VERY_LOW` very low (percent).
pub const LOW: u8 = 15;
pub const VERY_LOW: u8 = 5;

/// Points above a level the battery must climb (charging) before its tone can play again, so a
/// reading that wobbles across the line (15, 16, 15 %) warns only once.
const REARM: u8 = 3;

fn level(percent: u8) -> u8 {
    match percent {
        p if p <= VERY_LOW => 2,
        p if p <= LOW => 1,
        _ => 0,
    }
}

/// The battery tone due at `percent`: once when it falls to `LOW` and once more at `VERY_LOW`.
/// `warned` (0 none, 1 low, 2 very low) remembers what was played; charging clearly back above
/// a level (`REARM`) lowers it again.
pub fn battery_warning(percent: u8, warned: &mut u8) -> Option<Tone> {
    let now = level(percent);
    if now > *warned {
        *warned = now;
        return Some(if now == 2 {
            Tone::VeryLowBattery
        } else {
            Tone::LowBattery
        });
    }
    if now < *warned && level(percent.saturating_sub(REARM)) < *warned {
        *warned = level(percent.saturating_sub(REARM));
    }
    None
}

/// The tone for a mic event: `was` is the state known before it (None: not read yet).
pub fn mic_tone(was: Option<bool>, now: Option<bool>) -> Option<Tone> {
    match now {
        Some(m) if was != Some(m) => Some(if m { Tone::Muted } else { Tone::Unmuted }),
        _ => None,
    }
}

pub const RATE: u32 = 48_000;
/// Peak level, a fraction of full scale: a prompt, not an alarm.
const LEVEL: f32 = 0.2;
/// Fade in and out of each note, so it does not click.
const FADE_MS: u32 = 6;

/// The tone as 16-bit mono PCM at `RATE`.
pub fn samples(t: Tone) -> Vec<i16> {
    let mut out = Vec::new();
    let fade = (RATE * FADE_MS / 1000) as f32;
    for &(hz, ms) in t.notes() {
        let n = (RATE * ms / 1000) as usize;
        for i in 0..n {
            let edge = (i.min(n - 1 - i) as f32 / fade).min(1.0);
            let s = (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin();
            out.push((s * edge * LEVEL * i16::MAX as f32) as i16);
        }
    }
    out
}

/// The audio output of the headset whose USB name is `product`. Windows names an output
/// "<endpoint> (<device>)" and cuts it to 31 characters, e.g. "Headset Earphone (CORSAIR HS80 "
/// for "CORSAIR HS80 RGB Wireless Gaming Receiver" [HW], so the part in brackets must begin the
/// product name.
pub fn output_for(product: &str, outputs: &[(u32, String)]) -> Option<(u32, String)> {
    let product = product.to_lowercase();
    outputs
        .iter()
        .find(|(_, name)| {
            let Some((_, device)) = name.split_once('(') else {
                return false;
            };
            let device = device.trim_end_matches(')').trim().to_lowercase();
            device.len() >= 8 && product.starts_with(&device)
        })
        .cloned()
}

/// Play `t` on the audio output of headset `product` and return once it has played; the
/// output's name on success.
pub fn play(t: Tone, product: &str) -> Result<String, String> {
    let (id, name) =
        output_for(product, &platform::audio_outputs()).ok_or_else(|| format!("no audio output for {product}"))?;
    platform::play_pcm(id, RATE, &samples(t))?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Tone; 4] = [Tone::Muted, Tone::Unmuted, Tone::LowBattery, Tone::VeryLowBattery];
    const HS80: &str = "CORSAIR HS80 RGB Wireless Gaming Receiver";

    fn outs(names: &[&str]) -> Vec<(u32, String)> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| (i as u32, n.to_string()))
            .collect()
    }

    /// Frequency of a stretch of samples, from its zero crossings.
    fn hz(s: &[i16]) -> f32 {
        let crossings = s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        crossings as f32 / 2.0 / (s.len() as f32 / RATE as f32)
    }

    fn ms(n: u32) -> usize {
        (RATE * n / 1000) as usize
    }

    #[test]
    fn the_output_is_found_by_its_cut_name() {
        let o = outs(&[
            "Speakers (Realtek(R) Audio)",
            // [HW] Windows 11, HS80 RGB Wireless
            "Headset Earphone (CORSAIR HS80 ",
            "LG ULTRAGEAR (NVIDIA High Defin",
        ]);
        assert_eq!(output_for(HS80, &o).map(|o| o.0), Some(1));
        assert_eq!(output_for("CORSAIR VIRTUOSO", &o), None);
        // a short bracket ("(R)") never matches by accident
        assert_eq!(output_for("R", &o), None);
    }

    #[test]
    fn output_names_in_other_shapes() {
        // a name short enough to keep its closing bracket, in another case
        let o = outs(&["Headphones (corsair hs80 rgb wireless gaming receiver)"]);
        assert_eq!(output_for(HS80, &o).map(|o| o.0), Some(0));
        // no brackets at all, or nothing in them
        assert_eq!(
            output_for(HS80, &outs(&["CORSAIR HS80 RGB Wireless", "Headset ()"])),
            None
        );
        // a bracket naming more than the product is another device
        assert_eq!(
            output_for("CORSAIR HS80", &outs(&["Headset (CORSAIR HS80 RGB Wi"])),
            None
        );
        // two outputs of the headset: the first one listed
        let o = outs(&["Speakers (CORSAIR HS80 RGB Wir", "Headset Earphone (CORSAIR HS80 "]);
        assert_eq!(output_for(HS80, &o).map(|o| o.0), Some(0));
        assert_eq!(output_for(HS80, &[]), None);
    }

    #[test]
    fn tone_names() {
        assert_eq!(Tone::parse("mute"), Some(Tone::Muted));
        assert_eq!(Tone::parse("muted"), Some(Tone::Muted));
        assert_eq!(Tone::parse("unmute"), Some(Tone::Unmuted));
        assert_eq!(Tone::parse("low"), Some(Tone::LowBattery));
        assert_eq!(Tone::parse("verylow"), Some(Tone::VeryLowBattery));
        assert_eq!(Tone::parse("Mute"), None);
        assert_eq!(Tone::parse(""), None);
    }

    #[test]
    fn tones_are_short_and_quiet() {
        for t in ALL {
            let s = samples(t);
            assert!(s.len() < RATE as usize, "{t:?} under a second");
            let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap();
            assert!(peak <= (LEVEL * i16::MAX as f32) as u16 + 1);
            // loud enough to hear
            assert!(peak > (LEVEL * i16::MAX as f32 * 0.9) as u16, "{t:?} peak {peak}");
            // fades in and out
            assert!(s[0].unsigned_abs() < 100 && s[s.len() - 1].unsigned_abs() < 100);
        }
    }

    #[test]
    fn tones_last_as_long_as_their_notes() {
        for t in ALL {
            let total: usize = t.notes().iter().map(|&(_, n)| ms(n)).sum();
            assert_eq!(samples(t).len(), total, "{t:?}");
        }
    }

    #[test]
    fn pauses_are_silent_and_notes_fade_at_both_ends() {
        let s = samples(Tone::LowBattery);
        // 120 ms beep, 90 ms pause, 120 ms beep
        assert!(s[ms(120)..ms(210)].iter().all(|&v| v == 0));
        for edge in [ms(120) - 1, ms(210)] {
            assert!(s[edge].unsigned_abs() < 100, "click at sample {edge}");
        }
    }

    #[test]
    fn mute_falls_and_unmute_rises() {
        let m = samples(Tone::Muted);
        let u = samples(Tone::Unmuted);
        // notes 80 ms, pause 25 ms, 130 ms; measured inside each note
        let (first, last) = (ms(5)..ms(75), ms(110)..ms(230));
        let (m1, m2) = (hz(&m[first.clone()]), hz(&m[last.clone()]));
        let (u1, u2) = (hz(&u[first]), hz(&u[last]));
        assert!(
            (m1 - 1320.0).abs() < 30.0 && (m2 - 880.0).abs() < 30.0,
            "mute {m1} -> {m2}"
        );
        assert!(
            (u1 - 880.0).abs() < 30.0 && (u2 - 1320.0).abs() < 30.0,
            "unmute {u1} -> {u2}"
        );
        assert_ne!(m, u);
    }

    #[test]
    fn very_low_sounds_lower_and_more_urgent_than_low() {
        let beeps = |t: Tone| t.notes().iter().filter(|n| n.0 > 0.0).count();
        assert_eq!(beeps(Tone::LowBattery), 2);
        assert_eq!(beeps(Tone::VeryLowBattery), 3);
        let low = hz(&samples(Tone::LowBattery)[ms(5)..ms(115)]);
        let very = hz(&samples(Tone::VeryLowBattery)[ms(5)..ms(95)]);
        assert!(very < low, "{very} Hz vs {low} Hz");
    }

    #[test]
    fn mic_events_play_only_on_a_change() {
        assert_eq!(mic_tone(Some(false), Some(true)), Some(Tone::Muted));
        assert_eq!(mic_tone(Some(true), Some(false)), Some(Tone::Unmuted));
        // the same state again (a repeated event) is not a change
        assert_eq!(mic_tone(Some(true), Some(true)), None);
        assert_eq!(mic_tone(Some(false), Some(false)), None);
        // state not read yet: the event is the news
        assert_eq!(mic_tone(None, Some(true)), Some(Tone::Muted));
        assert_eq!(mic_tone(None, Some(false)), Some(Tone::Unmuted));
        assert_eq!(mic_tone(Some(true), None), None);
    }

    fn heard(readings: &[u8], warned: &mut u8) -> Vec<Tone> {
        readings.iter().filter_map(|&p| battery_warning(p, warned)).collect()
    }

    #[test]
    fn battery_warnings_play_once_per_level() {
        let mut w = 0;
        let all = heard(&[40, 16, 15, 14, 9, 6, 5, 4, 0], &mut w);
        assert_eq!(all, [Tone::LowBattery, Tone::VeryLowBattery]);
        // charged back well above low, then down again: warned again
        assert_eq!(heard(&[30, 15], &mut w), [Tone::LowBattery]);
        // switched on already very low: only the very-low tone
        let mut w = 0;
        assert_eq!(heard(&[3, 4], &mut w), [Tone::VeryLowBattery]);
    }

    #[test]
    fn a_wobbling_reading_warns_once() {
        let mut w = 0;
        assert_eq!(heard(&[17, 15, 16, 15, 16, 17, 18, 15], &mut w), [Tone::LowBattery]);
        assert_eq!(heard(&[6, 5, 6, 5, 7, 8, 5], &mut w), [Tone::VeryLowBattery]);
    }

    #[test]
    fn charging_rearms_each_level_on_its_own() {
        // very low, then charged to just over the very-low line + REARM: low stays played
        let mut w = 0;
        heard(&[15, 5], &mut w);
        assert_eq!(
            heard(&[VERY_LOW + REARM + 1, 14, VERY_LOW], &mut w),
            [Tone::VeryLowBattery]
        );
        // the low line rearms only above LOW + REARM
        let mut w = 0;
        heard(&[LOW], &mut w);
        assert_eq!(heard(&[LOW + REARM, LOW], &mut w), []);
        assert_eq!(heard(&[LOW + REARM + 1, LOW], &mut w), [Tone::LowBattery]);
        // straight from full to empty (switched off and back on): one very-low tone
        let mut w = 0;
        assert_eq!(heard(&[100, 2], &mut w), [Tone::VeryLowBattery]);
    }
}
