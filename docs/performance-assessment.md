# Volty sustained-run performance assessment

**Assessment date:** 2026-10-26  
**Scope:** One observed Windows release process, sampled for 60.2 seconds, plus a source review of polling and Bluetooth discovery. The 10-second polling assessment below is a source-based estimate, not a controlled device-hardware benchmark.

## Summary

The observed process had a small, stable footprint over the one-minute sample: working set varied by 0.05 MB, private bytes by 0.08 MB, and CPU time did not measurably increase at the sampling precision used. This is consistent with an idle tray application, but it is not a general benchmark or proof of long-term stability.

Source inspection shows that the device poller waits between scans rather than continuously looping. At a 10-second successful-poll interval, it can run up to six scans per minute when scans complete quickly, versus at most one every two minutes with the default interval. This increases per-device query activity, but does not imply a twelve-fold increase in total process CPU. Bluetooth is disabled in the default Cargo feature set. When enabled, its paired-device GATT scan has a 30-second budget, but the earlier classic Bluetooth and SetupAPI scans have no single overall deadline. Those paths can dominate scan time on slow or unusually large device inventories.

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

- The default successful-poll wait is 120 seconds, with a configurable range of 10-3600 seconds; the default retry wait is 15 seconds. The worker waits after each query completes, so query duration adds to the start-to-start interval. ([config.rs](../crates/volty-tray/src/config.rs#L12), [app.rs](../crates/volty-tray/src/app.rs#L1336))
- Device discovery runs on a dedicated worker thread. The tray loop sleeps between checks using configurable `ui_poll_interval_ms` (50-1000 ms), defaulting to 250 ms rather than busy-spinning. This is four loop checks per second by default, with up to 250 ms additional delay before processing queued UI work. ([app.rs](../crates/volty-tray/src/app.rs#L1042), [config.rs](../crates/volty-tray/src/config.rs#L14))
- The in-memory diagnostic log is capped at 200 entries. The file logger appends and flushes each accepted entry without rotating the file. This is negligible for a single minute at normal log levels, but sustained trace logging can accumulate disk usage over a much longer run. ([app.rs](../crates/volty-tray/src/app.rs#L32), [logging.rs](../crates/volty-tray/src/logging.rs#L38))

### 10-second successful-poll scenario

Set `poll_interval_seconds = 10` to use this cadence; the application default remains 120 seconds. Because the worker waits after each completed query, the approximate scan start-to-start interval is `query duration + 10 seconds`. A quick successful query therefore yields about six scans per minute (roughly 12 times the default scan frequency). Failed queries use the retry interval instead; with the default 15-second retry, a fast failure results in about four attempts per minute.

Each scan repeats the same HID discovery and battery-query work, so average device activity and scan-related CPU/I/O will rise with scan frequency. The existing one-minute process sample did not use a verified 10-second configuration and cannot quantify that increase. Exact impact depends on connected devices and read latency; no device-specific CPU or battery-life claim is made here.

The UI loop's new 250 ms default reduces idle checks from about ten to four per second compared with the previous 100 ms value. The tradeoff is up to 250 ms before the loop processes queued tray actions or completed readings; device polling itself remains on its worker thread.

### Optional Bluetooth path

Bluetooth is not enabled by default in the tray crate. ([Cargo.toml](../crates/volty-tray/Cargo.toml))

When enabled, each device query performs classic Bluetooth property discovery, SetupAPI device-property discovery, and paired BLE discovery sequentially. Individual Windows Runtime operations time out after five seconds; the paired BLE/GATT scan has a 30-second total budget. The classic property/HFP and SetupAPI phases do not share that budget, so their total duration can scale with the number of devices and Windows response times. The work is on the device-poller thread, not the tray UI thread, but it can delay the next battery update. ([windows_backend.rs](../crates/volty-bluetooth/src/windows_backend.rs#L44), [windows_backend.rs](../crates/volty-bluetooth/src/windows_backend.rs#L45), [windows_backend.rs](../crates/volty-bluetooth/src/windows_backend.rs#L69), [windows_backend.rs](../crates/volty-bluetooth/src/windows_backend.rs#L519))

## Conclusion and follow-up

No CPU hot loop or growing memory footprint was evident in this one-minute observation. The default polling design is sleep-based and should remain low-activity between reads. The main performance uncertainty is scan latency, especially with the experimental Bluetooth feature enabled; the observed process did not provide a controlled comparison of Bluetooth or hardware configurations.

A representative follow-up should record the effective configuration and feature set, then sample a clean run for at least 10 minutes with the relevant devices connected. Include per-scan duration, CPU time, working/private memory, and log-file size so the test covers multiple polls and any retry behavior.
