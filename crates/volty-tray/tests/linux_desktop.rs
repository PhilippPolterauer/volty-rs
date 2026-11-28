#![cfg(target_os = "linux")]

use std::{
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve_bluez(address: &str, path: &str) {
    let connection = dbus::blocking::Connection::new_address(address).unwrap();
    connection
        .request_name("org.bluez", false, false, true)
        .unwrap();
    let mut service = dbus_crossroads::Crossroads::new();
    let manager = service.object_manager::<()>();
    service.insert("/", &[manager], ());
    let headset = service.register("org.bluez.Device1", |interface| {
        interface
            .property("Connected")
            .get(|_, _: &mut ()| Ok(true));
        interface
            .property("Alias")
            .get(|_, _: &mut ()| Ok("Integration headset".to_owned()));
        interface
            .property("Address")
            .get(|_, _: &mut ()| Ok("02:00:00:00:00:01".to_owned()));
    });
    let battery = service.register("org.bluez.Battery1", |interface| {
        interface
            .property("Percentage")
            .get(|_, _: &mut ()| Ok(63_u8));
    });
    service.insert(path.to_owned(), &[headset, battery], ());
    thread::spawn(move || {
        let _ = service.serve(&connection);
    });
}
fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn private_bus_reading_reaches_status_history_and_dashboard() {
    let dir = tempfile::tempdir().unwrap();
    let mut daemon = ChildGuard(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    let path = "/org/bluez/hci0/dev_02_00_00_00_00_01";
    serve_bluez(address, path);
    let volty = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_volty"));
        command
            .env("DBUS_SYSTEM_BUS_ADDRESS", address)
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .env("XDG_CONFIG_HOME", dir.path().join("config"))
            .env("XDG_CACHE_HOME", dir.path().join("cache"));
        command
    };

    let status = run(volty().arg("status"));
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status
            .lines()
            .any(|line| line.contains("Integration headset")
                && line.contains("63%")
                && line.contains("BlueZ Battery1")),
        "{status}"
    );

    let capture = dir.path().join("headset.volty.jsonl");
    run(volty()
        .args(["capture", "--strict", "--output"])
        .arg(&capture));
    let contents = std::fs::read_to_string(&capture).unwrap();
    assert!(contents.contains("org.bluez.Battery1.Percentage"));
    for secret in [path, "02:00:00:00:00:01", "Integration headset"] {
        assert!(!contents.contains(secret));
    }
    let replay = run(volty().args(["capture", "replay"]).arg(&capture));
    assert!(String::from_utf8_lossy(&replay.stdout)
        .contains("bluetooth: 1 evidence records, 1 readings matched"));

    let mut tray = ChildGuard(volty().arg("tray").stderr(Stdio::piped()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            tray.0.try_wait().unwrap().is_none(),
            "tray exited before recording history"
        );
        let history = run(volty().args(["history", "--csv"]));
        if String::from_utf8_lossy(&history.stdout).contains("Integration headset,63,") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tray did not record the BlueZ reading"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let duplicate = run(Command::new("timeout")
        .args(["--kill-after=1s", "5s", "env"])
        .args(volty().get_envs().map(|(key, value)| {
            let mut pair = key.to_os_string();
            pair.push("=");
            pair.push(value.unwrap());
            pair
        }))
        .arg(env!("CARGO_BIN_EXE_volty"))
        .arg("tray"));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already running"));

    let watcher = dbus::blocking::Connection::new_address(address).unwrap();
    let (registered, registrations) = std::sync::mpsc::channel();
    let mut service = dbus_crossroads::Crossroads::new();
    let iface = service.register("org.kde.StatusNotifierWatcher", |interface| {
        interface.method(
            "RegisterStatusNotifierItem",
            ("service",),
            (),
            |_, sender: &mut std::sync::mpsc::Sender<String>, (name,): (String,)| {
                sender.send(name).unwrap();
                Ok(())
            },
        );
    });
    service.insert("/StatusNotifierWatcher", &[iface], registered);
    watcher
        .request_name("org.kde.StatusNotifierWatcher", false, false, true)
        .unwrap();
    thread::spawn(move || {
        let _ = service.serve(&watcher);
    });
    let name = registrations.recv_timeout(Duration::from_secs(5)).unwrap();
    let client = dbus::blocking::Connection::new_address(address).unwrap();
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    let item = client.with_proxy(&name, "/StatusNotifierItem", Duration::from_secs(3));
    let icon: String = item.get("org.kde.StatusNotifierItem", "IconName").unwrap();
    assert_eq!(icon, "battery-good");
    let menu: dbus::Path<'static> = item.get("org.kde.StatusNotifierItem", "Menu").unwrap();
    let menu = client.with_proxy(&name, menu, Duration::from_secs(3));
    use dbus::arg::{PropMap, RefArg, Variant};
    type Layout = (i32, PropMap, Vec<Variant<Box<dyn RefArg>>>);
    let (_, layout): (u32, Layout) = menu
        .method_call(
            "com.canonical.dbusmenu",
            "GetLayout",
            (0_i32, 1_i32, Vec::<String>::new()),
        )
        .unwrap();
    assert!(format!("{layout:?}").contains("Integration headset: 63%"));
    menu.method_call::<(), _, _, _>(
        "com.canonical.dbusmenu",
        "Event",
        (3_i32, "clicked", Variant(0_i32), 0_u32),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = tray.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "tray did not quit");
        thread::sleep(Duration::from_millis(50));
    }
    run(volty().arg("dashboard"));
    let dashboard = dir.path().join("cache/volty/dashboard.html");
    let html = std::fs::read_to_string(&dashboard).unwrap();
    assert!(html.contains("Integration headset") && html.contains("\"latest_percent\":63"));
    assert_eq!(
        dashboard.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    run(volty().args(["autostart", "enable"]));
    let launcher = dir.path().join(Path::new("config/autostart/volty.desktop"));
    assert!(std::fs::read_to_string(&launcher)
        .unwrap()
        .contains("Exec=volty tray"));
    run(volty().args(["autostart", "disable"]));
    assert!(!launcher.exists());
}
