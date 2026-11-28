#![cfg(target_os = "linux")]

use dbus::{
    arg::{prop_cast, PropMap, RefArg, Variant},
    blocking::{
        stdintf::org_freedesktop_dbus::{Properties, RequestNameReply},
        Connection,
    },
    channel::{MatchingReceiver, Sender},
    message::MatchRule,
};
use dbus_crossroads::{Context, Crossroads, MethodErr};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const EXPECTED_ID_SUBSTRING: &str = "volty";
const ITEM_UNREGISTERED: &str = "StatusNotifierItemUnregistered";
const CALL_TIMEOUT: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

type TestResult<T> = Result<T, Box<dyn Error>>;
type Pixmap = (i32, i32, Vec<u8>);
type Layout = (i32, PropMap, Vec<Variant<Box<dyn RefArg>>>);

#[derive(Default)]
struct Registrations {
    items: BTreeSet<String>,
    hosts: BTreeSet<(String, String)>,
}

fn registration_address<'a>(
    context: &'a Context,
    service_or_path: &'a str,
    default_path: &'a str,
) -> Result<(dbus::strings::BusName<'a>, dbus::Path<'a>), MethodErr> {
    if service_or_path.starts_with('/') {
        let sender = context
            .message()
            .sender()
            .ok_or_else(|| MethodErr::failed("Registration has no message sender"))?;
        let path = dbus::Path::new(service_or_path).map_err(|err| MethodErr::failed(&err))?;
        Ok((sender, path))
    } else {
        let name =
            dbus::strings::BusName::new(service_or_path).map_err(|err| MethodErr::failed(&err))?;
        Ok((name, dbus::Path::from(default_path)))
    }
}

