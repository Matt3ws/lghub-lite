# peripheral-lite — engineering log

Minimal Rust CLI that talks HID++ directly to Logitech hardware on Windows. No G HUB dependency at runtime.
Not affiliated with or endorsed by Logitech. User-facing documentation is in [README.md](README.md); this file is the developer's engineering log.

**How this was figured out.** Behaviour was learned for interoperability from three sources:
- Logitech G HUB's own device layer, examined statically; addresses such as `FUN_140ccee80` or `0x140d26650` refer to functions in `lghub_agent.exe` (SHA-256 `04c1f3e8…`, G HUB 2023.1);
- public HID++ knowledge (Solaar, libratbag);
- tests on real hardware.

Only facts needed for interoperability (IDs, byte layouts, sequences) are recorded here, in our own words; no Logitech code or data files are included. `§N` citations refer to the author's private research notes (`protocol.md`), which are **not published** because they contain material derived from decompiled software.

Tags: `[OBS]` seen in G HUB, `[PUB]` public HID++ knowledge (Solaar/libratbag), `[INF]` inferred, `[UNK]` unknown, `[HW]` verified on hardware here.

## 1. Status

**Hardware used:**
- PRO X SUPERLIGHT 2 ("SL2"):
  - over the LIGHTSPEED receiver `c54d` (selector `c54d:1`);
  - over its USB cable as `c09b`.
- PRO X Wireless headset `0aba` over USB (its own dongle).

| Command | State |
|---|---|
| `devices` | ✅ direct devices, receivers, and paired slots with online state |
| `info <dev>` | ✅ protocol version, name, type, firmware entities, full feature table |
| `battery <dev>` | ✅ SL2 via 0x1004 (receiver and cable). ✅ Headset via 0x1F20: voltage, plus **percentage and hours left from a user-supplied power-model XML** (matches G HUB: 78 % / 18.8 h vs 78 % / 18.7 h). Without the file, voltage only |
| `eq <dev>` | ✅ headset 0x8310 hardware EQ, **read-only** (10 bands, ±12 dB). Writing is not supported; see §2 Headset |
| `dpi <dev> [v]` | ✅ read via 0x2201/0x2202. ✅ write via 0x2202 with read-back in host mode, over the cable and the receiver. 0x2201 write is unverified (no device) |
| `rate <dev> [hz]` | ✅ 0x8061 read/write with read-back in host mode, over the cable and the receiver. The Hz mapping is [PUB] and unmeasured |
| `mode <dev> [host\|onboard]` | ✅ 0x8100 get/set with read-back, over the cable and the receiver |
| `watch` | ✅ event-driven monitor: device arrival/removal, receiver slot connect/disconnect (incl. sleep/wake), DPI/rate/battery events. Headset battery (0x1F20) is read every 5 min and printed only when the % changes, the one timer in the program (see Headset). Idle CPU 0, one thread between reads. Human-run hot-plug test passed (TEST REQUEST 5) |
| `preset <dev> [dpi=N] [rate=HZ] \| clear`, `presets`, `apply <dev>` | ✅ per-model presets in `%APPDATA%\peripheral-lite\presets.txt` (`--config` overrides). `apply` and `watch` switch to host mode and set DPI/rate with read-back (SL2 over the receiver) |
| `profiles <dev>`, `dump <dev> <file>` | ✅ read-only 0x8100 onboard memory: info, active profile, user/factory directories, per-sector CRC status, full text backup (atomic write). Nothing is written to the device. SL2 backup saved at `%APPDATA%\peripheral-lite\onboard-backup-40a9-2026-10-15.txt` |
| `autostart [on\|off]` | ✅ per-user login entry (HKCU `Run` value `peripheral-lite`) for `peripheral-lite-bg.exe`; `on` also starts it, `off` also stops it [HW: on → running, second copy exits, log written; off → entry removed, process ended] |
| `peripheral-lite-bg.exe` | ✅ windowless `watch` (single instance via the `Local\peripheral-lite-bg` mutex), output with timestamps in `watch.log` next to the preset file, moved to `watch.log.old` at start-up once over 1 MB |
| `peripheral-lite-gui.exe` | ✅ (Windows) egui window + tray, in the separate `gui/` package. [HW]: lists the SL2 and headset with battery (headset bar 73 %); close hides to the tray; hidden idle 0 ms CPU over 20 s; a second launch re-shows the window and exits. Redesigned 2026-10-16 as a skinnable undecorated widget (display, segment gauge, 24 h history from `history-<device>.csv`, alert level, settings drawer, skin manager with `.json` skins); [HW] renders both devices, 0 ms CPU idle over 15 s. Clicking through it still needs a human test (TEST REQUEST 10); the same `Monitor::exec` code is hardware-verified via the CLI |
| sidetone | ❌ removed: 0x8300 level reads and writes back, but a human heard no effect at 50 (2026-10-15). Facts are kept in §2 Headset |
| `raw <dev> <hex…> --allow-raw` | implemented, debug only |
| `--trace` | prints every sent/received report in hex; reports that don't match the pending request are marked `(ignored)` |

**MVP done** (2026-10-15). The full chain works on hardware, and the human confirmed the physical effect: the cursor was noticeably slower at 400 DPI and normal at 800 (SL2, USB cable).

## 2. Protocol facts used by the code

### Architecture
A Cargo workspace: the core package (one library plus two thin binaries; no traits, no async) and a separate `gui/` package, so the core's dependency tree stays `windows-sys` only:
- `src/lib.rs`: module list, `verbose`, the Logitech VID.
- `src/main.rs`: the `peripheral-lite` CLI (argument parsing and output).
- `src/bin/bg.rs`: `peripheral-lite-bg`, the windowless watcher. Built with `windows_subsystem = "windows"`; routes stdout/stderr into the log with `SetStdHandle` (std looks the handle up on every write).
- `src/autostart.rs`: HKCU `Run` value, plus starting (`CreateProcessW`) and stopping (Toolhelp + `TerminateProcess`) the watcher. `std::process::Command` was avoided: it added ~74 KB.
- `src/hid.rs`: SetupDi enumeration, HidP caps, CreateFileW (shared R/W, overlapped).
  - One persistent overlapped read per collection, cancelled and drained on drop.
  - `wait_any` is a single `WaitForMultipleObjects` over all collections plus an optional extra event.
  - `Notifier` wraps `CM_Register_Notification` for HID interface arrival/removal.
- `src/hidpp.rs`: framing, swid, reply matching, error decoding, retries, queue of unsolicited reports, Root/ping, feature names.
- `src/device.rs`: interface grouping, receiver slots, identity, battery, DPI, report rate, onboard mode, event decoding.
- `src/watch.rs`: `Monitor` (open, sync, `next` → `Vec<Update>`, `exec(sel, Cmd)` → `Snapshot`) shared by the CLI's `watch` (prints each `Update` via `Display`, lines unchanged and tested) and the GUI. Applies presets on every attach unless switched off (the GUI switches it off while `peripheral-lite-bg` runs).
- `gui/src/main.rs`: `peripheral-lite-gui`.
  - eframe 0.37 (glow renderer) + tray-icon 0.26.
  - **Two threads:** the UI thread, and one worker that owns the `Monitor` and does all HID I/O, because requests can block for seconds while a wireless device wakes. Commands go over an `mpsc` channel plus `hid::Waker`; updates come back over a channel plus `request_repaint`.
  - **Tray:** eframe's `App::logic` also runs while the window is hidden whenever a repaint is requested, so tray clicks restore the window without Win32 workarounds.
  - **Single instance:** mutex `Local\peripheral-lite-gui`. A second launch shows the first window via `FindWindowW` + `ShowWindow` and exits.
  - **Headset without a curve file:** the device view shows the voltage, explains why there is no percentage, and offers **Import from G HUB** (`audio::find_ghub_curve` searches `C:\ProgramData\LGHUB\depots` at most three folders deep for a `*battery*.xml` whose `pid` matches the model; only on that click) and **Open folder**. `Monitor::snapshot` re-checks for a curve, so an import takes effect without a restart. [HW: the found path was `…\162476\pro_headset\pro_x_wireless_headset_battery.xml`]
  - **Rename (lghub-lite → peripheral-lite, after v0.4.3):** `src/migrate.rs`, run first by the CLI, `peripheral-lite-bg` and the app (not in `--demo`). When something is left under the old name it takes the `migrate` lock, then: stops an old `lghub-lite-bg` (and, in the app, an old `lghub-lite-gui`) found through their old mutexes / lock files (`Local\lghub-lite-*`, `lghub-lite-*.lock`); moves each entry of the old settings folder into the new one if that is missing or holds only lock files (rename, else copy and leave the original); rewrites the old login entry (Run value `lghub-lite`, `~/.config/autostart/lghub-lite.desktop`, LaunchAgent `io.github.lghub-lite`) to the same program under the new name found next to the running exe (macOS: also in/around `peripheral-lite.app`), with an old-folder `--config` path renamed, and removes the old entry; restarts the watcher as `peripheral-lite-bg` if an old one was running. Each step is a no-op once done. **v0.4.3's updater cannot reach a peripheral-lite release** (it looks for `lghub-lite-<tag>-windows-x64.zip` and the old exe names), so users update by hand once; no bridge release.
  - **Autostart:** writes the same HKCU `Run` value as the CLI (`"…\peripheral-lite-gui.exe" --tray`), so only one of the two watchers starts at login.
  - **Updates** (`gui/src/update.rs`): only on a click. `GET api.github.com/repos/…/releases` via System32 `curl.exe` (no TLS crate), highest `vX.Y.Z` `tag_name` wins (every release is a pre-release, so `/releases/latest` would return nothing). Install (Windows): download `peripheral-lite-<tag>-windows-x64.zip` + `.sha256`, verify (own SHA-256), extract with System32 `tar.exe`, rename each running exe to `*.old` and copy the new one in, undoing on failure; then start `peripheral-lite-gui.exe --updated` (waits up to 10 s for the instance lock) and the bg watcher if it ran, and exit. `*.old` are removed at the next start. [Tested: the live v0.4.2 zip installs into a temp folder, `cargo test -- --ignored`]
