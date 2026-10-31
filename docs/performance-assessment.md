# Volty sustained-run performance assessment

**Assessment date:** 2026-10-26  
**Scope:** One observed Windows release process, sampled for 60.2 seconds, plus a source review of polling and Bluetooth discovery refreshed on 2026-10-31 for the event-driven tray loop. The 10-second polling assessment below is a source-based estimate, not a controlled device-hardware benchmark.

## Summary

The observed process had a small, stable footprint over the one-minute sample: working set varied by 0.05 MB, private bytes by 0.08 MB, and CPU time did not measurably increase at the sampling precision used. This is consistent with an idle tray application, but it is not a general benchmark or proof of long-term stability.

Source inspection shows that the device poller waits between scans rather than continuously looping. At a 10-second successful-poll interval, it can run up to six scans per minute when scans complete quickly, versus at most one every two minutes with the default interval. This increases per-device query activity, but does not imply a twelve-fold increase in total process CPU. Bluetooth is disabled in the default Cargo feature set. When enabled, its classic Bluetooth property, SetupAPI, and paired-device GATT phases share a 30-second overall scan budget, with individual Windows Runtime operations still capped at five seconds. Slow or unusually large device inventories may therefore return partial results with a budget warning.

## Observed process

The measurements below are from a process already running as `target/release/volty.exe`. Its configuration, connected devices, and compiled feature set were not inspected or changed, so the results should be treated as an observation of this machine and run only.

| Metric | Result |
| --- | --- |
| Sampling period | 60.2 seconds, 13 samples at approximately 5-second intervals |
| CPU time | No measurable increase; 0.000 seconds at 3-decimal-second display precision |
| Working set | 17.27-17.32 MB |
| Private bytes | 2.74-2.82 MB |
| Handles | 207-208 |
| Threads | 5-8 |

The process had been running for about 2 minutes 18 seconds at the end of sampling. The sample therefore crossed the default 120-second poll point if the default configuration was active, but the runtime configuration was not verified, so completion of a scheduled poll during the sample cannot be confirmed. CPU values below the measurement precision are also possible.

## Source-based runtime assessment

### Polling and idle behavior

- The default successful-poll wait is 120 seconds, with a configurable range of 10-3600 seconds; the default retry wait is 15 seconds. The worker waits after each query completes, so query duration adds to the start-to-start interval. ([config.rs](../crates/volty-tray/src/config.rs#L12), [app/poll.rs](../crates/volty-tray/src/app/poll.rs))
- Device discovery runs on a dedicated worker thread. The tray loop is event-driven: it blocks in `MsgWaitForMultipleObjects` and is woken by window messages, the poll worker, and the config watcher, so it uses no CPU while idle and processes queued work immediately. ([app.rs](../crates/volty-tray/src/app.rs), [wake.rs](../crates/volty-tray/src/app/wake.rs))
- The in-memory diagnostic log is capped at 200 entries. The file logger appends and flushes each accepted entry and rotates at 5 MiB, keeping one `.1` backup, so sustained trace logging is bounded to roughly 10 MiB of disk. ([app.rs](../crates/volty-tray/src/app.rs), [logging.rs](../crates/volty-tray/src/logging.rs))

### 10-second successful-poll scenario

Set `poll_interval_seconds = 10` to use this cadence; the application default remains 120 seconds. Because the worker waits after each completed query, the approximate scan start-to-start interval is `query duration + 10 seconds`. A quick successful query therefore yields about six scans per minute (roughly 12 times the default scan frequency). Failed queries use the retry interval instead; with the default 15-second retry, a fast failure results in about four attempts per minute.

Each scan repeats the same HID discovery and battery-query work, so average device activity and scan-related CPU/I/O will rise with scan frequency. The existing one-minute process sample did not use a verified 10-second configuration and cannot quantify that increase. Exact impact depends on connected devices and read latency; no device-specific CPU or battery-life claim is made here.

The UI loop no longer polls on a timer; it blocks until a window message, poll result, or config change arrives, so idle wakeups are zero and queued tray actions or completed readings are processed immediately. Device polling itself remains on its worker thread.

The battery ETA uses a per-device discharge history, defaulting to a 10-minute window (configurable from 1 to 180 minutes). With 10-second polling, this retains about 60 samples per device. The estimate needs a full window of usable readings, resets while charging or after a level increase, and requires no additional device I/O.

### Optional Bluetooth path

Bluetooth is not enabled by default in the tray crate. ([Cargo.toml](../crates/volty-tray/Cargo.toml))

When enabled, each device query performs classic Bluetooth property discovery, SetupAPI device-property discovery, and paired BLE discovery sequentially under one shared 30-second overall scan budget. Individual Windows Runtime operations still time out after five seconds. If the budget is exhausted, phases stop and return results gathered so far with a warning. The work is on the device-poller thread, not the tray UI thread, but it can delay the next battery update. ([budget.rs](../crates/volty-bluetooth/src/windows_backend/budget.rs#L5), [budget.rs](../crates/volty-bluetooth/src/windows_backend/budget.rs#L6), [ble.rs](../crates/volty-bluetooth/src/windows_backend/ble.rs#L25))

## Conclusion and follow-up

No CPU hot loop or growing memory footprint was evident in this one-minute observation. The default polling design is event-driven: the tray loop blocks in `MsgWaitForMultipleObjects` while idle, and the device worker waits between completed scans instead of spinning. The main performance uncertainty is scan latency, especially with the experimental Bluetooth feature enabled; the observed process did not provide a controlled comparison of Bluetooth or hardware configurations.

A representative follow-up should record the effective configuration and feature set, then sample a clean run for at least 10 minutes with the relevant devices connected. Include per-scan duration, CPU time, working/private memory, and log-file size so the test covers multiple polls and any retry behavior.
