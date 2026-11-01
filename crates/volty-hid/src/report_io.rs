//! Device-neutral HID report I/O and an opt-in recorder for protocol work.

use std::{error::Error, fmt, time::Duration, time::Instant};

/// The HID operation that produced a captured report.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HidReportDirection {
    Input,
    Output,
    FeatureRead,
    FeatureWrite,
}

/// One successful HID report operation, timestamped relative to recorder start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HidReportEvent {
    pub t_ms: u64,
    pub direction: HidReportDirection,
    pub bytes: Vec<u8>,
}

/// Report-level I/O implemented by a native HID handle or a test transport.
pub trait HidReportIo {
    type Error;

    /// Reads one input report, including its report ID byte; zero means timeout.
    fn read_input(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize, Self::Error>;

    /// Writes one output report, including its report ID byte.
    fn write_output(&mut self, report: &[u8]) -> Result<usize, Self::Error>;

    /// Reads a feature report; the buffer's first byte must contain the report ID.
    fn get_feature_report(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error>;

    /// Writes one feature report, including its report ID byte.
    fn send_feature_report(&mut self, report: &[u8]) -> Result<usize, Self::Error>;
}

/// Errors surfaced by the recorder without hiding transport failures.
#[derive(Debug)]
pub enum HidReportIoError<E> {
    Transport(E),
    InvalidLength {
        direction: HidReportDirection,
        reported: usize,
        buffer_len: usize,
    },
}

impl<E: fmt::Display> fmt::Display for HidReportIoError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "HID transport error: {error}"),
            Self::InvalidLength {
                direction,
                reported,
                buffer_len,
            } => write!(
                f,
                "HID {direction:?} returned {reported} bytes for a {buffer_len} byte buffer"
            ),
        }
    }
}

impl<E: Error + 'static> Error for HidReportIoError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::InvalidLength { .. } => None,
        }
    }
}

/// Wraps HID report I/O and records successful input, output, and feature operations.
pub struct RecordingHidIo<T: HidReportIo> {
    inner: T,
    started: Instant,
    events: Vec<HidReportEvent>,
}

impl<T: HidReportIo> RecordingHidIo<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            started: Instant::now(),
            events: Vec::new(),
        }
    }

    pub fn take_events(&mut self) -> Vec<HidReportEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    fn record(
        &mut self,
        direction: HidReportDirection,
        bytes: &[u8],
        reported: usize,
    ) -> Result<(), HidReportIoError<T::Error>> {
        if reported > bytes.len() {
            return Err(HidReportIoError::InvalidLength {
                direction,
                reported,
                buffer_len: bytes.len(),
            });
        }
        if reported > 0 {
            self.events.push(HidReportEvent {
                t_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                direction,
                bytes: bytes[..reported].to_vec(),
            });
        }
        Ok(())
    }
}

impl<T: HidReportIo> HidReportIo for RecordingHidIo<T> {
    type Error = HidReportIoError<T::Error>;

