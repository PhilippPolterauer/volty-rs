# Reddit announcement — Volty v0.4.0

**Subreddit:** r/vibecoding

## Title

I vibe-coded a Windows tray battery monitor in Rust — v0.4.0 adds history, stale detection, and an honest list of what still doesn't work

## Body

Hey r/vibecoding,

I built **Volty**, a small Windows system-tray battery monitor in Rust. It started
because I wanted one clean battery readout for my Logitech mouse without opening
Logitech's software. v0.4.0 just landed.

**What it does**

- HID++ battery monitoring for supported Logitech devices, generic HID battery
  support, and opt-in Bluetooth (one headset validated; BLE still unverified)
- Multi-threshold alerts at 20/10/5 by default, once per discharge cycle, with an
  optional repeat cooldown
- A local SQLite history database and a `volty history` command with table and
  CSV output
- Battery-glyph or numeric tray icon, with an optional pinned device
- Last-seen ages and `stale` markers, so a sleeping or disconnected mouse stops
  looking alive
- One standalone `.exe`, MIT licensed, no installer, no telemetry, no uploads

**The vibe-coding part**

I orchestrated coding agents with one git worktree per feature, then reviewed and
merged each branch into `main`. Protocol work is covered by synthetic and mocked
tests only — I don't have a shelf of hardware to test against. That constraint
shaped the release: anything I couldn't validate is labeled experimental in the
README and changelog instead of being sold as working.

The parts that worked well:

- One worktree per task kept agent changes isolated and made review diffs small.
- Writing the tests and expected behavior before delegating kept the agents from
  inventing APIs.
- Running fmt, the full workspace test suite, and Clippy with `-D warnings` after
  every merge caught most integration mistakes early.
- Keeping a separate agent for documentation made the "what is actually
  verified?" gap obvious.

The part that didn't get easier: review. Agents are fast at producing plausible
code and slow at knowing which hardware assumptions are safe. I still had to read
the protocol paths myself.

**What's still rough**

- Hardware validation is thin. The Logitech G502 X Superlight RGB is the only
  fully verified HID++ device.
- The new Battery Voltage (`0x1001`) fallback is implemented and replayed from a
  synthetic fixture, but it has not been tested on a real voltage-only device.
- Bluetooth BLE GATT is implemented but not hardware-validated.
- Windows 10/11 only. No installer, no Mac, no Linux.

**Feedback wanted**

If you have a Logitech device that only exposes HID++ Battery Voltage (`0x1001`),
or a BLE peripheral with a standard battery service, I'd love a hardware capture.
There's a **Hardware report** issue template, and captures are local files you can
inspect before attaching.

I'd also like to hear how other people structure agent-assisted work. Worktree
per task worked well for me, but review is still the bottleneck.

Repo: https://github.com/PhilippPolterauer/volty-rs
Release: https://github.com/PhilippPolterauer/volty-rs/releases/tag/v0.4.0

**Disclosure:** this is my hobby project. No monetization, MIT licensed, and I'm
not affiliated with Logitech.
