use std::{
    error::Error,
    io, ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
};

use tao::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::WindowExtWindows,
    window::WindowBuilder,
};
use wry::WebViewBuilder;

use crate::app_icon::{apply_to_window, tao_window_icon};

use super::{ipc, management::DashboardBackend};

enum UserEvent {
    IpcResponse(String),
    Activate,
}

struct ActivationEvent(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: event handles are process-wide kernel objects that may be waited on from any thread.
unsafe impl Send for ActivationEvent {}

impl ActivationEvent {
    fn wait(&self, timeout_ms: u32) -> u32 {
        // SAFETY: this wrapper owns a valid event handle opened for synchronization.
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, timeout_ms) }
    }
}

impl Drop for ActivationEvent {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the valid handle returned by OpenEventW.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            eprintln!(
                "Could not close the dashboard activation listener: {}",
                io::Error::last_os_error()
            );
        }
    }
}

struct ActivationListener {
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ActivationListener {
    fn start(proxy: EventLoopProxy<UserEvent>) -> io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE},
        };

        let name = crate::win32::to_wide(crate::DASHBOARD_ACTIVATION_EVENT);
        // SAFETY: name is NUL-terminated and the named event grants synchronization access.
        let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        let event = ActivationEvent(event);

        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new()
            .name("volty-dashboard-activation".to_owned())
            .spawn(move || loop {
                if worker_stopped.load(Ordering::Acquire) {
                    break;
                }
                match event.wait(250) {
                    WAIT_OBJECT_0 => {
                        if proxy.send_event(UserEvent::Activate).is_err() {
                            break;
                        }
                    }
                    WAIT_TIMEOUT => {}
                    WAIT_FAILED => {
                        eprintln!(
                            "Could not wait for dashboard activation: {}",
                            io::Error::last_os_error()
                        );
                        break;
                    }
                    result => {
                        eprintln!("Unexpected dashboard activation wait result: {result}");
                        break;
                    }
                }
            });

        match worker {
            Ok(worker) => Ok(Self {
                stopped,
                worker: Some(worker),
            }),
            Err(error) => Err(error),
        }
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("The dashboard activation listener panicked");
            }
        }
    }
}

impl Drop for ActivationListener {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn open_dashboard_window(
    html: &str,
    backend: DashboardBackend,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let response_proxy = event_loop.create_proxy();
    let mut activation_listener = ActivationListener::start(response_proxy.clone())?;
    let (request_sender, request_receiver) = mpsc::channel::<String>();
    let _ipc_worker = thread::Builder::new()
        .name("volty-dashboard-ipc".to_owned())
        .spawn(move || {
            while let Ok(message) = request_receiver.recv() {
                let response = ipc::handle_message(&message, &backend);
                if response_proxy
                    .send_event(UserEvent::IpcResponse(response))
                    .is_err()
                {
                    break;
                }
            }
        })?;
    let window = WindowBuilder::new()
        .with_title("Volty Dashboard")
        .with_inner_size(PhysicalSize::new(1100, 760))
        .with_resizable(true)
        .with_window_icon(Some(tao_window_icon()?))
        .build(&event_loop)?;
    apply_to_window(ptr::with_exposed_provenance_mut(
        window.hwnd().cast_unsigned(),
    ))?;
    let _webview = WebViewBuilder::new()
        .with_html(html.to_owned())
        .with_ipc_handler(move |request| {
            let _ = request_sender.send(request.body().to_owned());
        })
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Activate) => {
                focus_window(&window);
                let script = "window.__voltyRefreshDashboard?.();";
                if let Err(error) = _webview.evaluate_script(script) {
                    eprintln!("Could not refresh the dashboard after activation: {error}");
                }
            }
            Event::UserEvent(UserEvent::IpcResponse(response)) => {
                let script = format!("window.__voltyDashboardResponse?.({response});");
                if let Err(error) = _webview.evaluate_script(&script) {
                    eprintln!("Could not deliver dashboard IPC response: {error}");
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                activation_listener.stop();
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

fn focus_window(window: &tao::window::Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let hwnd = ptr::with_exposed_provenance_mut(window.hwnd().cast_unsigned());
    // SAFETY: window.hwnd() is the live dashboard HWND owned by this event loop.
    unsafe { ShowWindow(hwnd, SW_RESTORE) };
    // SAFETY: window.hwnd() remains valid for the duration of this event callback.
    if unsafe { SetForegroundWindow(hwnd) } == 0 {
        eprintln!("Windows did not allow the dashboard to become the foreground window");
    }
}
