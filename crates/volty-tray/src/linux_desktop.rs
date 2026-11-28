use crate::notify::{charging, notification_sample, NotificationEngine, NotificationError};
use dbus::arg::PropMap;
use dbus::arg::Variant;
use dbus::blocking::{stdintf::org_freedesktop_dbus::RequestNameReply, Connection};
use dbus::channel::{MatchingReceiver, Sender as _};
use dbus::message::MatchRule;
use dbus_crossroads::{Crossroads, MethodErr};

use std::collections::BTreeMap;
use std::error::Error;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use std::time::{Duration, Instant};
use volty_battery::{BatteryLevel, DeviceId, SourceReading};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const EXPIRE_AFTER: Duration = Duration::from_secs(90);

#[derive(Clone, Copy)]
enum Action {
    Dashboard,
    Refresh,
    Quit,
}

struct Device {
    reading: SourceReading,
    last_seen: Instant,
    stale: bool,
    notifications: NotificationEngine,
}

impl Device {
    fn summary(&self) -> String {
        let status = &self.reading.battery;
        let charge = match status.percentage {
            volty_battery::BatteryPercentage::Exact(value) => format!("{value}%"),
            volty_battery::BatteryPercentage::Estimated { value, .. } => format!("~{value}%"),
            volty_battery::BatteryPercentage::Unavailable => match status.level {
                BatteryLevel::Critical => "critical".into(),
                BatteryLevel::Low => "low".into(),
                BatteryLevel::Good => "good".into(),
                BatteryLevel::Full => "full".into(),
                BatteryLevel::Unknown => "unknown".into(),
            },
        };
        let state = if self.stale {
            " (stale)"
        } else if charging(status.state) {
            " (charging)"
        } else {
            ""
        };
        format!("{}: {charge}{state}", self.reading.label.display())
    }
}

const SERVICE: &str = "io.github.volty.Volty";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const MENU_PATH: &str = "/MenuBar";

type Layout = (i32, PropMap, Vec<Variant<Box<dyn dbus::arg::RefArg>>>);

struct Tray {
    actions: Vec<Action>,
    devices: Vec<String>,
    icon: &'static str,
    revision: u32,
}

impl Tray {
    fn menu_properties(&self, id: i32) -> Result<PropMap, MethodErr> {
        let mut properties = PropMap::new();
        match id {
            0 => {
                properties.insert(
                    "children-display".into(),
                    Variant(Box::new("submenu".to_owned())),
                );
            }
            1..=4 => {
                if let Some(label) =
                    ["Open dashboard", "Refresh now", "Quit"].get((id - 1) as usize)
                {
                    properties.insert("label".into(), Variant(Box::new(label.to_string())));
                } else {
                    properties.insert("type".into(), Variant(Box::new("separator".to_owned())));
                }
            }
            _ => {
                let label = id
                    .checked_sub(5)
                    .and_then(|index| self.devices.get(index as usize))
                    .ok_or_else(|| MethodErr::failed("Unknown menu item"))?;
                properties.insert("label".into(), Variant(Box::new(label.replace('_', "__"))));
                properties.insert("enabled".into(), Variant(Box::new(false)));
            }
        }
        Ok(properties)
    }

    fn menu_ids(&self) -> impl Iterator<Item = i32> + '_ {
        (5..5 + self.devices.len() as i32).chain([4, 1, 2, 3])
    }

    fn layout(&self, id: i32, depth: i32, names: &[String]) -> Result<Layout, MethodErr> {
        let mut properties = self.menu_properties(id)?;
        properties.retain(|name, _| names.is_empty() || names.contains(name));
        let children = if id == 0 && depth != 0 {
            self.menu_ids()
                .map(|id| {
                    self.layout(id, 0, names)
                        .map(|item| Variant(Box::new(item) as Box<dyn dbus::arg::RefArg>))
                })
                .collect::<Result<_, _>>()?
        } else {
            Vec::new()
        };
        Ok((id, properties, children))
    }

    fn click(&mut self, id: i32, event: &str) {
        if event == "clicked" {
            if let Some(action) = id.checked_sub(1).and_then(|index| {
                [Action::Dashboard, Action::Refresh, Action::Quit].get(index as usize)
            }) {
                self.actions.push(*action);
            }
        }
    }
}

