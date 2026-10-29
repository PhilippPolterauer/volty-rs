use crate::battery_query::{discover_battery_feature, read_battery, BatteryFeature, QUERY_TIMEOUT};
use crate::logitech_hidpp::{transact_io, ReportIo};
use crate::*;
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::sync::{LazyLock, Mutex, PoisonError};
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
    forget_absent_interfaces(&interfaces);
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

    /// Opens an already enumerated interface without selecting it again.
    pub fn open_for_interface(info: &HidInterfaceInfo) -> Result<Self, HidppError> {
        open_hidpp_session(info)
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

    /// Runs `f` with the session allocator while `self` is borrowed as the transport.
    fn with_software_ids<R>(
        &mut self,
        f: impl FnOnce(&mut Self, &mut SoftwareIdAllocator) -> R,
    ) -> R {
        let mut software_ids = std::mem::take(&mut self.software_ids);
        let result = f(self, &mut software_ids);
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

type FeatureKey = (CString, u8);

static BATTERY_FEATURES: LazyLock<Mutex<HashMap<FeatureKey, BatteryFeature>>> =
    LazyLock::new(Mutex::default);

fn cached_feature(key: &FeatureKey) -> Option<BatteryFeature> {
    let cache = BATTERY_FEATURES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.get(key).copied()
}

fn cache_feature(key: FeatureKey, feature: BatteryFeature) {
    let mut cache = BATTERY_FEATURES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.insert(key, feature);
}

fn forget_feature(key: &FeatureKey) {
    let mut cache = BATTERY_FEATURES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.remove(key);
}

/// Drops cached features of interfaces that are no longer present, so a
/// device replugged under the same path is rediscovered.
fn forget_absent_interfaces(interfaces: &[HidInterfaceInfo]) {
    let present: HashSet<&CStr> = interfaces.iter().map(|info| info.path.as_c_str()).collect();
    let mut cache = BATTERY_FEATURES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.retain(|(path, _), _| present.contains(path.as_c_str()));
    drop(cache);
    #[cfg(windows)]
    crate::generic_battery::forget_absent_interfaces(interfaces);
}

fn query_battery_session(
    session: &mut HidppSession,
    device_index: u8,
) -> Result<BatteryStatus, HidppError> {
    let key = (session.info.path.clone(), device_index);
    if let Some(feature) = cached_feature(&key) {
        match session.with_software_ids(|transport, software_ids| {
            read_battery(transport, device_index, software_ids, feature)
        }) {
            Ok(battery) => return Ok(battery),
            // The device did not answer, which says nothing about its feature layout.
            Err(error @ (HidppError::Timeout | HidppError::HidApi(_))) => return Err(error),
            // A different device may now sit in this slot; discover it again.
            Err(_) => forget_feature(&key),
        }
    }
    let feature = session.with_software_ids(|transport, software_ids| {
        discover_battery_feature(transport, device_index, software_ids)
    })?;
    let battery = session.with_software_ids(|transport, software_ids| {
        read_battery(transport, device_index, software_ids, feature)
    })?;
    cache_feature(key, feature);
    Ok(battery)
}

fn query_device_session(
    session: &mut HidppSession,
    device_index: u8,
) -> Result<(Result<Option<String>, HidppError>, BatteryStatus), HidppError> {
    let feature = session.with_software_ids(|transport, software_ids| {
        discover_battery_feature(transport, device_index, software_ids)
    })?;
    let name = session.get_device_name(device_index, QUERY_TIMEOUT);
    let battery = session.with_software_ids(|transport, software_ids| {
        read_battery(transport, device_index, software_ids, feature)
    })?;
    cache_feature((session.info.path.clone(), device_index), feature);
    Ok((name, battery))
}
