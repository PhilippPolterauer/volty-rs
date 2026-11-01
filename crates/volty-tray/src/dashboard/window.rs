use std::{error::Error, ptr};

use tao::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::windows::WindowExtWindows,
    window::WindowBuilder,
};
use wry::WebViewBuilder;

use crate::app_icon::{apply_to_window, tao_window_icon};

pub(super) fn open_dashboard_window(html: &str) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new();
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
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
        let _ = &_webview;
    });
}
