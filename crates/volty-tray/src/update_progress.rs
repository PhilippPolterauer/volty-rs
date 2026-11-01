use std::{
    io, mem, ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{
            InitCommonControlsEx, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX, PBM_SETMARQUEE,
            PBM_SETPOS, PBM_SETRANGE32, PBS_MARQUEE, PBS_SMOOTH,
        },
        WindowsAndMessaging::{
            CreateWindowExW, GetWindowLongPtrW, SendMessageW, SetTimer, SetWindowLongPtrW,
            GWL_STYLE, SW_SHOWNORMAL, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WM_TIMER, WS_CAPTION,
            WS_CHILD, WS_EX_TOPMOST, WS_OVERLAPPED, WS_VISIBLE,
        },
    },
};

use crate::{
    native_ui::{set_control_font, static_control_color, FontSet},
    win32,
};

const WINDOW_CLASS: &str = "VoltyUpdateProgressWindow";
const WINDOW_WIDTH: i32 = 420;
const WINDOW_HEIGHT: i32 = 132;
const LABEL_RECT: win32::ControlRect = win32::ControlRect {
    x: 20,
    y: 18,
    width: 364,
    height: 20,
};
const BAR_RECT: win32::ControlRect = win32::ControlRect {
    x: 20,
    y: 46,
    width: 364,
    height: 18,
};
const TIMER_ID: usize = 1;
const TIMER_INTERVAL_MS: u32 = 100;
const BAR_RESOLUTION: u64 = 1000;
const WM_CTLCOLORSTATIC: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_CTLCOLORSTATIC;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Preparing = 0,
    Downloading = 1,
    Verifying = 2,
    Restarting = 3,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Downloading,
            2 => Self::Verifying,
            3 => Self::Restarting,
            _ => Self::Preparing,
        }
    }
}

struct Shared {
    version: String,
    phase: AtomicU8,
    downloaded: AtomicU64,
    total: AtomicU64,
    done: AtomicBool,
}

