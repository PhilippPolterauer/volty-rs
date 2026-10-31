//! Windows HID parser bindings that discover and read standard battery usages.
use super::{
    decode_captured, is_battery_button, is_battery_value, select_layout, BatteryLayout,
    ButtonField, GenericBatteryButtonReading, GenericBatteryValueReading, LevelSource,
    ProbedLayout, ReportKind, ValueField, PAGE_BATTERY_SYSTEM,
};
use crate::protocol::ProtocolError;
use crate::{BatteryStatus, HidInterfaceInfo};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::io;
use std::ptr;
use std::sync::{LazyLock, Mutex, PoisonError};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetFeature, HidD_GetInputReport, HidD_GetPreparsedData,
    HidP_Feature, HidP_GetButtonCaps, HidP_GetCaps, HidP_GetUsageValue, HidP_GetUsages,
    HidP_GetValueCaps, HidP_Input, HIDP_BUTTON_CAPS, HIDP_CAPS, HIDP_REPORT_TYPE,
    HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS, PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileA, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// Probe results per interface path; `None` records an interface without a usable battery.
static LAYOUTS: LazyLock<Mutex<HashMap<CString, Option<ProbedLayout>>>> =
    LazyLock::new(Mutex::default);

pub(super) fn cached_layout(path: &CStr) -> Option<BatteryLayout> {
    cached_probed_layout(path).map(|probed| probed.layout)
}

pub(super) fn cached_probed_layout(path: &CStr) -> Option<ProbedLayout> {
    let known = LAYOUTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        .cloned();
    if let Some(known) = known {
        return known;
    }
    let layout = probe(path);
    LAYOUTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.to_owned(), layout.clone());
    layout
}

/// Returns only an already cached probe result, without opening the device.
pub(super) fn known_probed_layout(path: &CStr) -> Option<ProbedLayout> {
    LAYOUTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        .cloned()
        .flatten()
}

pub(super) fn forget_absent_interfaces(interfaces: &[HidInterfaceInfo]) {
    let present: HashSet<&CStr> = interfaces.iter().map(|info| info.path.as_c_str()).collect();
    LAYOUTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|path, _| present.contains(path.as_c_str()));
}

pub(super) fn read_snapshot(
    path: &CStr,
    layout: &BatteryLayout,
) -> Result<
    (
        BatteryStatus,
        Vec<GenericBatteryValueReading>,
        Vec<GenericBatteryButtonReading>,
    ),
    ProtocolError,
> {
    read_with(&HidCollection::open(path)?, layout)
}

pub(super) fn read_battery(
    path: &CStr,
    layout: &BatteryLayout,
) -> Result<BatteryStatus, ProtocolError> {
    read_snapshot(path, layout).map(|(battery, _, _)| battery)
}

/// Accepts a layout only if the battery can actually be read, so a declared but
/// unreadable field never turns into a recurring poll error.
fn probe(path: &CStr) -> Option<ProbedLayout> {
    let device = HidCollection::open(path).ok()?;
    let (values, buttons) = device.battery_fields();
    let layout = select_layout(&values, &buttons)?;
    read_with(&device, &layout).ok()?;
    Some(ProbedLayout {
        values,
        buttons,
        layout,
    })
}

fn read_with(
    device: &HidCollection,
    layout: &BatteryLayout,
) -> Result<
    (
        BatteryStatus,
        Vec<GenericBatteryValueReading>,
        Vec<GenericBatteryButtonReading>,
    ),
    ProtocolError,
> {
    let mut readings = Vec::new();
    let mut buttons = Vec::new();
    let mut reports = Reports {
        device,
        fetched: Vec::new(),
    };
    match layout.level {
        LevelSource::Percent(field) => {
            let raw = reports.value(&field)?;
            readings.push(field.reading(raw));
        }
        LevelSource::Ratio { remaining, full } => {
            let raw_remaining = reports.value(&remaining)?;
            readings.push(remaining.reading(raw_remaining));
            let raw_full = reports.value(&full)?;
            readings.push(full.reading(raw_full));
        }
    }
    for field in [layout.charging, layout.discharging].into_iter().flatten() {
        if let Some(pressed) = reports.pressed(&field) {
            buttons.push(field.reading(pressed));
        }
    }
    let battery = decode_captured(layout, &readings, &buttons).ok_or_else(|| {
        ProtocolError::from("a selected battery field is missing or outside its declared range")
    })?;
    Ok((battery, readings, buttons))
}

/// Reports fetched during one read, so fields sharing a report cost a single call.
struct Reports<'a> {
    device: &'a HidCollection,
    fetched: Vec<((ReportKind, u8), Vec<u8>)>,
}

