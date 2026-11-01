use std::{error::Error, ptr, sync::mpsc, thread};

use tao::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::WindowExtWindows,
    window::WindowBuilder,
};
use wry::WebViewBuilder;

use crate::app_icon::{apply_to_window, tao_window_icon};

use super::{ipc, management::DashboardBackend};

enum UserEvent {
    IpcResponse(String),
}

pub(super) fn open_dashboard_window(
    html: &str,
    backend: DashboardBackend,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let response_proxy = event_loop.create_proxy();
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
            Event::UserEvent(UserEvent::IpcResponse(response)) => {
                let script = format!("window.__voltyDashboardResponse?.({response});");
                if let Err(error) = _webview.evaluate_script(&script) {
                    eprintln!("Could not deliver dashboard IPC response: {error}");
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    });
}