/// Handle to a small always-on-top window that reports update progress.
/// The window runs on its own thread, so the update worker only touches atomics.
pub struct UpdateProgress {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl UpdateProgress {
    #[cfg(test)]
    pub(crate) fn without_window() -> Self {
        Self {
            shared: Arc::new(Shared {
                version: "test".to_owned(),
                phase: AtomicU8::new(Phase::Preparing as u8),
                downloaded: AtomicU64::new(0),
                total: AtomicU64::new(0),
                done: AtomicBool::new(false),
            }),
            thread: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> (Phase, u64, u64) {
        (
            Phase::from_u8(self.shared.phase.load(Ordering::Relaxed)),
            self.shared.downloaded.load(Ordering::Relaxed),
            self.shared.total.load(Ordering::Relaxed),
        )
    }

    /// Opens the window; failures are non-fatal and yield a handle that reports nothing.
    pub fn start(version: &str) -> Self {
        let shared = Arc::new(Shared {
            version: version.to_owned(),
            phase: AtomicU8::new(Phase::Preparing as u8),
            downloaded: AtomicU64::new(0),
            total: AtomicU64::new(0),
            done: AtomicBool::new(false),
        });
        let (ready_sender, ready_receiver) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("volty-update-progress".to_owned())
            .spawn(move || {
                if let Err(error) = run_window(&thread_shared, &ready_sender) {
                    eprintln!("Volty could not show the update progress window: {error}");
                }
                // Unblocks the starter if creation failed before signalling readiness.
                let _ = ready_sender.send(());
            })
            .map_err(|error| eprintln!("Volty could not start the update progress thread: {error}"))
            .ok();
        if thread.is_some() {
            let _ = ready_receiver.recv();
        }
        Self { shared, thread }
    }

    pub fn set_phase(&self, phase: Phase) {
        self.shared.phase.store(phase as u8, Ordering::Relaxed);
    }

    pub fn set_total(&self, total: u64) {
        self.shared.total.store(total, Ordering::Relaxed);
    }

    pub fn set_downloaded(&self, downloaded: u64) {
        self.shared.downloaded.store(downloaded, Ordering::Relaxed);
    }
}

impl Drop for UpdateProgress {
    fn drop(&mut self) {
        self.shared.done.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct WindowState {
    shared: Arc<Shared>,
    label: HWND,
    bar: HWND,
    marquee: bool,
    last_text: String,
    fonts: FontSet,
    create_complete: bool,
}

fn run_window(
    shared: &Arc<Shared>,
    ready: &mpsc::Sender<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let class_name = win32::to_wide(WINDOW_CLASS);
    let title = win32::to_wide("Updating Volty");
    // SAFETY: a null module name requests the current process executable's module handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    win32::register_window_class(&class_name, window_proc, instance)?;

    let controls = INITCOMMONCONTROLSEX {
        dwSize: mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_PROGRESS_CLASS,
    };
    // SAFETY: controls is a fully initialized structure that lives through the call.
    if unsafe { InitCommonControlsEx(ptr::addr_of!(controls)) } == 0 {
        return Err(io::Error::other("could not initialize the progress control").into());
    }

    let mut state = Box::new(WindowState {
        shared: Arc::clone(shared),
        label: ptr::null_mut(),
        bar: ptr::null_mut(),
        marquee: false,
        last_text: String::new(),
        fonts: FontSet::new()?,
        create_complete: false,
    });
    let (x, y) = center_on_primary_screen();
    // SAFETY: class/title are NUL-terminated and outlive creation; the Box-backed state pointer
    // stays valid until this function returns after the window is destroyed.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION,
            x,
            y,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::from_mut(state.as_mut()).cast(),
        )
    };
    if window.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    if let Err(error) = create_controls(window, instance, &mut state) {
        if let Err(destroy_error) = win32::destroy_window(window) {
            let _ = win32::clear_window_state(window);
            eprintln!("Volty could not destroy the incomplete progress window: {destroy_error}");
        }
        return Err(error.into());
    }
    refresh(&mut state);
    // SAFETY: window is live; the timer is destroyed together with it.
    if unsafe { SetTimer(window, TIMER_ID, TIMER_INTERVAL_MS, None) } == 0 {
        let error = io::Error::last_os_error();
        let _ = win32::destroy_window(window);
        return Err(error.into());
    }
    state.create_complete = true;
    let _ = ready.send(());
    win32::run_foreground_message_loop(window, SW_SHOWNORMAL, state, |state, value| {
        state.create_complete = value;
    })?;
    Ok(())
}

fn center_on_primary_screen() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    // SAFETY: GetSystemMetrics takes only a metric index.
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    ((width - WINDOW_WIDTH) / 2, (height - WINDOW_HEIGHT) / 2)
}

fn create_controls(window: HWND, instance: HINSTANCE, state: &mut WindowState) -> io::Result<()> {
    let empty = win32::to_wide("");
    state.label = win32::create_static_control(window, instance, &empty, WS_VISIBLE, LABEL_RECT)?;
    set_control_font(state.label, state.fonts.body_small);

    let class = win32::to_wide("msctls_progress32");
    state.bar = win32::create_control(
        window,
        instance,
        win32::ControlSpec {
            class: &class,
            text: &empty,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE | PBS_SMOOTH,
            x: BAR_RECT.x,
            y: BAR_RECT.y,
            width: BAR_RECT.width,
            height: BAR_RECT.height,
            id: 0,
        },
    )?;
    // SAFETY: bar is a live progress control; the range is a plain integer pair.
    unsafe { SendMessageW(state.bar, PBM_SETRANGE32, 0, BAR_RESOLUTION as LPARAM) };
    Ok(())
}

fn set_marquee(state: &mut WindowState, enabled: bool) {
    if state.marquee == enabled {
        return;
    }
    state.marquee = enabled;
    // SAFETY: bar is a live progress control owned by this thread; only the marquee style bit changes.
    unsafe {
        let style = GetWindowLongPtrW(state.bar, GWL_STYLE);
        let style = if enabled {
            style | PBS_MARQUEE as isize
        } else {
            style & !(PBS_MARQUEE as isize)
        };
        SetWindowLongPtrW(state.bar, GWL_STYLE, style);
        SendMessageW(state.bar, PBM_SETMARQUEE, usize::from(enabled), 30);
    }
}

fn refresh(state: &mut WindowState) {
    let shared = &state.shared;
    let phase = Phase::from_u8(shared.phase.load(Ordering::Relaxed));
    let downloaded = shared.downloaded.load(Ordering::Relaxed);
    let total = shared.total.load(Ordering::Relaxed);

    let text = match phase {
        Phase::Preparing => format!("Preparing Volty {} update...", shared.version),
        Phase::Downloading if total > 0 => format!(
            "Downloading Volty {}: {} of {} ({}%)",
            shared.version,
            format_mib(downloaded),
            format_mib(total),
            (downloaded.min(total) * 100 / total)
        ),
        Phase::Downloading => format!(
            "Downloading Volty {}: {}",
            shared.version,
            format_mib(downloaded)
        ),
        Phase::Verifying => "Verifying download...".to_owned(),
        Phase::Restarting => "Installing update. Volty will restart...".to_owned(),
    };

    let determinate = phase == Phase::Downloading && total > 0;
    set_marquee(state, !determinate);
    if determinate {
        let position = downloaded.min(total) * BAR_RESOLUTION / total;
        // SAFETY: bar is a live progress control; wParam is the new position.
        unsafe { SendMessageW(state.bar, PBM_SETPOS, position as WPARAM, 0) };
    }

    if text != state.last_text {
        if let Err(error) = win32::set_window_text(state.label, &win32::to_wide(&text)) {
            eprintln!("Volty could not update the update progress text: {error}");
        }
        state.last_text = text;
    }
}

fn format_mib(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE supplies this window's valid CREATESTRUCTW in lParam.
        if !unsafe { win32::attach_window_state(window, lparam) } {
            return 0;
        }
    }

    match message {
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        WM_TIMER => {
            let state_ptr = win32::window_state::<WindowState>(window);
            if !state_ptr.is_null() {
                // SAFETY: GWLP_USERDATA is the boxed state for this live window, used only on this thread.
                let state = unsafe { &mut *state_ptr };
                if state.shared.done.load(Ordering::Relaxed) {
                    if let Err(error) = win32::destroy_window(window) {
                        eprintln!("Volty could not close the update progress window: {error}");
                    }
                } else {
                    refresh(state);
                }
            }
            0
        }
        // The update cannot be cancelled once started.
        WM_CLOSE => 0,
        WM_DESTROY => {
            let state_ptr = win32::window_state::<WindowState>(window);
            if !state_ptr.is_null() {
                // SAFETY: GWLP_USERDATA remains the boxed state until the message loop exits.
                win32::quit_after_creation(unsafe { (*state_ptr).create_complete });
            }
            0
        }
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
}
