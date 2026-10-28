use crate::logitech_hidpp::{transact_io, ReportIo};
use crate::*;
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// Generic metadata for a HID interface, excluding its path and serial number.
#[derive(Clone, Debug)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub release_number: u16,
    pub usage_page: Option<u16>,
    pub usage: Option<u16>,
    pub interface_number: i32,
    pub bus_type: String,
    pub manufacturer_string: Option<String>,
    pub product_string: Option<String>,
}

impl HidDeviceInfo {
    fn from_interface(info: &HidInterfaceInfo) -> Self {
        Self {
            vendor_id: info.vendor_id,
            product_id: info.product_id,
            release_number: info.release_number,
            usage_page: info.usage_page,
            usage: info.usage,
            interface_number: info.interface_number,
            bus_type: info.bus_type.clone(),
            manufacturer_string: info.manufacturer_string.clone(),
            product_string: info.product_string.clone(),
        }
    }
}

impl HidInterfaceInfo {
    fn from_hidapi(info: &DeviceInfo) -> Self {
        #[cfg(windows)]
        let (usage_page, usage) = (Some(info.usage_page()), Some(info.usage()));
        #[cfg(not(windows))]
        let (usage_page, usage) = (None, None);

        Self {
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            release_number: info.release_number(),
            usage_page,
            usage,
            interface_number: info.interface_number(),
            bus_type: format!("{:?}", info.bus_type()),
            manufacturer_string: info.manufacturer_string().map(str::to_owned),
            product_string: info.product_string().map(str::to_owned),
            serial_number: info.serial_number().map(str::to_owned),
            path: info.path().to_owned(),
        }
    }
}

static SHARED_API: Mutex<Option<HidApi>> = Mutex::new(None);

/// Runs `f` against a process-wide `HidApi`, so opening a device by path does not
/// repeat a full HID enumeration. `refresh` re-enumerates an existing context.
fn with_shared_api<T>(
    refresh: bool,
    f: impl FnOnce(&HidApi) -> Result<T, HidppError>,
) -> Result<T, HidppError> {
    let mut guard = SHARED_API.lock().unwrap_or_else(PoisonError::into_inner);
    match guard.as_mut() {
        Some(api) if refresh => api.refresh_devices().map_err(HidppError::HidApi)?,
        Some(_) => {}
        None => *guard = Some(HidApi::new().map_err(HidppError::HidApi)?),
    }
    match guard.as_ref() {
        Some(api) => f(api),
        None => Err(HidppError::UnexpectedResponse(
            "shared HID context is unavailable",
        )),
    }
}

/// Enumerate all visible HID interfaces in deterministic order.
pub fn enumerate_hid_interfaces() -> Result<Vec<HidInterfaceInfo>, HidppError> {
    let mut interfaces: Vec<_> = with_shared_api(true, |api| {
        Ok(api
            .device_list()
            .map(HidInterfaceInfo::from_hidapi)
            .collect())
    })?;
    interfaces.sort_by(|left, right| {
        (
            left.vendor_id,
            left.product_id,
            left.interface_number,
            left.usage_page,
            left.usage,
            &left.path,
        )
            .cmp(&(
                right.vendor_id,
                right.product_id,
                right.interface_number,
                right.usage_page,
                right.usage,
                &right.path,
            ))
    });
    Ok(interfaces)
}

/// Select exactly one enumerated interface; zero or multiple matches are errors.
pub fn select_hid_interface(
    selector: &HidInterfaceSelector,
) -> Result<HidInterfaceInfo, HidppError> {
    let interfaces = enumerate_hid_interfaces()?;
    select_hid_interface_from(&interfaces, selector)
}

/// A HID++ transport session bound to one explicitly selected interface.
pub struct HidppSession {
    device: HidDevice,
    info: HidInterfaceInfo,
    software_ids: SoftwareIdAllocator,
}

impl HidppSession {
    pub fn open(selector: &HidInterfaceSelector) -> Result<Self, HidppError> {
        let api = HidApi::new().map_err(HidppError::HidApi)?;
        let interfaces: Vec<_> = api
            .device_list()
            .map(HidInterfaceInfo::from_hidapi)
            .collect();
        let info = select_hid_interface_from(&interfaces, selector)?;
        let device = api.open_path(&info.path).map_err(HidppError::HidApi)?;
        Ok(Self {
            device,
            info,
            software_ids: SoftwareIdAllocator::new(),
        })
    }

    pub fn info(&self) -> &HidInterfaceInfo {
        &self.info
    }

    /// Look up a logical feature through Root while preserving its metadata.
    pub fn get_feature(
        &mut self,
        device_index: u8,
        logical_feature_id: u16,
        timeout: Duration,
    ) -> Result<Option<FeatureInfo>, HidppError> {
        let software_id = self.software_ids.allocate();
        get_feature(self, device_index, logical_feature_id, software_id, timeout)
    }

    pub fn get_device_name(
        &mut self,
        device_index: u8,
        timeout: Duration,
    ) -> Result<Option<String>, HidppError> {
        let mut software_ids = self.software_ids.clone();
        let result = query_device_name(self, device_index, &mut software_ids, timeout);
        self.software_ids = software_ids;
        result
    }

