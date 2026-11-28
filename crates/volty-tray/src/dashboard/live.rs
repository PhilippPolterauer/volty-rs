//! Session-local, disposable projection of the monitor's current device state.
//! The tray process is the only writer. No live readings are stored on disk here.

use std::{
    io, ptr, slice,
    sync::atomic::{AtomicU32, Ordering::SeqCst},
};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE},
    System::Memory::{
        CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
        MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
    },
};

const MAPPING_NAME: &str = "Local\\VoltyLiveDevicesV1";
const WORDS: usize = 16_384;
const HEADER_WORDS: usize = 2;
const BYTES: usize = WORDS * size_of::<AtomicU32>();

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct LiveDevice {
    pub(crate) identity: String,
    pub(crate) name: String,
    pub(crate) reading: Option<LiveReading>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct LiveReading {
    pub(crate) level: LiveLevel,
    pub(crate) charging: bool,
    pub(crate) remaining_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) enum LiveLevel {
    Exact(u8),
    Estimated(u8),
    Unavailable,
}

impl LiveLevel {
    pub(crate) fn value(self) -> Option<u8> {
        match self {
            Self::Exact(value) | Self::Estimated(value) => Some(value),
            Self::Unavailable => None,
        }
    }
}

pub(crate) struct LivePublisher {
    view: View,
    last: Option<Vec<LiveDevice>>,
}

impl LivePublisher {
    pub(crate) fn start() -> io::Result<Self> {
        Ok(Self {
            view: View::open(MAPPING_NAME, true)?
                .ok_or_else(|| io::Error::other("Live mapping unavailable"))?,
            last: None,
        })
    }

    pub(crate) fn publish(&mut self, devices: Vec<LiveDevice>) -> io::Result<()> {
        if self.last.as_ref() == Some(&devices) {
            return Ok(());
        }
        let inventory_changed = self.last.as_ref().is_none_or(|current| {
            current
                .iter()
                .map(|device| (&device.identity, &device.name))
                .ne(devices
                    .iter()
                    .map(|device| (&device.identity, &device.name)))
        });
        let bytes = serde_json::to_vec(&devices)?;
        if bytes.len() > BYTES - HEADER_WORDS * size_of::<u32>() {
            self.clear();
            return Err(io::Error::other(
                "Live device snapshot exceeds shared memory capacity",
            ));
        }
        self.view.write(&bytes);
        self.last = Some(devices);
        super::window::notify_live();
        if inventory_changed {
            super::notify_dashboard();
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.view.write(&[]);
        self.last = None;
        super::window::notify_live();
    }
}

impl Drop for LivePublisher {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(super) fn read() -> Option<Vec<LiveDevice>> {
    let view = View::open(MAPPING_NAME, false).ok()??;
    read_snapshot(view.words())
}

fn read_snapshot(words: &[AtomicU32]) -> Option<Vec<LiveDevice>> {
    for _ in 0..3 {
        let before = words[0].load(SeqCst);
        let length = words[1].load(SeqCst) as usize;
        if length == 0 || length > BYTES - HEADER_WORDS * size_of::<u32>() {
            return None;
        }
        if !before.is_multiple_of(2) {
            continue;
        }
        let mut bytes = Vec::with_capacity(length.next_multiple_of(4));
        for word in &words[HEADER_WORDS..HEADER_WORDS + length.div_ceil(4)] {
            bytes.extend(word.load(SeqCst).to_le_bytes());
        }
        if words[0].load(SeqCst) == before {
            let devices = serde_json::from_slice::<Vec<LiveDevice>>(&bytes[..length]).ok()?;
            return (devices
                .windows(2)
                .all(|pair| pair[0].identity < pair[1].identity)
                && devices.iter().all(|device| {
                    !device.identity.is_empty()
                        && device
                            .reading
                            .and_then(|reading| reading.level.value())
                            .is_none_or(|value| value <= 100)
                }))
            .then_some(devices);
        }
    }
    None
}

struct View {
    handle: HANDLE,
    address: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl View {
    fn open(name: &str, write: bool) -> io::Result<Option<Self>> {
        let name = crate::win32::to_wide(name);
        // SAFETY: requests a zero-initialized paging-file mapping with a terminated name.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                BYTES as u32,
                name.as_ptr(),
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: this immediately retrieves CreateFileMappingW's existing-object status.
        if !write && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            // SAFETY: this call owns the handle and has not created a view.
            unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        // SAFETY: the handle is live. Keep it open so other processes can still find the name.
        let view = unsafe {
            let view = MapViewOfFile(
                handle,
                if write { FILE_MAP_WRITE } else { FILE_MAP_READ },
                0,
                0,
                BYTES,
            );
            if view.Value.is_null() {
                let error = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(error);
            }
            view
        };
        Ok(Some(Self {
            handle,
            address: view,
        }))
    }

    fn words(&self) -> &[AtomicU32] {
        // SAFETY: the owned view is page-aligned and spans WORDS initialized AtomicU32s.
        // Both processes access the mapped words exclusively through atomic operations.
        unsafe { slice::from_raw_parts(self.address.Value.cast(), WORDS) }
    }

    fn write(&self, bytes: &[u8]) {
        let words = self.words();
        let sequence = words[0].load(SeqCst) & !1;
        words[0].store(sequence.wrapping_add(1), SeqCst);
        words[1].store(bytes.len() as u32, SeqCst);
        for (slot, chunk) in words[HEADER_WORDS..].iter().zip(bytes.chunks(4)) {
            let mut word = [0; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            slot.store(u32::from_le_bytes(word), SeqCst);
        }
        words[0].store(sequence.wrapping_add(2), SeqCst);
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: this instance owns both the mapped view and its handle.
        unsafe {
            UnmapViewOfFile(self.address);
            CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_snapshot_preserves_estimates_and_clear_state() {
        let name = format!("Local\\VoltyLiveDevicesTest-{}", std::process::id());
        assert!(View::open(&name, false).unwrap().is_none());
        let writer = View::open(&name, true).unwrap().unwrap();
        let reader = View::open(&name, false).unwrap().unwrap();
        let mut device = LiveDevice {
            identity: "hid:test".to_owned(),
            name: "Test mouse".to_owned(),
            reading: Some(LiveReading {
                level: LiveLevel::Estimated(50),
                charging: false,
                remaining_seconds: None,
            }),
        };
        writer.write(&serde_json::to_vec(&[&device]).unwrap());
        assert_eq!(read_snapshot(reader.words()), Some(vec![device.clone()]));
        writer.words()[0].fetch_add(1, SeqCst);
        assert_eq!(read_snapshot(reader.words()), None);
        device.reading.as_mut().unwrap().level = LiveLevel::Exact(101);
        writer.write(&serde_json::to_vec(&[&device]).unwrap());
        assert_eq!(read_snapshot(reader.words()), None);
        writer.write(&[]);
        assert_eq!(read_snapshot(reader.words()), None);
    }
}