fn export_tray(session: &Connection, tray: Arc<Mutex<Tray>>) {
    type State = Arc<Mutex<Tray>>;
    type Event = (i32, String, Variant<Box<dyn dbus::arg::RefArg>>, u32);
    type Pixmap = (i32, i32, Vec<u8>);
    let mut server = Crossroads::new();
    let item = server.register(ITEM, |interface| {
        interface
            .property("Id")
            .get(|_, _: &mut State| Ok("volty".to_owned()));
        interface
            .property("Title")
            .get(|_, _: &mut State| Ok("Volty".to_owned()));
        interface
            .property("Category")
            .get(|_, _: &mut State| Ok("Hardware".to_owned()));
        interface
            .property("Status")
            .get(|_, _: &mut State| Ok("Active".to_owned()));
        interface
            .property("WindowId")
            .get(|_, _: &mut State| Ok(0_i32));
        interface
            .property("ItemIsMenu")
            .get(|_, _: &mut State| Ok(false));
        interface
            .property("Menu")
            .get(|_, _: &mut State| Ok(dbus::Path::from(MENU_PATH)));
        interface
            .property("IconName")
            .get(|_, tray: &mut State| Ok(tray.lock().unwrap().icon.to_owned()));
        interface
            .property("IconPixmap")
            .get(|_, _: &mut State| Ok(Vec::<Pixmap>::new()));
        for name in [
            "IconThemePath",
            "OverlayIconName",
            "AttentionIconName",
            "AttentionMovieName",
        ] {
            interface
                .property(name)
                .get(|_, _: &mut State| Ok(String::new()));
        }
        for name in ["OverlayIconPixmap", "AttentionIconPixmap"] {
            interface
                .property(name)
                .get(|_, _: &mut State| Ok(Vec::<Pixmap>::new()));
        }
        interface.property("ToolTip").get(|_, tray: &mut State| {
            let description = tray
                .lock()
                .unwrap()
                .devices
                .join("\n")
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            Ok((
                String::new(),
                Vec::<Pixmap>::new(),
                "Volty".to_owned(),
                description,
            ))
        });
        interface.signal::<(), _>("NewIcon", ());
        interface.signal::<(), _>("NewToolTip", ());
        interface.method(
            "Activate",
            ("x", "y"),
            (),
            |_, tray: &mut State, _: (i32, i32)| {
                tray.lock().unwrap().actions.push(Action::Dashboard);
                Ok(())
            },
        );
        for method in ["ContextMenu", "SecondaryActivate"] {
            interface.method(method, ("x", "y"), (), |_, _: &mut State, _: (i32, i32)| {
                Ok(())
            });
        }
        interface.method(
            "Scroll",
            ("delta", "orientation"),
            (),
            |_, _: &mut State, _: (i32, String)| Ok(()),
        );
    });
    let menu = server.register(MENU, |interface| {
        interface
            .property("Version")
            .get(|_, _: &mut State| Ok(3_u32));
        interface
            .property("TextDirection")
            .get(|_, _: &mut State| Ok("ltr".to_owned()));
        interface
            .property("Status")
            .get(|_, _: &mut State| Ok("normal".to_owned()));
        interface
            .property("IconThemePath")
            .get(|_, _: &mut State| Ok(Vec::<String>::new()));
        interface.signal::<(u32, i32), _>("LayoutUpdated", ("revision", "parent"));
        interface.method(
            "GetLayout",
            ("parentId", "recursionDepth", "propertyNames"),
            ("revision", "layout"),
            |_, tray: &mut State, (id, depth, names): (i32, i32, Vec<String>)| {
                let tray = tray.lock().unwrap();
                Ok((tray.revision, tray.layout(id, depth, &names)?))
            },
        );
        interface.method(
            "GetGroupProperties",
            ("ids", "propertyNames"),
            ("properties",),
            |_, tray: &mut State, (mut ids, names): (Vec<i32>, Vec<String>)| {
                let tray = tray.lock().unwrap();
                if ids.is_empty() {
                    ids.extend(std::iter::once(0).chain(tray.menu_ids()));
                }
                let properties = ids
                    .into_iter()
                    .filter_map(|id| {
                        let mut properties = tray.menu_properties(id).ok()?;
                        properties.retain(|name, _| names.is_empty() || names.contains(name));
                        Some((id, properties))
                    })
                    .collect::<Vec<_>>();
                Ok((properties,))
            },
        );
        interface.method(
            "GetProperty",
            ("id", "name"),
            ("value",),
            |_, tray: &mut State, (id, name): (i32, String)| {
                let value = tray
                    .lock()
                    .unwrap()
                    .menu_properties(id)?
                    .remove(&name)
                    .ok_or_else(|| MethodErr::failed("Unknown menu property"))?;
                Ok((value,))
            },
        );
        interface.method(
            "Event",
            ("id", "eventId", "data", "timestamp"),
            (),
            |_, tray: &mut State, (id, event, _, _): Event| {
                tray.lock().unwrap().click(id, &event);
                Ok(())
            },
        );
        interface.method(
            "EventGroup",
            ("events",),
            ("idErrors",),
            |_, tray: &mut State, (events,): (Vec<Event>,)| {
                let mut tray = tray.lock().unwrap();
                let mut errors = Vec::new();
                for (id, event, _, _) in events {
                    if tray.menu_properties(id).is_ok() {
                        tray.click(id, &event);
                    } else {
                        errors.push(id);
                    }
                }
                Ok((errors,))
            },
        );
        interface.method(
            "AboutToShow",
            ("id",),
            ("needUpdate",),
            |_, _: &mut State, _: (i32,)| Ok((false,)),
        );
        interface.method(
            "AboutToShowGroup",
            ("ids",),
            ("updatesNeeded", "idErrors"),
            |_, _: &mut State, _: (Vec<i32>,)| Ok((Vec::<i32>::new(), Vec::<i32>::new())),
        );
    });
    server.insert(ITEM_PATH, &[item], tray.clone());
    server.insert(MENU_PATH, &[menu], tray);
    session.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |message, conn| {
            let _ = server.handle_message(message, conn);
            true
        }),
    );
}

