---
name: Device report
about: Tell us how peripheral-lite works with your Logitech device
title: "Device report: <device name>"
labels: device-report
---

**Device:** (e.g. G502 X LIGHTSPEED)
**Connection:** (USB cable / LIGHTSPEED or Unifying receiver / Bluetooth)
**peripheral-lite version:** (from the release name)
**What works, what does not:**

### Report

On Windows, double-click **`diagnose.cmd`** in the peripheral-lite folder; it writes
`peripheral-lite-report.txt` and opens it. On Linux, run `./peripheral-lite report > report.txt`.
Paste the whole file below, or attach it.

The report contains device names, IDs, the HID layout and, for steps that failed, the bytes
exchanged with the device. Nothing personal.

```text
paste here
```

Please do **not** use `raw` unless asked; sending arbitrary bytes to a device is risky.