impl Reports<'_> {
    fn report(&mut self, kind: ReportKind, report_id: u8) -> io::Result<&mut Vec<u8>> {
        let position = match self
            .fetched
            .iter()
            .position(|(key, _)| *key == (kind, report_id))
        {
            Some(position) => position,
            None => {
                let report = self.device.fetch(kind, report_id)?;
                self.fetched.push(((kind, report_id), report));
                self.fetched.len() - 1
            }
        };
        Ok(&mut self.fetched[position].1)
    }

    fn value(&mut self, field: &ValueField) -> io::Result<u32> {
        let device = self.device;
        let report = self.report(field.kind, field.report_id)?;
        device.usage_value(field, report)
    }

    fn pressed(&mut self, field: &ButtonField) -> Option<bool> {
        let device = self.device;
        let report = self.report(field.kind, field.report_id).ok()?;
        device.usage_pressed(field, report)
    }
}

fn report_type(kind: ReportKind) -> HIDP_REPORT_TYPE {
    match kind {
        ReportKind::Feature => HidP_Feature,
        ReportKind::Input => HidP_Input,
    }
}

/// An open top-level HID collection together with its parsed report descriptor.
struct HidCollection {
    handle: HANDLE,
    preparsed: PHIDP_PREPARSED_DATA,
    caps: HIDP_CAPS,
}

impl Drop for HidCollection {
    fn drop(&mut self) {
        // SAFETY: preparsed data and handle are owned by this value and released exactly once.
        unsafe {
            HidD_FreePreparsedData(self.preparsed);
            CloseHandle(self.handle);
        }
    }
}