fn register_tray(session: &Connection) {
    let message = dbus::Message::new_method_call(
        WATCHER,
        "/StatusNotifierWatcher",
        WATCHER,
        "RegisterStatusNotifierItem",
    )
    .expect("valid watcher method")
    .append1(SERVICE);
    // The watcher can query the item as part of registration.
    let _ = session.send(message);
}

fn emit(
    session: &Connection,
    path: &str,
    interface: &str,
    member: &str,
    args: impl dbus::arg::AppendAll,
) {
    let mut signal = dbus::Message::new_signal(path, interface, member).expect("valid tray signal");
    signal.append_all(args);
    let _ = session.send(signal);
}
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    let session = Connection::new_session()
        .map_err(|err| format!("Volty tray needs a desktop session bus: {err}"))?;
    if session.request_name(SERVICE, false, false, true)? != RequestNameReply::PrimaryOwner {
        eprintln!("Volty tray is already running in this session.");
        return Ok(());
    }

    let executable = std::env::current_exe()?;
    let tray = Arc::new(Mutex::new(Tray {
        actions: Vec::new(),
        devices: vec!["Checking batteries…".into()],
        icon: "battery-missing",
        revision: 0,
    }));
    export_tray(&session, tray.clone());
    let mut watcher_rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged");
    watcher_rule.sender = Some("org.freedesktop.DBus".into());
    watcher_rule.path = Some("/org/freedesktop/DBus".into());
    session.add_match(
        watcher_rule,
        |(name, _, owner): (String, String, String), session, _| {
            if name != WATCHER {
                return true;
            }
            if owner.is_empty() {
                eprintln!("Volty: system tray unavailable; battery monitoring continues.");
            } else {
                register_tray(session);
            }
            true
        },
    )?;
    let bus = session.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_secs(2),
    );
    let (watcher_present,): (bool,) =
        bus.method_call("org.freedesktop.DBus", "NameHasOwner", (WATCHER,))?;
    if watcher_present {
        register_tray(&session);
    } else {
        eprintln!("Volty: system tray unavailable; battery monitoring continues.");
    }
    let (request, requests) = std::sync::mpsc::sync_channel(1);
    let (result, results) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut monitor = crate::linux::Monitor::new();
        let mut devices = BTreeMap::new();
        let notifications = Connection::new_session();
        if let Err(err) = &notifications {
            eprintln!("Volty: could not connect to the desktop notification bus: {err}");
        }
        while requests.recv().is_ok() {
            refresh(&mut monitor, notifications.as_ref().ok(), &mut devices);
            let mut summaries = devices.values().map(Device::summary).collect::<Vec<_>>();
            if summaries.is_empty() {
                summaries.push("No batteries detected".into());
            }
            let icon = battery_icon(devices.values());
            if result.send((summaries, icon)).is_err() {
                break;
            }
        }
    });
    let mut dashboards: Vec<Child> = Vec::new();
    let mut next_poll = Instant::now();
    let mut polling = false;
    loop {
        match results.try_recv() {
            Ok((summaries, icon)) => {
                polling = false;
                let mut state = tray.lock().unwrap();
                if state.devices != summaries {
                    state.devices = summaries;
                    state.revision = state.revision.wrapping_add(1);
                    emit(
                        &session,
                        MENU_PATH,
                        MENU,
                        "LayoutUpdated",
                        (state.revision, 0_i32),
                    );
                    emit(&session, ITEM_PATH, ITEM, "NewToolTip", ());
                }
                if state.icon != icon {
                    state.icon = icon;
                    emit(&session, ITEM_PATH, ITEM, "NewIcon", ());
                }
                dashboards.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("Volty battery monitor stopped unexpectedly".into());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if !polling && Instant::now() >= next_poll {
            request.try_send(())?;
            polling = true;
            next_poll = Instant::now() + POLL_INTERVAL;
        }
        let wait = if polling {
            Duration::from_millis(100)
        } else {
            next_poll.saturating_duration_since(Instant::now())
        };
        session.process(wait)?;
        for action in std::mem::take(&mut tray.lock().unwrap().actions) {
            match action {
                Action::Refresh => next_poll = Instant::now(),
                Action::Dashboard => match Command::new(&executable)
                    .args(["dashboard", "--open"])
                    .stdin(Stdio::null())
                    .spawn()
                {
                    Ok(child) => dashboards.push(child),
                    Err(err) => eprintln!("Volty: could not open dashboard: {err}"),
                },
                Action::Quit => return Ok(()),
            }
        }
    }
}

fn refresh(
    monitor: &mut crate::linux::Monitor,
    session: Option<&Connection>,
    devices: &mut BTreeMap<DeviceId, Device>,
) {
    let result = monitor.poll();
    let now = Instant::now();
    devices.retain(|_, device| {
        device.stale = true;
        now.duration_since(device.last_seen) < EXPIRE_AFTER
    });
    let (readings, warnings) = match result {
        Ok(result) => result,
        Err(err) => {
            eprintln!("Volty: battery poll failed: {err}");
            return;
        }
    };
    for warning in warnings {
        eprintln!("Volty: {warning}");
    }
    if let Err(err) = crate::history::record_readings(&readings) {
        eprintln!("Volty: could not record battery history: {err}");
    }
    for reading in readings {
        let device = devices.entry(reading.id.clone()).or_insert_with(|| Device {
            reading: reading.clone(),
            last_seen: now,
            stale: false,
            notifications: NotificationEngine::default(),
        });
        device.reading = reading;
        device.last_seen = now;
        device.stale = false;
        if let Some(session) = session {
            notify(session, device, now);
        }
    }
}

fn notify(session: &Connection, device: &mut Device, now: Instant) {
    let Some(sample) = notification_sample(&device.reading.battery) else {
        return;
    };
    let body = device
        .summary()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let result =
        device
            .notifications
            .update_with(&device.reading.label.display(), sample, now, |_, _| {
                dispatch_notification(session, &body)
            });
    if let Err(err) = result {
        eprintln!("Volty: could not show low battery notification: {err}");
    }
}

fn dispatch_notification(session: &Connection, body: &str) -> Result<(), NotificationError> {
    session
        .with_proxy(
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            Duration::from_secs(2),
        )
        .method_call::<(u32,), _, _, _>(
            "org.freedesktop.Notifications",
            "Notify",
            (
                "Volty",
                0_u32,
                "battery-caution",
                "Low battery",
                body,
                Vec::<String>::new(),
                PropMap::new(),
                -1_i32,
            ),
        )
        .map(|_| ())
        .map_err(|err| NotificationError::new(err.to_string()))
}

fn battery_icon<'a>(devices: impl Iterator<Item = &'a Device>) -> &'static str {
    let Some(device) = devices.filter(|device| !device.stale).min_by_key(|device| {
        let status = &device.reading.battery;
        status.percentage.value().unwrap_or(match status.level {
            BatteryLevel::Critical => 5,
            BatteryLevel::Low => 20,
            BatteryLevel::Good => 50,
            BatteryLevel::Full => 100,
            BatteryLevel::Unknown => 101,
        })
    }) else {
        return "battery-missing";
    };
    let status = &device.reading.battery;
    if charging(status.state) {
        return "battery-good-charging";
    }
    match (status.percentage.value(), status.level) {
        (Some(0..=5), _) | (None, BatteryLevel::Critical) => "battery-empty",
        (Some(6..=20), _) | (None, BatteryLevel::Low) => "battery-caution",
        (Some(21..=40), _) => "battery-low",
        (Some(41..=89), _) | (None, BatteryLevel::Good) => "battery-good",
        (Some(_), _) | (None, BatteryLevel::Full) => "battery-full",
        (None, BatteryLevel::Unknown) => "battery-missing",
    }
}