- `src/device.rs` verified writes: `set_dpi_verified`, `set_rate_verified`, `set_mode_verified`, `explain_refusal`, shared by the CLI and GUI.
- `src/config.rs`: preset file parse/format, apply, and `write_atomic` (write `<file>.tmp`, then rename), shared by presets and dumps.
- `src/profiles.rs`: onboard memory read path (0x8100 f0/f4/f5/f10), CRC-CCITT, directory parsing, dump format.
- `src/audio.rs`: headset features 0x1F20 (battery voltage) and 0x8310 (EQ read).
- `src/battery.rs`: voltage-to-percent curves (measured / G HUB file / built-in / generic estimate), the voltage recording and the curve builder, all in fixed-point integers.
- `gui/`: the desktop app, a separate Cargo project (own `Cargo.lock`, `target/`, `[workspace]`) that uses the root as a library. The root has no workspace, so deleting `gui/` leaves a working CLI-only tree (checked by building a copy without it, 2026-10-16). README screenshots come from `peripheral-lite-gui.exe --demo` (`docs/screenshots/`).
- `dev/` (not part of the app): `agent.ps1` (GET-only G HUB agent client) and `calibrate.ps1` (logs peripheral-lite vs G HUB battery readings to `%APPDATA%\peripheral-lite\battery-calibration.csv`).

**Threads.** Our code runs on one thread. Windows runs the `CM_Register_Notification` callback on a thread-pool thread. The callback only calls `SetEvent`, and the main thread waits on that event; no window or message loop is needed. While idle the process has 1 thread [HW, see §8].

### Transport
- **Collection selection.** We accept page `FF00` usage 1 (7 B) or 2 (20 B), and page `FF43` where usage `HHLL` has LL a single bit and `HH&LL≠0`. [OBS §9.2]
  - We also require the length to match a HID++ report, which excludes the headset's 64-byte `FF00/0001` collection. [INF]
  - [HW] receiver c54d `mi_02` and SL2 wired c09b `mi_02` both have `FF00/0001 in=7 out=7` and `FF00/0002 in=20 out=20`.
  - [HW] headset 0aba `mi_03` has `FF43/0202 in=20 out=20`. That is the collection G HUB opens.
- **Grouping.** Collections of one interface are grouped by path with `&colNN` and the last `&NNNN` of the instance removed. [HW]
- **Write.** Each message goes to the first collection whose output length is at least the message length. It is zero-padded and byte 0 is rewritten to that collection's report ID. [OBS §9.3] [HW]
- **Reads.** We read both collections. [HW]
  - The SL2 answers short (`10`) requests with long (`11`) reports, over both the receiver and the cable.
  - Receiver HID++ 1.0 short register replies come back as `10`.
- **Share mode** `FILE_SHARE_READ|WRITE` works while G HUB runs. [HW]
- **Every open handle receives every input report.** A `watch` process sees the events caused by another process's writes. [HW]

### Request/response
- **Software ID.** We use swid **0x03** (`--swid` overrides). G HUB uses 0x0A..0x0E [OBS §9.4]. No collision was seen with G HUB running. [HW]
- **Matching.** We compare `rsp[1..3]==cmd[1..3]` plus per-call echo offsets. The report ID is ignored and replies must be at least 5 bytes. [OBS §9.5]
  - Echo offsets: ping byte 6; 0x2201 f1/f2 and 0x2202 f5/f6 byte 4; 0x2202 f2 bytes 4–6; B5 sub-address byte 4. [OBS dec_all.c FUN_140ccee80, FUN_140cd04e0, FUN_140cd0df0, FUN_140cd0ff0]