    fn read_input(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize, Self::Error> {
        let reported = self
            .inner
            .read_input(buffer, timeout)
            .map_err(HidReportIoError::Transport)?;
        self.record(HidReportDirection::Input, buffer, reported)?;
        Ok(reported)
    }

    fn write_output(&mut self, report: &[u8]) -> Result<usize, Self::Error> {
        let written = self
            .inner
            .write_output(report)
            .map_err(HidReportIoError::Transport)?;
        self.record(HidReportDirection::Output, report, written)?;
        Ok(written)
    }

    fn get_feature_report(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        let reported = self
            .inner
            .get_feature_report(buffer)
            .map_err(HidReportIoError::Transport)?;
        self.record(HidReportDirection::FeatureRead, buffer, reported)?;
        Ok(reported)
    }

    fn send_feature_report(&mut self, report: &[u8]) -> Result<usize, Self::Error> {
        let written = self
            .inner
            .send_feature_report(report)
            .map_err(HidReportIoError::Transport)?;
        self.record(HidReportDirection::FeatureWrite, report, written)?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, io};

    #[derive(Default)]
    struct MockIo {
        reads: VecDeque<Vec<u8>>,
        writes: Vec<Vec<u8>>,
        feature_reads: Vec<Vec<u8>>,
        feature_writes: Vec<Vec<u8>>,
    }

    impl HidReportIo for MockIo {
        type Error = io::Error;

        fn read_input(
            &mut self,
            buffer: &mut [u8],
            _timeout: Duration,
        ) -> Result<usize, Self::Error> {
            let Some(report) = self.reads.pop_front() else {
                return Ok(0);
            };
            buffer[..report.len()].copy_from_slice(&report);
            Ok(report.len())
        }

        fn write_output(&mut self, report: &[u8]) -> Result<usize, Self::Error> {
            self.writes.push(report.to_vec());
            Ok(report.len())
        }

        fn get_feature_report(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
            let report = vec![buffer[0], 0x7f];
            buffer[..report.len()].copy_from_slice(&report);
            self.feature_reads.push(report.clone());
            Ok(report.len())
        }

        fn send_feature_report(&mut self, report: &[u8]) -> Result<usize, Self::Error> {
            self.feature_writes.push(report.to_vec());
            Ok(report.len())
        }
    }

    #[test]
    fn records_successful_report_operations_in_call_order() {
        let io = MockIo {
            reads: VecDeque::from([vec![1, 2, 3]]),
            ..MockIo::default()
        };
        let mut recording = RecordingHidIo::new(io);
        let mut input = [0; 8];
        assert_eq!(
            recording
                .read_input(&mut input, Duration::from_millis(10))
                .unwrap(),
            3
        );
        assert_eq!(recording.write_output(&[4, 5]).unwrap(), 2);
        let mut feature = [6, 0, 0];
        assert_eq!(recording.get_feature_report(&mut feature).unwrap(), 2);
        assert_eq!(recording.send_feature_report(&[7, 8, 9]).unwrap(), 3);

        let events = recording.take_events();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.direction, event.bytes.clone()))
                .collect::<Vec<_>>(),
            [
                (HidReportDirection::Input, vec![1, 2, 3]),
                (HidReportDirection::Output, vec![4, 5]),
                (HidReportDirection::FeatureRead, vec![6, 0x7f]),
                (HidReportDirection::FeatureWrite, vec![7, 8, 9]),
            ]
        );
        assert!(events.windows(2).all(|pair| pair[0].t_ms <= pair[1].t_ms));
    }

    #[test]
    fn timeouts_are_not_recorded_as_empty_reports() {
        let mut recording = RecordingHidIo::new(MockIo::default());
        assert_eq!(
            recording
                .read_input(&mut [0; 8], Duration::from_millis(1))
                .unwrap(),
            0
        );
        assert!(recording.take_events().is_empty());
    }

    #[test]
    fn invalid_transport_lengths_are_reported() {
        struct InvalidIo;

        impl HidReportIo for InvalidIo {
            type Error = io::Error;

            fn read_input(
                &mut self,
                _buffer: &mut [u8],
                _timeout: Duration,
            ) -> Result<usize, Self::Error> {
                Ok(2)
            }

            fn write_output(&mut self, _report: &[u8]) -> Result<usize, Self::Error> {
                Ok(0)
            }

            fn get_feature_report(&mut self, _buffer: &mut [u8]) -> Result<usize, Self::Error> {
                Ok(0)
            }

            fn send_feature_report(&mut self, _report: &[u8]) -> Result<usize, Self::Error> {
                Ok(0)
            }
        }

        let mut recording = RecordingHidIo::new(InvalidIo);
        assert!(matches!(
            recording.read_input(&mut [0], Duration::ZERO),
            Err(HidReportIoError::InvalidLength { reported: 2, .. })
        ));
    }
}
