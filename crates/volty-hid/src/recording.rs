//! A recording HID++ transport that captures every exchange as bytes.

use crate::{HidppError, HidppFrame, HidppTransport};
use std::time::{Duration, Instant};

/// One HID++ request/response pair captured by [`RecordingTransport`].
#[derive(Clone, Debug)]
pub struct HidppExchange {
    /// The encoded request report.
    pub request: Vec<u8>,
    /// The encoded response report, when the transport returned a valid frame.
    pub response: Option<Vec<u8>>,
    /// The transport, protocol, or encoding error, rendered for logging.
    pub error: Option<String>,
    /// Wall-clock time spent inside the transport call.
    pub elapsed: Duration,
}

/// Wraps a [`HidppTransport`] and records every exchange in call order.
pub struct RecordingTransport<T: HidppTransport> {
    inner: T,
    exchanges: Vec<HidppExchange>,
}

impl<T: HidppTransport> RecordingTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            exchanges: Vec::new(),
        }
    }

    pub fn exchanges(&self) -> &[HidppExchange] {
        &self.exchanges
    }

    pub fn take_exchanges(&mut self) -> Vec<HidppExchange> {
        std::mem::take(&mut self.exchanges)
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    fn record(
        &mut self,
        request: Vec<u8>,
        response: Option<Vec<u8>>,
        error: Option<String>,
        started: Instant,
    ) {
        self.exchanges.push(HidppExchange {
            request,
            response,
            error,
            elapsed: started.elapsed(),
        });
    }
}

impl<T: HidppTransport> HidppTransport for RecordingTransport<T> {
    fn transact(
        &mut self,
        request: &HidppFrame,
        timeout: Duration,
    ) -> Result<HidppFrame, HidppError> {
        let started = Instant::now();
        let request_bytes = match request.encode() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.record(Vec::new(), None, Some(error.to_string()), started);
                return Err(error);
            }
        };

        match self.inner.transact(request, timeout) {
            Ok(response) => match response.encode() {
                Ok(response_bytes) => {
                    self.record(request_bytes, Some(response_bytes), None, started);
                    Ok(response)
                }
                Err(error) => {
                    self.record(request_bytes, None, Some(error.to_string()), started);
                    Err(error)
                }
            },
            Err(error) => {
                self.record(request_bytes, None, Some(error.to_string()), started);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct MockTransport {
        requests: Vec<HidppFrame>,
        responses: VecDeque<Result<HidppFrame, HidppError>>,
    }

    impl HidppTransport for MockTransport {
        fn transact(
            &mut self,
            request: &HidppFrame,
            _timeout: Duration,
        ) -> Result<HidppFrame, HidppError> {
            self.requests.push(request.clone());
            self.responses
                .pop_front()
                .unwrap_or(Err(HidppError::Timeout))
        }
    }

    #[test]
    fn successful_exchanges_record_encoded_request_and_response() {
        let request = HidppFrame::long(1, 6, 1, 3, [7; 16]);
        let response = HidppFrame::long(1, 6, 1, 3, [9; 16]);
        let mut transport = RecordingTransport::new(MockTransport {
            responses: VecDeque::from([Ok(response.clone())]),
            ..MockTransport::default()
        });

        assert_eq!(
            transport
                .transact(&request, Duration::from_secs(1))
                .unwrap(),
            response
        );

        let exchanges = transport.exchanges();
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].request, request.encode().unwrap());
        assert_eq!(
            exchanges[0].response.as_deref(),
            Some(response.encode().unwrap().as_slice())
        );
        assert!(exchanges[0].error.is_none());
        assert_eq!(transport.inner().requests, [request]);
    }

    #[test]
    fn transport_errors_are_recorded_and_returned_unchanged() {
        let request = HidppFrame::short(1, 2, 3, 4, [5, 6, 7]);
        let mut transport = RecordingTransport::new(MockTransport {
            responses: VecDeque::from([Err(HidppError::Timeout)]),
            ..MockTransport::default()
        });

        assert!(matches!(
            transport.transact(&request, Duration::from_secs(1)),
            Err(HidppError::Timeout)
        ));

        let exchange = &transport.exchanges()[0];
        assert_eq!(exchange.request, request.encode().unwrap());
        assert!(exchange.response.is_none());
        assert_eq!(
            exchange.error.as_deref(),
            Some("timed out waiting for a HID++ response")
        );
    }

    #[test]
    fn multiple_exchanges_are_preserved_in_call_order() {
        let short = HidppFrame::short(1, 2, 3, 4, [5, 6, 7]);
        let long = HidppFrame::long(2, 8, 1, 9, [0xaa; 16]);
        let short_response = HidppFrame::short(1, 2, 3, 4, [8, 9, 10]);
        let long_response = HidppFrame::long(2, 8, 1, 9, [0xbb; 16]);
        let mut transport = RecordingTransport::new(MockTransport {
            responses: VecDeque::from([
                Ok(short_response.clone()),
                Err(HidppError::UnexpectedResponse("second call failed")),
                Ok(long_response.clone()),
            ]),
            ..MockTransport::default()
        });

        transport.transact(&short, Duration::from_secs(1)).unwrap();
        transport
            .transact(&long, Duration::from_secs(1))
            .unwrap_err();
        transport.transact(&long, Duration::from_secs(1)).unwrap();

        let exchanges = transport.take_exchanges();
        assert_eq!(exchanges.len(), 3);
        assert_eq!(exchanges[0].request, short.encode().unwrap());
        assert_eq!(
            exchanges[0].response.as_deref(),
            Some(short_response.encode().unwrap().as_slice())
        );
        assert_eq!(exchanges[1].request, long.encode().unwrap());
        assert!(exchanges[1].response.is_none());
        assert!(exchanges[1].error.is_some());
        assert_eq!(exchanges[2].request, long.encode().unwrap());
        assert_eq!(
            exchanges[2].response.as_deref(),
            Some(long_response.encode().unwrap().as_slice())
        );
        assert!(transport.exchanges().is_empty());
    }

    #[test]
    fn request_encode_errors_are_recorded_without_calling_the_inner_transport() {
        let mut request = HidppFrame::long(1, 6, 1, 3, [0; 16]);
        request.params.pop();
        let mut transport = RecordingTransport::new(MockTransport::default());

        let error = transport
            .transact(&request, Duration::from_secs(1))
            .unwrap_err();
        let message = error.to_string();
        assert!(matches!(
            error,
            HidppError::InvalidPayloadLength {
                expected: 16,
                actual: 15
            }
        ));

        let exchange = &transport.exchanges()[0];
        assert!(exchange.request.is_empty());
        assert!(exchange.response.is_none());
        assert_eq!(exchange.error.as_deref(), Some(message.as_str()));
        assert!(transport.inner().requests.is_empty());
    }
}
