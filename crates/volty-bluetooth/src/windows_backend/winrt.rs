use std::{marker::PhantomData, rc::Rc, sync::mpsc, time::Duration};

use windows::core::RuntimeType;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

use super::budget::ScanBudget;
use crate::{error, BluetoothError};

thread_local! {
    /// Windows Runtime is initialized once per thread for the thread's lifetime.
    ///
    /// `windows` caches activation factories in process-wide statics that assume
    /// Windows Runtime stays initialized; tearing it down after every scan leaves
    /// those cached pointers stale and crashes on the next scan.
    static APARTMENT: WinRtApartment = WinRtApartment::initialize();
}

/// Initializes Windows Runtime on the current thread and keeps it initialized
/// until that thread exits.
pub(super) fn initialize() -> Result<(), BluetoothError> {
    APARTMENT.with(WinRtApartment::ensure_initialized)
}

struct WinRtApartment {
    initialization_error: Option<String>,
    _not_send: PhantomData<Rc<()>>,
}

impl WinRtApartment {
    fn initialize() -> Self {
        // SAFETY: RoInitialize affects this thread and the !Send guard balances
        // each successful initialization when the thread exits.
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Self {
                initialization_error: None,
                _not_send: PhantomData,
            },
            Err(initialization_error) => Self {
                initialization_error: Some(
                    windows_error("could not initialize Windows Runtime", initialization_error)
                        .to_string(),
                ),
                _not_send: PhantomData,
            },
        }
    }

    fn ensure_initialized(&self) -> Result<(), BluetoothError> {
        match &self.initialization_error {
            Some(initialization_error) => Err(error(initialization_error.clone())),
            None => Ok(()),
        }
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        if self.initialization_error.is_none() {
            // SAFETY: this !Send guard is dropped on the thread whose RoInitialize succeeded.
            unsafe { RoUninitialize() };
        }
    }
}

pub(super) fn await_operation<T>(
    operation: IAsyncOperation<T>,
    operation_name: &str,
    budget: &ScanBudget,
) -> Result<T, BluetoothError>
where
    T: RuntimeType + Clone + Send + 'static,
{
    let Some(timeout) = budget.operation_timeout() else {
        let _ = operation.Cancel();
        return Err(error(format!(
            "{operation_name} skipped because the shared Bluetooth scan budget was exhausted"
        )));
    };
    let result = await_operation_with_timeout(operation, operation_name, timeout);
    if result.is_err() && budget.is_exhausted() {
        return Err(error(format!(
            "{operation_name} timed out because the shared Bluetooth scan budget was exhausted"
        )));
    }
    result
}

fn await_operation_with_timeout<T>(
    operation: IAsyncOperation<T>,
    operation_name: &str,
    timeout: Duration,
) -> Result<T, BluetoothError>
where
    T: RuntimeType + Clone + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    operation
        .SetCompleted(&AsyncOperationCompletedHandler::new(move |completed, _| {
            let result = completed.ok()?.GetResults();
            let _ = sender.send(result);
            Ok(())
        }))
        .map_err(|error| windows_error("could not observe Windows Runtime operation", error))?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => {
            result.map_err(|error| windows_error(&format!("{operation_name} failed"), error))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = operation.Cancel();
            Err(error(format!("{operation_name} timed out")))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(error(format!(
            "{operation_name} completed without returning a result"
        ))),
    }
}

pub(super) fn windows_error(context: &str, windows_error: windows::core::Error) -> BluetoothError {
    error(format!(
        "{context} (HRESULT 0x{:08x})",
        windows_error.code().0 as u32
    ))
}