    /// Enumerate the device's advertised features via FeatureSet and Root.
    pub fn enumerate_features(
        &mut self,
        device_index: u8,
        timeout: Duration,
    ) -> Result<Vec<(u16, FeatureInfo)>, HidppError> {
        let mut software_ids = self.software_ids.clone();
        let result = enumerate_features(self, device_index, &mut software_ids, timeout);
        self.software_ids = software_ids;
        result
    }
}

impl ReportIo for HidDevice {
    fn write_output(&mut self, bytes: &[u8]) -> Result<usize, HidppError> {
        self.write(bytes).map_err(HidppError::HidApi)
    }

    fn read_timeout(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize, HidppError> {
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        if timeout_ms == 0 {
            return Ok(0);
        }
        HidDevice::read_timeout(self, buffer, timeout_ms).map_err(HidppError::HidApi)
    }
}

impl HidppTransport for HidppSession {
    fn transact(
        &mut self,
        request: &HidppFrame,
        timeout: Duration,
    ) -> Result<HidppFrame, HidppError> {
        transact_io(&mut self.device, request, timeout)
    }
}

/// Enumerate all HID interfaces visible to hidapi, regardless of vendor or bus.
pub fn enumerate_hid_devices() -> Result<Vec<HidDeviceInfo>, HidppError> {
    enumerate_hid_interfaces().map(|interfaces| {
        interfaces
            .iter()
            .map(HidDeviceInfo::from_interface)
            .collect()
    })
}

pub(crate) fn query_battery_interface(
    info: &HidInterfaceInfo,
    device_index: u8,
) -> Result<BatteryStatus, HidppError> {
    let mut session = open_hidpp_session(info)?;
    query_battery_session(&mut session, device_index)
}

pub(crate) fn query_device_interface(
    info: &HidInterfaceInfo,
    device_index: u8,
) -> Result<(Result<Option<String>, HidppError>, BatteryStatus), HidppError> {
    let mut session = open_hidpp_session(info)?;
    query_device_session(&mut session, device_index)
}

fn open_hidpp_session(info: &HidInterfaceInfo) -> Result<HidppSession, HidppError> {
    let device = with_shared_api(false, |api| {
        api.open_path(&info.path).map_err(HidppError::HidApi)
    })?;
    Ok(HidppSession {
        device,
        info: info.clone(),
        software_ids: SoftwareIdAllocator::new(),
    })
}

fn query_battery_session(
    session: &mut HidppSession,
    device_index: u8,
) -> Result<BatteryStatus, HidppError> {
    if let Some(feature) = session.get_feature(
        device_index,
        FEATURE_UNIFIED_BATTERY,
        Duration::from_secs(2),
    )? {
        return read_unified_battery(session, device_index, feature.feature_index);
    }
    if let Some(feature) =
        session.get_feature(device_index, FEATURE_BATTERY_STATUS, Duration::from_secs(2))?
    {
        return read_legacy_battery(session, device_index, feature.feature_index);
    }
    Err(HidppError::UnsupportedBatteryFeature)
}

fn query_device_session(
    session: &mut HidppSession,
    device_index: u8,
) -> Result<(Result<Option<String>, HidppError>, BatteryStatus), HidppError> {
    if let Some(feature) = session.get_feature(
        device_index,
        FEATURE_UNIFIED_BATTERY,
        Duration::from_secs(2),
    )? {
        let name = session.get_device_name(device_index, Duration::from_secs(2));
        let battery = read_unified_battery(session, device_index, feature.feature_index)?;
        return Ok((name, battery));
    }

    if let Some(feature) =
        session.get_feature(device_index, FEATURE_BATTERY_STATUS, Duration::from_secs(2))?
    {
        let name = session.get_device_name(device_index, Duration::from_secs(2));
        let battery = read_legacy_battery(session, device_index, feature.feature_index)?;
        return Ok((name, battery));
    }

    Err(HidppError::UnsupportedBatteryFeature)
}

fn read_unified_battery(
    session: &mut HidppSession,
    device_index: u8,
    feature_index: u8,
) -> Result<BatteryStatus, HidppError> {
    let capabilities = transact_feature(
        session,
        device_index,
        feature_index,
        FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES,
        Duration::from_secs(2),
    )?;
    let capabilities = parse_unified_battery_capabilities(&capabilities)?;
    let response = transact_feature(
        session,
        device_index,
        feature_index,
        FUNCTION_GET_UNIFIED_BATTERY_INFO,
        Duration::from_secs(2),
    )?;
    parse_unified_battery(&response, capabilities)
}

fn read_legacy_battery(
    session: &mut HidppSession,
    device_index: u8,
    feature_index: u8,
) -> Result<BatteryStatus, HidppError> {
    let response = transact_feature(
        session,
        device_index,
        feature_index,
        FUNCTION_GET_BATTERY_STATUS,
        Duration::from_secs(2),
    )?;
    parse_legacy_battery_status(&response)
}

fn transact_feature(
    session: &mut HidppSession,
    device_index: u8,
    feature_index: u8,
    function_id: u8,
    timeout: Duration,
) -> Result<HidppFrame, HidppError> {
    let request = HidppFrame::long(
        device_index,
        feature_index,
        function_id,
        session.software_ids.allocate(),
        [0; 16],
    );
    session.transact(&request, timeout)
}
