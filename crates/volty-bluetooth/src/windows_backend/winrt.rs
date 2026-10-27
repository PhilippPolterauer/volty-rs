use std::{sync::mpsc, time::Duration};

use windows::core::RuntimeType;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

use super::budget::ScanBudget;
use crate::{error, BluetoothError};

pub(super) struct WinRtApartment;

impl WinRtApartment {
    pub(super) fn initialize() -> Result<Self, BluetoothError> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| windows_error("could not initialize Windows Runtime", error))?;
        Ok(Self)
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
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