impl HidCollection {
    fn open(path: &CStr) -> io::Result<Self> {
        // SAFETY: path is NUL-terminated and the remaining arguments are plain values or null.
        // Zero desired access reads descriptors and reports without contending with other users.
        let handle = unsafe {
            CreateFileA(
                path.as_ptr().cast(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut preparsed: PHIDP_PREPARSED_DATA = 0;
        // SAFETY: handle is a valid open device and preparsed is writable.
        if unsafe { !HidD_GetPreparsedData(handle, &raw mut preparsed) } {
            let error = io::Error::last_os_error();
            // SAFETY: handle was opened above and is not used afterwards.
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        // SAFETY: HIDP_CAPS is a plain C struct for which all-zero bytes are valid.
        let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
        // SAFETY: preparsed data is live and caps is writable.
        if unsafe { HidP_GetCaps(preparsed, &raw mut caps) } != HIDP_STATUS_SUCCESS {
            // SAFETY: both resources were acquired above and are not used afterwards.
            unsafe {
                HidD_FreePreparsedData(preparsed);
                CloseHandle(handle);
            }
            return Err(io::Error::other(
                "could not parse the HID report descriptor",
            ));
        }
        Ok(Self {
            handle,
            preparsed,
            caps,
        })
    }

    /// Lists the battery-related usages in feature reports, then input reports.
    fn battery_fields(&self) -> (Vec<ValueField>, Vec<ButtonField>) {
        let mut values = Vec::new();
        let mut buttons = Vec::new();
        for kind in [ReportKind::Feature, ReportKind::Input] {
            let (value_count, button_count) = match kind {
                ReportKind::Feature => (
                    self.caps.NumberFeatureValueCaps,
                    self.caps.NumberFeatureButtonCaps,
                ),
                ReportKind::Input => (
                    self.caps.NumberInputValueCaps,
                    self.caps.NumberInputButtonCaps,
                ),
            };
            for cap in self.value_caps(kind, value_count) {
                // SAFETY: IsRange selects which union member the parser filled in.
                let (first, last) = unsafe {
                    if !cap.IsRange {
                        (cap.Anonymous.NotRange.Usage, cap.Anonymous.NotRange.Usage)
                    } else {
                        (cap.Anonymous.Range.UsageMin, cap.Anonymous.Range.UsageMax)
                    }
                };
                for usage in (first..=last).filter(|usage| is_battery_value(cap.UsagePage, *usage))
                {
                    values.push(ValueField {
                        page: cap.UsagePage,
                        usage,
                        report_id: cap.ReportID,
                        kind,
                        logical_min: cap.LogicalMin,
                        logical_max: cap.LogicalMax,
                    });
                }
            }
            for cap in self.button_caps(kind, button_count) {
                // SAFETY: IsRange selects which union member the parser filled in.
                let (first, last) = unsafe {
                    if !cap.IsRange {
                        (cap.Anonymous.NotRange.Usage, cap.Anonymous.NotRange.Usage)
                    } else {
                        (cap.Anonymous.Range.UsageMin, cap.Anonymous.Range.UsageMax)
                    }
                };
                for usage in (first..=last).filter(|usage| is_battery_button(cap.UsagePage, *usage))
                {
                    buttons.push(ButtonField {
                        page: cap.UsagePage,
                        usage,
                        report_id: cap.ReportID,
                        kind,
                    });
                }
            }
        }
        (values, buttons)
    }

    fn value_caps(&self, kind: ReportKind, count: u16) -> Vec<HIDP_VALUE_CAPS> {
        if count == 0 {
            return Vec::new();
        }
        // SAFETY: HIDP_VALUE_CAPS is a plain C struct for which all-zero bytes are valid.
        let mut caps = vec![unsafe { std::mem::zeroed::<HIDP_VALUE_CAPS>() }; usize::from(count)];
        let mut length = count;
        // SAFETY: caps holds `length` writable elements and the preparsed data is live.
        let status = unsafe {
            HidP_GetValueCaps(
                report_type(kind),
                caps.as_mut_ptr(),
                &raw mut length,
                self.preparsed,
            )
        };
        if status != HIDP_STATUS_SUCCESS {
            return Vec::new();
        }
        caps.truncate(usize::from(length));
        caps
    }

    fn button_caps(&self, kind: ReportKind, count: u16) -> Vec<HIDP_BUTTON_CAPS> {
        if count == 0 {
            return Vec::new();
        }
        // SAFETY: HIDP_BUTTON_CAPS is a plain C struct for which all-zero bytes are valid.
        let mut caps = vec![unsafe { std::mem::zeroed::<HIDP_BUTTON_CAPS>() }; usize::from(count)];
        let mut length = count;
        // SAFETY: caps holds `length` writable elements and the preparsed data is live.
        let status = unsafe {
            HidP_GetButtonCaps(
                report_type(kind),
                caps.as_mut_ptr(),
                &raw mut length,
                self.preparsed,
            )
        };
        if status != HIDP_STATUS_SUCCESS {
            return Vec::new();
        }
        caps.truncate(usize::from(length));
        caps
    }

    fn fetch(&self, kind: ReportKind, report_id: u8) -> io::Result<Vec<u8>> {
        let length = match kind {
            ReportKind::Feature => self.caps.FeatureReportByteLength,
            ReportKind::Input => self.caps.InputReportByteLength,
        };
        if length == 0 {
            return Err(io::Error::other("the collection has no such report"));
        }
        let mut report = vec![0u8; usize::from(length)];
        report[0] = report_id;
        let length = u32::from(length);
        // SAFETY: report is a writable buffer of exactly `length` bytes and the handle is open.
        let succeeded = unsafe {
            match kind {
                ReportKind::Feature => {
                    HidD_GetFeature(self.handle, report.as_mut_ptr().cast(), length)
                }
                ReportKind::Input => {
                    HidD_GetInputReport(self.handle, report.as_mut_ptr().cast(), length)
                }
            }
        };
        if !succeeded {
            return Err(io::Error::last_os_error());
        }
        Ok(report)
    }

    fn usage_value(&self, field: &ValueField, report: &[u8]) -> io::Result<u32> {
        let mut value = 0u32;
        // SAFETY: report is a live buffer of the reported length, value is writable, and the preparsed data is live.
        let status = unsafe {
            HidP_GetUsageValue(
                report_type(field.kind),
                field.page,
                0,
                field.usage,
                &raw mut value,
                self.preparsed,
                report.as_ptr(),
                u32::try_from(report.len()).unwrap_or(u32::MAX),
            )
        };
        if status == HIDP_STATUS_SUCCESS {
            Ok(value)
        } else {
            Err(io::Error::other(
                "could not extract the battery usage from its report",
            ))
        }
    }

    fn usage_pressed(&self, field: &ButtonField, report: &mut [u8]) -> Option<bool> {
        let mut usages = [0u16; 128];
        let mut count = u32::try_from(usages.len()).unwrap_or(u32::MAX);
        // SAFETY: usages holds `count` writable elements, report is a live buffer of the reported length, and the preparsed data is live.
        let status = unsafe {
            HidP_GetUsages(
                report_type(field.kind),
                PAGE_BATTERY_SYSTEM,
                0,
                usages.as_mut_ptr(),
                &raw mut count,
                self.preparsed,
                report.as_mut_ptr(),
                u32::try_from(report.len()).unwrap_or(u32::MAX),
            )
        };
        if status != HIDP_STATUS_SUCCESS {
            return None;
        }
        Some(usages[..count as usize].contains(&field.usage))
    }
}