- **Unmatched reports** arriving during a transaction (events, other software's replies) are queued (max 64) for `watch`.
- **Errors.** [OBS §9.5]
  - HID++ 2.0: `rsp[2]==FF`, code in `rsp[5]`.
  - HID++ 1.0: `rsp[2]==8F`, code in `rsp[5]`; code names [PUB].
  - [HW] the receiver answers a 2.0 ping with `10 FF 8F 00 13 01 00`. This is how we detect a receiver. [OBS §9.6 NotHpp20]
  - [HW] empty pairing slots answer `10 FF 8F 83 B5 09 00`.
- **Timeout and retries.**
  - 2000 ms per attempt.
  - **2 retries** on timeout or Busy (0x08), pausing 100 + (10 << attempt) ms. G HUB uses 3000 ms, 9 retries and `rand()%500 + (10<<attempt)` ms [OBS §9.6]; dropping the random part is [INF].
  - Resending is safe here because every request is a read or a write of an absolute value.
  - Motivation [HW]: the first request after the SL2 had idled (asleep) timed out at 1000 ms.
  - Receiver slot pings in `devices` use a 600 ms timeout per attempt.
- **Duplicated reply.** Seen once on the SL2 right after wake. The second copy no longer matches a pending request and is ignored. [HW]

### Receiver (HID++ 1.0, device index 0xFF)
- **Reg 0x02 read.** `-> 10 FF 81 02 00 00 00`, `<- 10 FF 81 02 00 01 00`. `p[1]` = 1 connected device. [OBS §9.8 bytes; PUB meaning; HW]
- **Reg 0x00 read** (notification flags; read only, never written). [HW]
  - `-> 10 FF 81 00 00 00 00`, `<- … 00 01 00`.
  - [PUB Solaar] 0x000100 = wireless notifications enabled. Someone (the firmware default or G HUB) already set it, so we do not need to write it.
  - The bits G HUB itself writes are [UNK] (§9.8).
- **Pairing info.** `-> 10 FF 83 B5 20 00 00`, `<- 11 FF 83 B5 20 08 08 40 A9 11 00 02 B1 01 00 00 7F 24 00 00`. [OBS §9.8 request; PUB layout; HW]
  - `p[3..5]` = wireless PID 0x40A9 (big-endian); `p[7]&0x0F` = 2 = mouse.
- **Online check.** Root ping to device index `slot+1`. [HW]
  - For a paired device whose link is down, the receiver answers at once with `8F` code **0x08** ("unknown device" [PUB]), not a timeout. [HW, receiver just plugged in before the mouse linked] So `devices` and `watch` stay fast with the mouse off; the retries are not used for this case.

### Events decoded by `watch`
HID++ 2.0 events have swid 0 and fn 0 on a known feature index [OBS §9.5].
- **0x41 connection** [OBS §9.8] [HW]: `rsp[1]` = device index, `rsp[3]` = protocol, `rsp[4]` bits 0–3 = type, **bit 6 = link not established**, `rsp[5..6]` = wireless PID little-endian.
  - [HW] decoded as `slot 1 … wpid=40a9 kind=mouse protocol=0x11`. The wpid and kind agree with the B5 pairing reply.
  - Seen on mouse power off ("link lost") and on (connected).
  - Also seen on **sleep**: after a few idle minutes, "link lost", then "connected" on the first movement.
  - Protocol byte 0x11 meaning [UNK]; G HUB skips protocol 2 (§9.8).
- **On reconnect the SL2 announces its state unasked:** a 0x2202 DPI event, a 0x8061 rate event and a 0x1004 battery event, in that order. [HW]
- **0x40 with `rsp[3]==0x02`**: unpaired or disconnected [OBS §9.8]. Not seen; power-off and sleep use 0x41 with bit 6 instead. [HW]
- **0x1004 battery event**: same layout as f1. [OBS §10.5] [HW: `battery 56% level=good discharging external=none` after reconnect]
- **Removal.** The pending read fails with `ERROR_DEVICE_NOT_CONNECTED` (1167), usually before the PnP notification arrives. `watch` drops the interface on either signal. [HW receiver and cable]
- **0x2202 DPI event** `[sensor, X, Y, lod]`. [HW] `11 FF 09 00 00 06 40 06 40 02` (cable); `11 01 09 00 00 03 20 03 20 02` (receiver), seen by `watch` while another process wrote DPI.
- **0x8061 rate event** `[connType, bit]`. [HW] `11 FF 0C 00 00 02` (cable); `11 01 0C 00 01 03` (receiver).

### Identity (HID++ 2.0)
- **Root f1 ping.** `10 idx 00 1s 00 00 5A` → `[major, minor, echo]`. [OBS §9.6] [HW: SL2 4.2, headset 4.2]
- **Root f0 GetFeature** `[idHi, idLo]` → `[index, flags, version]`. [OBS §9.7] [HW]
- **0x0001 FeatureSet.** f0 → count (excludes Root); f1 `[i]` → `[idHi, idLo, flags, version]`. [OBS §9.7] [HW]
  - The SL2 has 32 features with the same indices on receiver and cable: 0x1004 at 6, 0x2202 at 9, 0x8061 at 12, 0x8100 at 13.
- **0x0005 DeviceName.** f0 → length; f1 `[offset]` → chars; f2 → type (3 mouse, 8 headset [PUB]). [OBS §9.7] [HW]
- **0x0003 FirmwareInfo.** [OBS §9.7 + PUB layout] [HW]
  - f0 → `[count, unitId(4), transport(2), modelId(6), …]`. SL2 model IDs are 40A9 (wireless) and C09B (wired).
  - f1 `[e]` → `[type&0x0F, prefix(3 ASCII), number, revision, build BE16, active, transportPid BE16, …]`. SL2: bootloader BL1 71.05 B0029, main MPM 32.05 B0029.
  - **Agent cross-check:** the headset decodes to `U112.01.B0203`; the agent reports `U112` / `112.1.203`. ✅

### Presets
- **Key.** Presets are keyed by the device's **first non-zero model ID** from FirmwareInfo f0, `p[7..13]` as three BE16 values. [OBS §9.7; PUB layout] [HW]
  - SL2 = **40A9**: `02 7A F4 AD 28 00 0C 40 A9 C0 9B 00 00 …`.
  - Headset = **0ABA**: `01 FF FF FF FF 00 03 00 00 00 00 0A BA …`.
  - The key is meant to be the same over the cable and the receiver (the device reports it, not the transport). So far it has only been read over the receiver for the SL2 [UNVERIFIED over the cable].
- **File format.** One line per model: `40a9 dpi=1200 rate=500`. `#` starts a comment.
  - `preset` rewrites the whole file, so hand-written comments are lost; the header says so.
  - Rates must be 125 << n (n = 0..6). DPI is validated against the device's own range list when saved via `preset`.
- **Apply sequence.** If the device has 0x8100 and is not in host mode, set host mode. Then set DPI and rate only where they differ, checking each by read-back.
  - A rate the active connection doesn't offer (e.g. 2000 Hz over the cable) is skipped with a message.
  - In `watch` failures are printed, never fatal.

### Battery
- **0x1004 f1** → `[soc %, level bits (b0 crit, b1 low, b2 good, b3 full), charging, external]`. [OBS §10.5 order, PUB bytes] [HW SL2]
  - Readings: 13 % (receiver, unplugged), then 28 % and 38 % charging on the cable, then 49 % discharging on the receiver.

### DPI
- **0x2201** (layout from G HUB; no device here has it). [OBS §9.9; FUN_140ccee80 / 140ccf0e0 / 140ccf280 / 140ccf450]
  - f1 `[sensor]` list; f2 `[sensor]` → `[sensor, cur, def]`.
  - f3 `[sensor, dpi BE16]`, plus `dpiLevel` if version ≥ 2 (meaning [UNK], we send 0). UNVERIFIED.
- **0x2202 v0** on the SL2 (index 9).
  - **f2 ranges `[sensor, dir, page]`.** [OBS FUN_140cd04e0] [HW]
    - Each reply echoes 3 bytes, then carries 13 bytes of a BE16 stream. **Values straddle pages**, so pages are concatenated.
    - `0xE0xx` = step xx (low byte); `0x0000` ends the list.
    - SL2: 100–200/1, 200–500/2, 500–1000/5, 1000–2000/10, 2000–5000/20, 5000–10000/50, 10000–20000/100, 20000–32000/125, 32000–44000/200.
    - Raw pages:
      ```
      11 01 09 23 00 00 00 00 64 E0 01 00 C8 E0 02 01 F4 E0 05 03
      11 01 09 23 00 00 01 E8 E0 0A 07 D0 E0 14 13 88 E0 32 27 10
      11 01 09 23 00 00 02 E0 64 4E 20 E0 7D 7D 00 E0 C8 AB E0 00
      11 01 09 23 00 00 03 00 00 …
      ```
  - **f5 get `[sensor]`** → **`[sensor, X, defaultX, Y, defaultY (BE16), lod]`**. [HW; byte positions OBS FUN_140cd0df0; field order corrected, see §3]
  - **f6 set `[sensor, xH, xL, yH, yL, lod]`**. We send X=Y=value and keep LOD. [OBS FUN_140cd0ff0] [HW cable + receiver]
    - Onboard mode: refused with **NotAllowed**.

### Report rate
- **0x8061 v0** on the SL2 (index 12). [OBS dec_all.c FUN_140cf3af0 / 140cf3c80 / 140cf3dd0 / 140cf3f60] [HW]
  - **`connType`: 0 = wired, 1 = wireless.** [HW]
    - f0 `[0]` → `00 0F`; f0 `[1]` → `00 7F`.
    - Events carry 00 over the cable and 01 over the receiver.
    - We pick connType 1 for devices behind a receiver and 0 for direct devices [INF; a direct Bluetooth device would need another rule].
  - **f0** GetDeviceCapabilities `[connType]` → reply[1] = rate bitmap for that connection type. Unused by the code.
  - **f1** GetActualReportRateList `[]` → reply[1] = bitmap for the **active** connection. Cable `0F` (125–1000 Hz); receiver `7F` (125–8000 Hz).
  - **f2** GetReportRate `[connType]` → reply[0] = bit number **of that connection type's rate**. [HW]
    - Over the receiver with wireless set to 500 Hz, f2 `[0]` → `03` (the wired setting) while f2 `[1]` → `02`.
    - So f2 must be asked for the active type. Before this fix, a read-back over the receiver falsely reported a mismatch.
  - **f3** SetReportRate `[bit]` sets the active connection's rate (in host mode). In onboard mode it is refused with **InvalidArgument**.
  - **Bit → Hz** = 125 << bit [PUB Solaar]. The real polling rate has not been measured (TEST REQUEST 4).

### Onboard profiles
- **0x8100** (SL2 index 13). [HW cable + receiver]
  - f2 get mode → `p[0]`: 1 = onboard [OBS §9.9], 2 = host [PUB].
  - f1 set `[1|2]`: G HUB sends `onboard ? 1 : 2` [OBS FUN_140d0e970].
  - Returning to onboard mode makes the device apply its profile DPI and rate again; it emits DPI and rate events.
- **Onboard memory, read-only.** [HW SL2 over the receiver, G HUB running]
  - **f0 GetOnboardProfilesInfo** → `[memModel, profileFormat, macroFormat, profileCount, factoryCount, buttonCount, sectorCount, sectorSize BE16, layout]`. [OBS FUN_140d0e7b0]
    - SL2: `01 07 01 05 01 05 10 00 FF 0A 04`. That is memory model 1, profile format 7, macro format 1, 5 user profiles, 1 factory profile, 5 buttons, **16 sectors of 255 B**, layout 0x0A (G-shift + DPI-shift by G HUB's masks). Trailing byte 0x04 [UNK].
  - **f4 GetActiveProfile** → BE16 sector. [OBS FUN_140d0eda0] SL2: **0x0101**, the factory profile.
  - **f5 ReadData `[sector BE16, offset BE16]`** → 16 bytes. [OBS FUN_140d0ef20]
    - A 255-byte sector is read at offsets 0, 16, …, 224 plus a final read at **239** (255 − 16), so nothing past the sector is requested [INF/PUB].
    - Over the receiver, 288 reads (18 sectors) took 3.7 s. Two consecutive dumps were byte-identical.
  - **f10 GetCRC `[sector BE16]`** → BE16 CRC, repeated (`B9 2A` × 8 for an erased sector). [OBS FUN_140d0fa30 sends the sector; reply layout HW]
    - **CRC-CCITT (init 0xFFFF, poly 0x1021) over sectorSize − 2 bytes = 0xB92A for 253 × FF**, matching the device. This confirms the CRC algorithm and coverage that a future profile write will need [PUB libratbag + HW].
    - ROM sectors are refused with InvalidArgument.
  - **Sector numbering.** User sectors are 0x0000..0x000F, with sector 0 the user directory; factory (ROM) sectors are 0x0100 + n, with 0x0100 the factory directory. [PUB libratbag]
    - [HW] the active profile is 0x0101, and the ROM directory starts `01 01 01 FF FF FF`: entry `[sector 0x0101, enabled 01, reserved FF]`, then the `FFFF` end marker.
  - **SL2 state.** All 16 user sectors are erased (all FF), so no user profile has ever been written and the mouse runs factory profile 0x0101. [HW]
  - **ROM sectors have no CRC.** No prefix length n of 0x0100 or 0x0101 has `crc(data[..n])` stored at n, and 0x0101 ends in `FF FF`. They are reported as "factory (ROM), no CRC". [HW]
  - **Factory profile contents (format 7), not decoded in code** [INF only]:
    - bytes 0–3 are `03 03 00 00`;
    - then five 5-byte entries that look like `[X LE16, Y LE16, LOD]`: 800, 1200, 1600, 2400, 3200 DPI, each with LOD 2;
    - byte 0 = 3 would match the 0x8061 rate bit for 1000 Hz, and 800 is the DPI the device reports in onboard mode.
    - Field meanings need [PUB] libratbag format docs plus hardware confirmation before any decode or write.

### Linux backend (2026-10-16, experimental)
All [INF]: written without Linux hardware. It compiles and passes clippy for
`x86_64-unknown-linux-gnu`, and the CI unit tests run on Ubuntu. Nothing has been checked against a real device.
- **Layout:** the per-OS code lives in `src/hid/{windows,linux}.rs`, `src/platform/*` and `src/autostart/*`, plus `gui/src/{os,tray}/*`. Everything else is shared.
- **Enumeration:** `/sys/class/hidraw/hidrawN/device` gives `uevent` (`HID_ID=bus:vid:pid`, `HID_NAME`) and `report_descriptor`.
  - `hid/descriptor.rs` turns the descriptor into one `Info` per top-level collection: page, usage, and the longest input/output report including the ID byte, the same numbers as Windows HidP_GetCaps. Tested with synthetic spec descriptors; on the receiver layout it gives FF00/1 = 7/7 and FF00/2 = 20/20, matching the [HW] Windows values.
  - All collections of a node share `path` (`/dev/hidrawN`). `iface` is the canonical sysfs device path.
- **One handle per node:** `Hidpp::open` routes each output length to one handle per distinct path. Opening a hidraw node twice would deliver each input report twice.
- **Receivers:** `hid-logitech-dj` adds virtual HID devices under the receiver's HID device. Nodes whose parent directory is itself a HID device (`0003:046D:C54D.0007/...`) are skipped, and devices are reached through the receiver by index, as Solaar does.
- **Waiting:** `poll(2)` on the hidraw fds plus an eventfd (`Waker`) and a `NETLINK_KOBJECT_UEVENT` socket (`Notifier`). `Signal::consume` gives auto-reset semantics.
  - Writes are synchronous in the kernel, so the write timeout only applies while a node is not writable.
- **Coexistence:** the kernel's `hid-logitech-hidpp` driver also talks HID++. Our software IDs differ (CLI 0x03, watchers 0x04), and replies are matched on the ID.
- **Access:** `/dev/hidraw*` is root-only by default. `packaging/linux/70-peripheral-lite.rules` adds `TAG+="uaccess"` for vendor 046d.
- **Paths:** `$XDG_CONFIG_HOME/peripheral-lite` (or `~/.config/peripheral-lite`). Instance locks are flock files in `$XDG_RUNTIME_DIR`. Autostart uses `~/.config/autostart/peripheral-lite.desktop`.
- **App:** eframe x11 + wayland. The tray is ksni (StatusNotifierItem over D-Bus, async-io runtime, no GTK); without an SNI host the app runs without a tray. A second launch sends "show" over a Unix socket. Skin file dialogs use zenity or kdialog if installed. App id `peripheral-lite` matches the desktop entry.

### Centurion headsets (PRO X 2 LIGHTSPEED, 0af7), 2026-10-16
- **[HW user report]** The PRO X 2 LIGHTSPEED headset's dongle (046d:0af7, "PRO X 2 LIGHTSPEED") exposes no HID++ collection. On interface 3 it has:
  - 000C/1 (2 B in), consumer keys;
  - FF13/1 (62/62 B);
  - FFA0/1 (64/64 B).
  So peripheral-lite listed nothing for it. The same report: PRO X SUPERLIGHT (wpid 4093) on receiver c547 works (HID++ 4.2; features 1004 v3, 2201 v2, 8060 v0, 8100), and its firmware names itself "PRO X Wireless".
- **[PUB HeadsetControl, GPL-3.0; facts only, no code]** These headsets speak "Centurion" on FFA0/1. The implementation is `src/centurion.rs`:
  - **Frames:** 64-byte frames with report ID 0x51: `[0x51, len, flags, payload]`.
  - **Features:** HID++-style Root (index 0, GetFeature = function 0 with the feature ID) and FeatureSet. Software ID 1.
  - **Bridge:** the headset sits behind a dongle feature 0x0003. Bridged payload: `[bridge, 0x11, size hi, size lo, 0, sub index, function|swid, params]`, length = payload + 1. The dongle ACKs first (swid nibble 1); the answer follows as an event: byte 7 status, byte 8 sub index (0xFF = rejected), data from byte 10.
  - **Direct requests** use length byte 0x41.
  - **Battery:** feature 0x0104 function 0 gives data `[percent, _, state]`; state 1 or 2 means charging.
  - **Other features named:** 0x0108 auto-sleep, 0x0604 sidetone, 0x0636 onboard EQ, 0x020D parametric EQ.
- **Status:**
  - **[HW user report, v0.2.2, 15:40]** Confirmed: bridge index 3; headset features 0x0104 battery at index 4, 0x0108 auto-sleep 6, 0x0604 sidetone 7, 0x0636 onboard EQ 9.
  - Absent features (0x0601, 0x060B, 0x020D) are not answered with index 0. The headset rejects them: `51 0A 00 03 10 00 05 00 FF 00 01 05` (sub index 0xFF, then index, function, code 0x05). peripheral-lite reports these as "not present".
  - The battery read gave `93% discharging`.
  - Since v0.2.3 the watcher and the app read it on the 5-minute poll. A failed read reports the headset offline until it answers again. Commands other than Refresh answer "battery only".
  - Still read-only.
- **PRO X Wireless 0x1F20 NotAllowed = headset off** [HW 2026-10-16 ~15:20, confirmed by the owner]: every voltage read answered NotAllowed while Root/name still worked (the dongle answers for itself). Since v0.2.3 this shows as "headset off or out of range"; the watcher reports the headset offline until a reading succeeds again.

### Corsair headsets (HS80 RGB Wireless, 1b1c:0a73), 2026-10-17
- **[HW]** HS80 RGB Wireless: dongle 1b1c:0a73 ("CORSAIR HS80 RGB Wireless Gaming Receiver", firmware 5.9.130); headset 0a71 (firmware 5.8.48). The dongle's interface 3 has:
  - COL01/02: 0001/0006 (keyboard);
  - COL03/04: 000C/1 (consumer);
  - COL05: **FF42/1, 64/64 B** (commands);
  - COL06: **FF42/2, 64 B in only** (events);
  - COL07: FF58/1 64/64.

  No feature reports. The same PC's iCUE Commander Core XT (0c2a) also has FF42/1, but with 385-byte reports; `corsair::is_corsair` requires 64 bytes, so it is never touched.
- **[HW] Framing ("Bragi"):**
  - **Requests** use report ID 0x02 on FF42/1: `[0x02, target, command, property, value…]`. Report ID 0x00 is refused by Windows ("parameter is incorrect") because the interface has several collections, and 0x03 is "incorrect function".
  - **Targets:** 0x08 = dongle, 0x09 = headset.
  - **Commands:** 0x02 = get; 0x01 = set [PUB].
  - **Replies** on FF42/1 use report ID 0x01: `[0x01, target-8, command, status, value u32 LE]`. Status 0 = ok, 5 = unsupported property.
  - Replies do not echo the property, so `corsair.rs` drains queued input before every request.
- **[HW] Properties, read-only sweep 01–FF.** Each entry gives the target/property, the value read, and its meaning. *(confirmed)* marks a meaning confirmed by a human action; the other meanings are [PUB] names from OpenRGB and Rexarn's HS80 notes.
  - **Headset (target 0x09):**
    - 0F: 310 = battery 31.0 % (tenths) *(confirmed)*
    - 10: battery state: 1 charging, 2 discharging *(confirmed with the charger in/out)*; 3 full [PUB]
    - 53: battery mV, 3657 discharging, ~4055 on the charger *(confirmed)*
    - 02: brightness 330 of 1000
    - 03: mode 1 = hardware
    - 0D: sleep timer on = 1
    - 0E: sleep timeout ms = 900000
    - 12: PID 0a71
    - 13: firmware 05 08 30
    - A6: mic muted 0/1 *(confirmed)*
    - also answer, meaning unknown: 17, 3A, 40, 43, 5F, 73, 74, 92, 95, B5, B6
  - **Dongle (target 0x08):**
    - 01: 4
    - 03: 1
    - 11/12: VID/PID
    - 13: firmware
    - 36: 2 (link state? [INF])
    - also answer: 17, 43, 4B, 5F, 66, 67, 73, 74, 78
  - **Not on this firmware (status 5):** the sidetone properties 0x46/0x47/0xD1 that Rexarn's notes give for the newer HS80 (dongle 0a97). HeadsetControl's README says HS80 sidetone is a USB-audio mixer control, not HID. Not implemented yet.
- **[HW] Events** on FF42/2, report ID 0x03: `[0x03, target-8, 0x01, property u16 LE, value u32 LE]`.
  - Mic mute: `03 01 01 A6 00 01` / `…00 00`.
  - Charger in/out: `03 01 01 10 00 01` / `…00 02`.
  - **No event for headset off:** requests to target 0x09 simply time out while the dongle keeps answering. The watcher therefore polls every 30 s while the headset is off (5 min otherwise) and reports it offline in between.
- **Status (v0.3.0):**
  - Read-only. `devices`, `info 1b1c:0a73` and `battery 1b1c:0a73` work, and so does the report.
  - Watch/app: battery and charging (live via events), mic state (watch line), headset off/on.
  - Selector `1b1c:PID`, which `device::parse_selector` rejects, so it cannot collide with a Logitech PID.
  - All tests ran with iCUE closed.
  - **[HW 2026-10-17, owner-confirmed] Set:** `[0x02, target, 0x01, property u16 LE, value u32 LE]`, i.e. the property is two bytes, as in events (get frames just happen to work with a one-byte property because the next byte is 0). Reply `[0x01, target-8, 0x01, status, …]`; bytes 4.. of a set reply hold stale data, not the new value.
    - A set with the one-byte layout (value at byte 4) is refused with status 0x09.
    - With the right layout, writes to the headset in **hardware mode (prop 03 = 1) are refused with 0x09**.
    - After set 03 = 2 (**software mode**) they are accepted: brightness 1000 → 0 → 330 was read back each time, and the owner saw the LEDs follow. Set 03 = 1 restores hardware mode.
  - **[HW 2026-10-17] More sets** (software-mode wrapper, read back):
    - Auto-off timeout 0E: 15 → 10 → 15 min.
    - Timer switch 0D: 1 → 0 → 1.
    - Mic mute A6: **refused with status 5 even in software mode**, so it is read-only. The headset's boom or button decides.
    - Since v0.4.0: `lights`, `autooff` and `mic` (read) in the CLI; Lighting and Auto-off chips plus the mic state in the app's drawer. `WRITABLE` gates which properties may be set.
  - **[HW 2026-10-17] Timing:** a request to the dongle takes ~3 ms, one to the headset ~100 ms (90–110, over the air). So the number of headset requests is what makes the app slow. A light change used to cost 13 of them (~1.3 s): a refused try, a mode check, the software-mode window and a read-back, then a full re-read for the drawer.
    - Since v0.4.1, `Corsair` remembers that the headset needs software mode.
    - The watcher keeps the last battery/settings and only reads back what was set (~0.4 s).
    - The app shows the new value at once.
    - Setting events (mic, lighting, auto-off) patch the drawer directly (`Update::Headset`).
    - Lighting is just on (1000) / off (0) at the owner's request.
  - **[HW 2026-10-17, owner-confirmed] Mic LED:**
    - With brightness 0 and the headset back in hardware mode, only the logo goes dark. The mic LED stays lit (red when muted): in hardware mode the headset drives it itself.
    - In **software mode with brightness 0, every LED stays dark in both boom positions**. The headset kept software mode on its own for 30 s, with no keep-alive.
    - Since v0.4.2, "lights off" (`Corsair::set_lights`) leaves the headset in software mode. "On" sets brightness 1000 and returns to hardware mode. Other sets keep software mode when the headset is already in it.
    - The choice is kept in `corsair.txt` (`0a73 lights=off`). The watcher and the app apply it again when the headset comes back (switched on, dongle plugged in), because software mode probably does not survive a power cycle [INF; to check].
  - **[HW owner report 2026-10-17] No headset sounds in software mode:** with the lights off, the mute/unmute, low-battery, very-low-battery and power tones stop; they come back with "on" (hardware mode).
    - [PUB] The prompts are recordings the firmware plays itself, and only in hardware mode; in software mode a mute change is only sent as the A6 event (ToastKiste21/corsair-hs80-max-re, README "Mic Mute Notifications"; HS80 MAX firmware, same protocol). No property turns them back on. arthurrmp/hs80-macos-menu-bar (HS80 RGB Wireless) plays its own mute tones on the host outside hardware mode.
    - So peripheral-lite plays its own tones (`sound.rs`, generated sine chimes, no files) while the lights are off: mute/unmute on the A6 event, low battery at 15 % and very low at 5 % (once each on the way down, played again only after charging 3 points above the line, so a wobbling reading warns once; the HS80's own thresholds are unknown).
    - They play on the headset's own output, found by name: waveOut lists it as `Headset Earphone (CORSAIR HS80 ` [HW Windows 11], cut to 31 characters, so the part in brackets must begin the HID product name. Windows only so far.
    - The app and the watcher both see every event, so only the process holding the `sounds-<pid>` instance lock plays them.
    - The power tones are not replaced: at power-off the headset is already gone.
  - **[HW 2026-10-17] Two processes, one dongle:** while the app (downloaded CI build) was polling, another process's reads received the app's replies; every open handle gets every reply, and replies carry no property. Since v0.4.0 each request holds a shared lock ("corsair-PID": named mutex / flock), so peripheral-lite processes take turns. iCUE does not take it, so keep iCUE closed.
  - **[HW 2026-10-17, owner-confirmed]** `watch` printed mic mute/unmute and charger in/out at once, "headset off" within the 30 s quick poll, and the battery again after switching on. A first run showed no events at all: Windows refuses write access to the input-only FF42/2 collection, so `hid::Handle::open` now falls back to read-only for collections without output reports.

### Headset (PRO X Wireless, 0aba, HID++ 4.2 on FF43/0202)
Features: 0x1F20 at index 6, 0x8300 at 5, 0x8310 at 4. [HW]

The function layouts were read from G HUB's request code with Rizin, against the installed `lghub_agent.exe` (SHA-256 04c1f3e8…). Only single functions were disassembled; there was no whole-binary analysis. Evidence IDs and addresses are below.

- **0x1F20 ADCMeasurement.** RTTI vtable 0x141932838: f0 0x140cc4070, f1 0x140cc4210, f2 0x140cc4310, f3 0x140cc4430, f4 0x140cc4560; event handler 0x140cc46c0.
  - **f0 `[channel]`** (G HUB requires channel < 16) → `[mV BE16, flags]`. G HUB splits the flags into 4 bits (meanings [UNK]). The event has the same layout. [OBS]
    - [HW] `-> 11 FF 06 03 00`, `<- 11 FF 06 03 0F 4F 01` = **3919 mV**, flags 0x01, while G HUB showed 80 %, not charging. So bit 0 is not "charging".
  - f1 → one byte; [HW] `1E` (30), meaning [UNK].
  - f2 sets one byte, f3 returns 2 bytes, f4 writes `[01, x]`: notification/config [INF]. These are not used.
- **Battery percentage: user-supplied curve** (human decision, 2026-10-15).
  - **How it works.** peripheral-lite looks in the preset folder (`%APPDATA%\peripheral-lite\`, or the folder of `--config`) for a `*.xml` power model whose `<powermodel pid=…>` equals the device's model ID.
    - The user copies that file from their **own** G HUB install, e.g. `C:\ProgramData\LGHUB\depotsr476\pro_headset\pro_x_wireless_headset_battery.xml`.
    - It is **never bundled or committed** (`*.xml` is in `.gitignore`), and peripheral-lite never reads G HUB's install folder. Without the file, only the voltage is shown, with a hint.
  - **Format used** [OBS file + protocol.md §10.6]: the `<discharge>` rows `minutes, volts, mAh used` (inside CDATA), and `<blocks default_unit="milliamps">` `power="N"` currents.
  - **Percent** = remaining share of the final mAh, taking the first row (in time order) at or below the measured voltage, floored.
    - [HW + file] 3919 mV → 80.5 % (G HUB showed 80); 3903 mV → **78 % (G HUB 78)**.
    - The curve is non-linear (≈91 % at 4.0 V, 63 % at 3.8 V, 28 % at 3.7 V), which is why the earlier 3-point linear table was dropped.
  - **Hours left** = remaining mAh / summed block current (53 mA here). Matches G HUB's `mileage` (18.8 h vs 18.7 h) and `maxLifeSpan` (1271 / 53 = 23.98 h).
  - Real files have 1 mV wiggles, so only mAh must be non-decreasing and the voltage must fall overall.
  - **Charging is not handled yet.** The `<charging>` tables are ignored, and while charging the voltage reads high, so the % is overstated. The 0x1F20 flag bits that indicate charging are [UNK]; this needs a reading while plugged in (TEST REQUEST 9).
- **Battery percentage without G HUB: own measured curve** (human decision, 2026-10-16; `src/battery.rs`).
  - **Order of sources:** `curve-<model>.txt` (our own measurement) → a G HUB XML the user copied → a built-in measured table (empty until a measured curve is contributed) → a generic single-cell Li-ion table [PUB], always shown as `≈` and labelled "generic Li-ion estimate".
  - **Recording:** every 0x1F20 reading taken by the `Monitor` (attach, the 5-min poll, GUI refresh) is appended as `unix_time,mV,flags` to `voltage-<model>.csv` in the preset folder; over 2 MB it moves to `.csv.old`. The CLI one-shot `battery` does not record.
  - **Builder:** splits the recording into discharges at any rise of more than 60 mV over the running minimum (charging); takes the newest one that starts at ≥ 4000 mV, reaches ≤ 3550 mV and has been ended by charging (a discharge still running is never used: built early it would stop at the last reading and be counted twice once it really ends), with ≥ 1 h of use and ≥ 20 readings. Use time = sum of gaps ≤ 15 min (longer gaps = headset off or PC asleep). For each 10 mV step the remaining permille = use time still ahead when the voltage first reached that step / total use time; then made non-increasing. Runtime = total use time. This assumes roughly constant draw [INF], which is also how the G HUB file's rows are spaced (constant 53 mA).
  - **Refinement:** a newer discharge is averaged with the saved curve (per voltage step and runtime), so ageing is followed gradually. Triggered automatically on the charging rise, on attach after recording the first reading (the headset may have been charged while nothing ran), or by the GUI button / `curve <dev> build`.
  - [HW 2026-10-16] At 3850 mV: G HUB file 71 %, ~17.1 h; generic estimate ≈50 %. The generic table is resting-voltage data and the headset reads lower under load, so it under-reports by ~20 points mid-range. Not tuned against Logitech's numbers on purpose (that would be copying their curve); the measured curve is the fix.
  - The G HUB file shows this headset running down to 3.29 V before it stops (1312 min), so the 3550 mV "empty" threshold is reached in normal use.
- **0x1F20 f0 can answer NotAllowed (0x05)** [HW 2026-10-16 14:39]: `-> 11 FF 06 03 00`, `<- 11 FF FF 06 03 05`, seen once by `peripheral-lite report`. Battery reads at 12:23 and at 14:39 (a later run) worked. Cause [UNK]; one candidate is the headset charging. Together with the 0 mV / flags 0x00 answers seen while it reconnects, it suggests the ADC is not always available. The app shows such a read as "unreadable" and keeps its last value.
- **No unsolicited 0x1F20 events** in ~3.5 min of capture with G HUB running. Only G HUB's own reads (`11 FF 06 0D …`, swid 0x0D, about once a minute) were seen.
  - 0x1F20 f3 config read returns `01 01` [HW]. Its meaning is [INF]: possibly "reporting enabled", but no event arrived.
  - So `watch` reads the voltage itself every **5 minutes**. This is the only timer in the program, justified because battery changes over hours and G HUB itself polls every ~60 s. It prints only when the percentage (or, without a curve, the 10 mV step) changes.
- **0x8300 Sidetone.** Methods 0x140d23a70 / 0x140d23bb0 / 0x140d23d00.
  - **f0 → level `[0]`.** [HW: 0, G HUB gain 0]
  - **f1 `[level]` sets it.** [HW] `-> 11 FF 05 13 1E`, echo `1E`; read-back 30; restored 0.
  - f2 (mute statuses) → **InvalidFunctionId** on this headset [HW].
  - The event carries three values [OBS RTTI binder `void(uint,uint,uint)`]; not decoded.
  - Range 0–100 is [INF] from the agent's `Volume.gain`.
  - G HUB's agent kept reporting gain 0 after our write: it reports its own cached state.
- **0x8310 Equalizer.** RTTI vtable 0x141942908: f0 0x140d260e0, f1 0x140d26280, f2 0x140d264a0, f3 0x140d26650, f4 0x140d26850, f5 0x140d26960.
  - **f0** → `[count, dbRange, capabilities, dbMin i8, dbMax i8]`. [OBS log string] [HW `0A 0C 01 00 00` = 10 bands, ±12 dB]
  - **f1 `[first band]`** → `[echo, BE16 Hz…]`, 7 bands per reply. [HW: 32 64 125 250 500 1k 2k from band 0; 4k 8k 16k from band 7]
  - **f2** → one signed dB byte per band. [HW: all 0]
  - **f3 SetEQ** sends `[byte from caller, gain…]` [OBS]. The first byte's meaning is [UNK].
    - [HW] `[00, 03, 02, 00 …]` and `[00, 0 × 10]` were both refused with **InvalidArgument**; the device was unchanged.
    - G HUB never uses this hardware EQ on this headset: agent `/devices/list` reports `onboardEqSupported: false` (G HUB's EQ for it is software). **No write is implemented**; we are not guessing further.
  - f4/f5 (one-byte get/set) → f4 is **InvalidFunctionId** on this headset [HW].

## 3. Corrections to protocol.md
- **§9.9, 0x2202 f5 field order.** The reply is `[sensor, X, defaultX, Y, defaultY, lod]`. [HW]
- **§9.9, 0x2202 f2.** The range stream continues across pages at byte granularity (13 bytes per page). [HW + OBS]
- **§9.9, 0x2201/0x2202 step.** G HUB takes the step from the low byte of `0xE0xx` only. [OBS]
- **§9.9, 0x2202 f6 / 0x8061 f3.** Refused in onboard mode with NotAllowed and InvalidArgument respectively; both work in host mode. [HW]
- **§9.9, 0x8061.**
  - f0 `[connType]` returns the rate bitmap for that connection type (0 wired, 1 wireless).
  - f1 takes no parameter and returns the active connection's bitmap.
  - **f2's result is per connType**, not global. [HW]
- **§10.7, PRO X Wireless headset (0aba).** It has **no** 0x1000/0x1001/0x1004. Battery voltage comes from **0x1F20 ADCMeasurement** f0 `[0]` → `[mV BE16, flags]`. [HW + OBS]
- **§8 / headset EQ.** The headset exposes a 10-band ±12 dB hardware EQ (0x8310), but G HUB does not use it (`onboardEqSupported: false`); its headset EQ is software. [HW + OBS]
- **§9.10 vs. the headset.** 0aba speaks plain HID++ 2.0 (protocol 4.2) on `FF43/0202`, not Centurion framing. [HW]
- **New device** missing from G HUB 2023.1's plaintext DB: receiver **c54d** + PRO X SUPERLIGHT 2, wireless PID **0x40A9**, wired PID **0xC09B**. [HW]

## 4. Dependencies
| Crate | Why | Transitive |
|---|---|---|
| `windows-sys 0.61` (Foundation, HumanInterfaceDevice, DeviceAndDriverInstallation, Storage_FileSystem, System_IO, System_Threading, Security) | Raw Win32 bindings for SetupDi/HidD/HidP/CreateFileW/overlapped I/O/CM notifications. std has none of these. | +1 (`windows-link`) |

**Rejected `hidapi`:** it builds C via `cc`, hides share mode and overlapped timeout control, and has no hot-plug API.

No `clap`, `serde`, logging crates or async runtime. Dev-only research tool: `dev/agent.ps1` (PowerShell, GET-only agent client, not part of the crate).

## 5. Tests run
| Date | Device | Connection | Test | Result |
|---|---|---|---|---|
| 2026-10-15 | LIGHTSPEED receiver c54d | USB | reg 0x02, B5 pairing 0..5, ping | ✅ 1 connected; slot 0 = 0x40A9 mouse; slots 1–5 error 0x09 |
| 2026-10-15 | SL2 (0x40A9) | LIGHTSPEED | ping, name, type, firmware, feature set | ✅ |
| 2026-10-15 | SL2 | LIGHTSPEED | battery 0x1004 | ✅ read 13 % |
| 2026-10-15 | SL2 | LIGHTSPEED | DPI set 0x2202 f6 → 1600 (onboard mode) | ⛔ NotAllowed; unchanged |
| 2026-10-15 | SL2 (c09b) | USB cable | mode host → DPI 1600 → read-back → 800 → onboard | ✅ |
| 2026-10-15 | SL2 (c09b) | USB cable | human run of TEST REQUEST 2: host → 400 → 800 → onboard | ✅ read-backs matched; **cursor noticeably slower at 400, normal at 800 (human-confirmed)** |
| 2026-10-15 | SL2 (c09b) | USB cable | battery while charging | ✅ 28 % then 38 %, charging, wired |
| 2026-10-15 | SL2 (c09b) | USB cable | report rate: onboard set (refused), host → 500 → 1000 → onboard | ✅ |
| 2026-10-15 | SL2 | LIGHTSPEED | `watch` while another process ran host → DPI 1200 → 800 → onboard | ✅ DPI write read back over the receiver; `watch` printed both DPI events and the rate event on return to onboard |
| 2026-10-15 | SL2 | LIGHTSPEED | report rate with connType fix: host → 500 → 1000 → onboard | ✅ read-back 500 then 1000; wireless bitmap 125–8000 Hz; final state onboard, 1000 Hz |
| 2026-10-15 | SL2 | LIGHTSPEED | `watch` idle 30 s | ✅ 0 ms CPU, 1 thread, 5.6 MB working set |
| 2026-10-15 | SL2 + receiver | LIGHTSPEED / cable | human run of TEST REQUEST 5 with `watch`: mouse off/on, cable in/out, receiver out/in, idle sleep then move | ✅ 0x41 link lost/connected on power cycle and on sleep/wake; re-announced DPI/rate/battery events; receiver and cable removal (os error 1167) and arrival; slot ping before link-up answered 0x08 |
| 2026-10-15 | SL2 | LIGHTSPEED | presets: save 1200/500 (scratch file), reject dpi=50, `apply` twice, then onboard mode and `watch` start-up | ✅ apply: "host mode, dpi 1200, rate 500", then "(already)" on the second run; `watch` applied the same at start-up and printed the device's DPI/rate events; restored onboard, 800, 1000 Hz |
| 2026-10-15 | SL2 | LIGHTSPEED | onboard memory: `profiles`, two `dump`s, backup to %APPDATA% | ✅ 16 user sectors erased; factory directory 0x0100 → profile 0x0101 (active); device GetCRC = our CRC for erased sectors; dumps byte-identical; read-only throughout |
| 2026-10-15 | headset 0aba | USB (own dongle) | ping, name, type, firmware, features | ✅ firmware matches agent |
| 2026-10-15 | headset 0aba | USB (own dongle) | human copied the power-model XML into %APPDATA%\peripheral-lite; `battery 0aba` | ✅ percentage shown (human-confirmed); our test copy: 78 % / 18.8 h vs G HUB 78 % / 18.7 h; watch printed the 78 → 77 % drop from its 5-min read |
| 2026-10-15 | headset 0aba | USB (own dongle) | 0x1F20 battery, 0x8300 sidetone read + write 30 → read-back → 0, 0x8310 EQ read + write attempt | ✅ 3919 mV vs G HUB 80 %; sidetone write/read-back ok; EQ read ok; EQ write refused (InvalidArgument, unchanged) → write removed |
| 2026-10-17 | Corsair HS80 RGB Wireless (1b1c:0a73) | USB (own dongle) | read-only get sweep 01–FF on dongle and headset; `devices`, `info`, `battery`; `watch` with mic mute/unmute, charger in/out, headset off/on (human run, iCUE closed) | ✅ 31 % / 3657 mV discharging, state 1 on the charger; events for mic and charger shown at once; off/on noticed within 30 s |
| — | — | — | `cargo test --workspace`: 38 offline byte-exact tests (fixtures from §9 and the [HW] traces above) | ✅ |

All tests ran with G HUB running. G HUB does not manage the SL2.

## 6. Pending test requests

**TEST REQUEST 1: SL2 battery.** Readings so far: 13 → 28 → 38 % charging, then 49 % on the receiver.
- What to observe: are these plausible, compared with the mouse's own indicator or another tool?

**TEST REQUEST 3: physical DPI check over the receiver.** The read-back is already verified [HW].
- Commands, with `c54d:1`: `mode … host`, `dpi … 400`, `dpi … 800`, `mode … onboard`.
- What to observe: the cursor is slower at 400.

**TEST REQUEST 4: real polling rate (optional).**
- Commands: host mode, then `rate <dev> 125` and measure with a polling-rate checker, then `rate <dev> 1000` and measure, then onboard.
- Expected: about 125 Hz, then about 1000 Hz. This would turn the [PUB] bit → Hz mapping into [HW].

TEST REQUEST 5 (hot-plug and wireless connect/disconnect with `watch`) passed on 2026-10-15; see §5.

**TEST REQUEST 10: the app** (redesigned 2026-10-16 as the "Battery Tracker v3" widget).
- Run `peripheral-lite-gui.exe`. ◀ ▶ and the Devices list switch between **PRO X 2** and the headset; the display, gauge and history follow.
- PRO X 2, **Settings**: the drawer opens. **Use host**: mode becomes "Host — settings from this PC".
- DPI 1600, **Apply**: "now 1600", the cursor is faster. Pick another rate chip. `peripheral-lite presets` now shows that DPI and rate (saved automatically). Switch the mouse off and on: they come back.
- Restore: DPI 800 and your usual rate, then **Use onboard**: mode is "Onboard profile" and `peripheral-lite presets` no longer lists the mouse.
- Headset, **Settings**: the battery curve line, the recording tooltip on **Build from recording**, and **Open folder** work.
- **Alert at** + / −: the red line moves on the gauge and the history; the value is kept after a restart.
- **Skins** (click the peripheral-lite title, or tray menu **Skins…**): pick each built-in (Amber shows pixel digits); change a colour on a built-in: a "(mine)" skin appears and is selected; **Export current** saves a `.json`; **Load skin file…** that file back; restart: the chosen skin is kept.
- Title bar: drag moves the widget; the left round button minimises; the right one hides it to the tray. Left-click the tray icon: the widget returns. Tray menu **Quit**: the app exits.
- **Tray** button: `peripheral-lite autostart` shows the GUI entry; click again to remove it.
- Icons: the taskbar battery lights one cell per started 25 % of the lowest device (red at its alert level); the tray shows two bars (first two devices), checkerboard when one is off. Switch the headset off: its tray bar turns to the checkerboard. (Generated icons are byte-identical to the design's PNGs for 75 %, 50/64 %, 14 % low and offline; checked with `PERIPHERAL_LITE_ICON_DUMP=<dir> cargo test -p peripheral-lite-gui icons` and a Python compare.)

**TEST REQUEST 12: Linux.** Needs a Linux machine (a live USB stick is enough) and the receiver or headset.
- Install the udev rule (packaging/linux/INSTALL-linux.md), replug, then run `./peripheral-lite collections`. Expected: the receiver's FF00/1 (7/7) and FF00/2 (20/20) collections on one `/dev/hidrawN`, and the headset's FF43 collection. No separate node for the paired mouse.
- Run `./peripheral-lite devices`, `battery c54d:1`, `dpi c54d:1`, and `watch` with an unplug and replug. All should match Windows.
- Run `./peripheral-lite-gui`: both devices listed, a tray icon on KDE/XFCE (on GNOME with the AppIndicator extension), and Tray adds `~/.config/autostart/peripheral-lite.desktop`.

**TEST REQUEST 11: measured headset curve.** Needs one full discharge, no G HUB.
- Charge the headset fully, then use it with the app (or `peripheral-lite-bg`) running until it is nearly empty or switches off, then plug it in to charge with the app still running.
- Expected: the event log shows "battery curve updated from the recorded discharge"; `peripheral-lite curve 0aba` shows "your measured curve" and a full runtime (G HUB's file says ~24 h at its assumed draw).
- To compare: move the G HUB XML out of `%APPDATA%\peripheral-lite\` first, otherwise the measured curve still wins, and compare `battery 0aba` with and without it at a few charge levels.
- Please keep `curve-0aba.txt`: it can become the built-in curve for this headset.

**TEST REQUEST 7: headset sidetone is audible.** (Command removed.) ❌ Failed on 2026-10-15: set 50 (read-back 50), human heard nothing; restored 0. Possible causes [INF]: a different value scale; or the effective sidetone being the USB-audio mixer control that G HUB's audio side uses rather than HID++ 0x8300. Open. G HUB can stay open.
- Commands: put the headset on, then `peripheral-lite sidetone 0aba 50`, speak, then `peripheral-lite sidetone 0aba 0`.
- What to observe: at 50 you hear your own voice in the headset; at 0 you don't.
- Also report whether G HUB later puts it back to 0 by itself.

**TEST REQUEST 9: headset battery while charging.** Plug the headset in to charge, then run `peripheral-lite --verbose battery 0aba` and `powershell -File dev\calibrate.ps1` (G HUB running). Expected: report the flags value. If it differs from 0x01, that bit means charging, and charging-curve support can be added.

**TEST REQUEST 8: battery calibration** (now only a cross-check of the curve against G HUB; optional).
- Command: `powershell -File dev\calibrate.ps1` from the repo folder, with the headset **not charging**.
- When: run it at several charge levels, for example full, ~60 %, ~30 % and ~10 %.
- Each run appends one peripheral-lite vs G HUB comparison point.

**TEST REQUEST 6: presets applied on reconnect.** G HUB can stay open.
- Commands:
  1. `peripheral-lite preset c54d:1 dpi=1600 rate=1000` (or any values you like).
  2. Start `peripheral-lite watch` and leave it running.
  3. Switch the mouse off and on; separately, let it sleep and then move it.
- Expected: after each `slot 1: connected`, a line `preset applied: …`.
  - If `host mode` appears in that list after a power cycle, host mode does **not** survive a power cycle; that answers the open question in §7.
- Over the cable, use `c09b` once to confirm the same preset (model 40a9) is found there.
- Restore: `peripheral-lite preset c54d:1 clear`, then `peripheral-lite mode c54d:1 onboard`.

## 7. Open questions
- Does host mode (0x8100 = 2) persist across a power cycle or a cable unplug? [UNK]
- **0x8061:** the bit → Hz mapping is only [PUB] (TEST REQUEST 4). How should connType be chosen for direct Bluetooth devices? [UNK]
- **Headset:**
  - meanings of the 0x1F20 flag bits and of f1 = 0x1E [UNK];
  - the sidetone event values and the real maximum sidetone level [UNK];
  - the EQ SetEQ first byte [UNK]; it can only be learned from a device/app that actually uses the hardware EQ;
  - charging detection and the charging curve (TEST REQUEST 9);
  - whether the headset ever pushes 0x1F20 events once G HUB is gone (watch decodes them if it does).
- **B5 pairing reply:** bytes p[1], p[2], p[5..] not decoded.
- **Onboard profiles before any write** (high-risk; see the brief §8):
  - decode profile format 7 with evidence;
  - confirm GetCRC on a non-erased flash sector;
  - check what f10's repeated bytes mean;
  - find the write sequence (f6 begin, f7 16-byte write with BE16 result, f8 end; §9.9);
  - **get explicit human approval.**
  - The backup `%APPDATA%\peripheral-lite\onboard-backup-40a9-2026-10-15.txt` exists. Restoring from it is not implemented.
- **Factory directory sector 0x0100** contains more than the directory after the `FFFF` end marker (bytes such as `03 04 05 06 00 01 02 …`, `D0 07`, …). Their meaning is [UNK].
- **Unknown SL2 feature IDs:** 0x2251, 0x80E0, 0x1602, plus the engineering IDs 0x1803/0x1817/0x1830/0x1875/0x1861/0x1E02/0x1E22/0x1EB0/0x18B1.
- **`watch` limit:** it waits on at most 63 collections (the `WaitForMultipleObjects` limit). That is far beyond any realistic setup.
- **Agent `GET /routes`** (§14 #1, 2026-10-15): 457 route entries, 305 unique paths.
  - The 37 not in `agent_routes.txt` are `%s` templates instantiated with `dev00000000`.

## 8. Metrics (2026-10-15, release: opt-level "s", LTO, codegen-units 1, panic=abort, strip)
| Metric | Value |
|---|---|
| `peripheral-lite.exe` | **311,296 B** (measured battery curves, recording, `curve` command: +22.5 KB) |
| `peripheral-lite-bg.exe` | **249,856 B** (+20.5 KB, same cause) |
| `peripheral-lite-gui.exe` | **4,889,088 B** (eframe/glow + tray-icon; Battery Tracker v3 widget, skins and history: +167 KB; the design's fonts, trimmed: +325 KB) |
| Crates | core: 3 (`peripheral-lite`, `windows-sys`, `windows-link`), unchanged; GUI package: 122 |
| `devices` wall time | median 115 ms (10 runs; headset + receiver, includes HID++ round trips) |
| `watch` / `peripheral-lite-bg` idle | **0 ms CPU** over 30 s, 1 thread between events, ~5.6 MB working set; one 0x1F20 read per 5 min when a voltage-only headset is present |
| GUI idle | **0 ms CPU** over 20 s, both shown and hidden; 7 threads; ~181 MB working set / 211 MB private. Most of that is the OpenGL driver context, not our code; the README points users who want a minimal footprint to `peripheral-lite-bg` |

**Size history**, with the cause of each step:
- 184,832 B at the MVP (opt-level 3 was 201,728 B, so we keep "s");
- 191,488 B with `rate`;
- 203,264 B with `watch`;
- 244,736 B with presets: std file and path code;
- 251,904 B with onboard read;
- 259,072 B with headset features;
- about 309 KB with the battery curve in floats (not measured at the time; reconstructed);
- **392 KB** with autostart via `std::process::Command`, too much, so it was replaced by `CreateProcessW` + Toolhelp: 318 KB;
- floats replaced by fixed-point integers: **284 KB**;
- 288,768 B with the Monitor refactor;
- **311,296 B** with measured battery curves (2026-10-16).

The `autostart` command itself costs ~6 KB.