struct Watcher {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Watcher {
    fn start() -> TestResult<Self> {
        let connection = Connection::new_session()?;
        let registrations = Arc::new(Mutex::new(Registrations::default()));
        let mut server = Crossroads::new();
        let interface = server.register(WATCHER, |interface| {
            interface.signal::<(String,), _>("StatusNotifierItemRegistered", ("service",));
            interface.signal::<(String,), _>(ITEM_UNREGISTERED, ("service",));
            interface.signal::<(), _>("StatusNotifierHostRegistered", ());
            interface.property("RegisteredStatusNotifierItems").get(
                |_, state: &mut Arc<Mutex<Registrations>>| {
                    Ok(state
                        .lock()
                        .unwrap()
                        .items
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>())
                },
            );
            interface.property("IsStatusNotifierHostRegistered").get(
                |_, state: &mut Arc<Mutex<Registrations>>| {
                    Ok(!state.lock().unwrap().hosts.is_empty())
                },
            );
            interface
                .property("ProtocolVersion")
                .get(|_, _: &mut Arc<Mutex<Registrations>>| Ok(0_i32));
            interface.method(
                "RegisterStatusNotifierItem",
                ("service_or_path",),
                (),
                |context, state: &mut Arc<Mutex<Registrations>>, (value,): (String,)| {
                    let (name, path) =
                        registration_address(context, &value, "/StatusNotifierItem")?;
                    let address = format!("{name}{path}");
                    if state.lock().unwrap().items.insert(address.clone()) {
                        context.push_msg(
                            context.make_signal("StatusNotifierItemRegistered", (address,)),
                        );
                    }
                    Ok(())
                },
            );
            interface.method(
                "RegisterStatusNotifierHost",
                ("service_or_path",),
                (),
                |context, state: &mut Arc<Mutex<Registrations>>, (value,): (String,)| {
                    let (name, path) =
                        registration_address(context, &value, "/StatusNotifierHost")?;
                    if state
                        .lock()
                        .unwrap()
                        .hosts
                        .insert((name.to_string(), path.to_string()))
                    {
                        context.push_msg(context.make_signal("StatusNotifierHostRegistered", ()));
                    }
                    Ok(())
                },
            );
        });
        server.insert(WATCHER_PATH, &[interface], registrations.clone());
        connection.start_receive(
            MatchRule::new_method_call(),
            Box::new(move |message, conn| {
                server
                    .handle_message(message, conn)
                    .expect("watcher dispatch failed");
                true
            }),
        );

        let mut owners = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged");
        owners.sender = Some("org.freedesktop.DBus".into());
        owners.path = Some("/org/freedesktop/DBus".into());
        connection.add_match(
            owners,
            move |(name, old, _): (String, String, String), conn, _| {
                if !old.is_empty() {
                    let mut state = registrations.lock().unwrap();
                    let prefix = format!("{name}/");
                    state.items.retain(|address| {
                        if !address.starts_with(&prefix) {
                            return true;
                        }
                        let signal =
                            dbus::Message::new_signal(WATCHER_PATH, WATCHER, ITEM_UNREGISTERED)
                                .unwrap()
                                .append1(address.clone());
                        conn.send(signal).expect("send unregistration signal");
                        false
                    });
                    state.hosts.retain(|(service, _)| service != &name);
                }
                true
            },
        )?;
        if connection.request_name(WATCHER, false, false, true)? != RequestNameReply::PrimaryOwner {
            return Err("Could not own the watcher name on the test session bus".into());
        }

        let stop = Arc::new(AtomicBool::new(false));
        let should_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("mock-status-notifier-watcher".into())
            .spawn(move || {
                while !should_stop.load(Ordering::Relaxed) {
                    connection
                        .process(POLL_INTERVAL)
                        .expect("watcher bus disconnected");
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.expect("mock watcher thread panicked");
        }
    }
}

struct TrayProcess(Child);

impl Drop for TrayProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for<T>(
    description: &str,
    mut poll: impl FnMut() -> TestResult<Option<T>>,
) -> TestResult<T> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        if let Some(value) = poll()? {
            return Ok(value);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(
                format!("Timed out after {WAIT_TIMEOUT:?} waiting for {description}").into(),
            );
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

fn item_proxy<'a>(
    client: &'a Connection,
    address: &str,
) -> dbus::blocking::Proxy<'a, &'a Connection> {
    let (name, path) = address
        .split_once('/')
        .expect("watcher stores name/path addresses");
    client.with_proxy(name.to_owned(), format!("/{path}"), CALL_TIMEOUT)
}

#[test]
#[ignore = "run under xvfb-run and dbus-run-session, as in Linux CI"]
fn volty_registers_status_notifier_item_at_startup() {
    let display = std::env::var_os("DISPLAY").expect("run the test under xvfb-run");
    let bus_address =
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").expect("run the test under dbus-run-session");
    let dir = tempfile::tempdir().unwrap();
    let _watcher = Watcher::start().expect("start mock watcher before launching Volty");
    let client = Connection::new_session().unwrap();
    let watcher = client.with_proxy(WATCHER, WATCHER_PATH, CALL_TIMEOUT);
    let (signal_tx, signal_rx) = mpsc::channel();
    let mut rule = MatchRule::new_signal(WATCHER, ITEM_UNREGISTERED);
    rule.sender = Some(WATCHER.into());
    rule.path = Some(WATCHER_PATH.into());
    client
        .add_match(rule, move |(address,): (String,), _, _| {
            signal_tx.send(address).is_ok()
        })
        .unwrap();
    watcher
        .method_call::<(), _, _, _>(
            WATCHER,
            "RegisterStatusNotifierHost",
            (client.unique_name().to_string(),),
        )
        .unwrap();
    assert!(watcher
        .get::<bool>(WATCHER, "IsStatusNotifierHostRegistered")
        .unwrap());

    let mut tray = TrayProcess(
        Command::new(env!("CARGO_BIN_EXE_volty"))
            .arg("tray")
            .env("DISPLAY", display)
            .env("DBUS_SESSION_BUS_ADDRESS", &bus_address)
            .env("DBUS_SYSTEM_BUS_ADDRESS", &bus_address)
            .env("XDG_CONFIG_HOME", dir.path().join("config"))
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .env("XDG_CACHE_HOME", dir.path().join("cache"))
            .spawn()
            .expect("launch volty tray"),
    );

    let registered = || watcher.get::<Vec<String>>(WATCHER, "RegisteredStatusNotifierItems");
    let mut discovered = BTreeMap::new();
    let result = (|| -> TestResult<()> {
        let description =
            format!("a StatusNotifierItem address containing {EXPECTED_ID_SUBSTRING:?}");
        let (address, properties) = wait_for(&description, || {
            let mut matching = None;
            for address in registered()? {
                let properties = item_proxy(&client, &address).get_all(ITEM);
                discovered.insert(address.clone(), format!("{properties:?}"));
                if address.contains(EXPECTED_ID_SUBSTRING) && matching.is_none() {
                    matching = Some((address, properties?));
                }
            }
            if let Some(status) = tray.0.try_wait()? {
                return Err(format!(
                    "volty tray exited before registration was verified ({status})"
                )
                .into());
            }
            Ok(matching)
        })?;

        for name in ["Id", "Title", "Status"] {
            if !prop_cast::<String>(&properties, name).is_some_and(|value| !value.trim().is_empty())
            {
                return Err(format!("{address} has no non-empty {name}").into());
            }
        }
        let item = item_proxy(&client, &address);
        let has_icon_name = prop_cast::<String>(&properties, "IconName")
            .is_some_and(|value| !value.trim().is_empty());
        if !has_icon_name
            && !item
                .get::<Vec<Pixmap>>(ITEM, "IconPixmap")?
                .iter()
                .any(|(width, height, bytes)| *width > 0 && *height > 0 && !bytes.is_empty())
        {
            return Err(
                format!("{address} has neither an icon name nor a non-empty icon pixmap").into(),
            );
        }

        let menu_path: dbus::Path<'static> = item.get(ITEM, "Menu")?;
        let menu = client.with_proxy(item.destination.clone(), menu_path, CALL_TIMEOUT);
        let (_, (root, _, children)): (u32, Layout) =
            menu.method_call(MENU, "GetLayout", (0_i32, 1_i32, Vec::<String>::new()))?;
        if root != 0 || children.is_empty() {
            return Err(format!("{address} has no D-Bus menu items").into());
        }
        let (entries,): (Vec<(i32, PropMap)>,) = menu.method_call(
            MENU,
            "GetGroupProperties",
            (Vec::<i32>::new(), vec!["label"]),
        )?;
        let quit = entries
            .iter()
            .find(|(_, properties)| {
                prop_cast::<String>(properties, "label").is_some_and(|label| label == "Quit")
            })
            .map(|(id, _)| *id)
            .ok_or_else(|| format!("{address} has no Quit menu item"))?;
        menu.method_call::<(), _, _, _>(MENU, "Event", (quit, "clicked", Variant(0_i32), 0_u32))?;
        let status = wait_for("volty tray to quit via its D-Bus menu", || {
            Ok(tray.0.try_wait()?)
        })?;
        if !status.success() {
            return Err(format!("volty tray failed after Quit ({status})").into());
        }
        wait_for("StatusNotifierItemUnregistered", || {
            client.process(POLL_INTERVAL)?;
            Ok(signal_rx
                .try_iter()
                .any(|item| item == address)
                .then_some(()))
        })?;
        if registered()?.contains(&address) {
            return Err(format!("{address} remains registered after Quit").into());
        }
        Ok(())
    })();

    if let Err(err) = result {
        if let Ok(entries) = registered() {
            for address in entries {
                let properties = item_proxy(&client, &address).get_all(ITEM);
                discovered
                    .entry(address)
                    .or_insert_with(|| format!("{properties:?}"));
            }
        }
        panic!("{err}\nDiscovered StatusNotifierItem addresses and properties: {discovered:#?}");
    }
}
