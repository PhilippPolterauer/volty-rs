use crate::{
    FeatureInfo, HidppError, HidppFrame, HidppTransport, SoftwareIdAllocator, FEATURE_DEVICE_NAME,
    FEATURE_ROOT, FEATURE_SET, FUNCTION_GET_DEVICE_NAME, FUNCTION_GET_DEVICE_NAME_FRAGMENT,
    FUNCTION_GET_FEATURE, FUNCTION_GET_FEATURE_SET_COUNT, FUNCTION_GET_FEATURE_SET_ID,
};

/// Reads a device name from the HID++ Device Name and Type feature.
///
/// Returns `None` when the device does not advertise the feature or has an
/// empty name. Longer names are fetched in protocol-sized fragments.
pub fn query_device_name(
    transport: &mut impl HidppTransport,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
    timeout: std::time::Duration,
) -> Result<Option<String>, HidppError> {
    let Some(feature) = get_feature(
        transport,
        device_index,
        FEATURE_DEVICE_NAME,
        software_ids.allocate(),
        timeout,
    )?
    else {
        return Ok(None);
    };

    let name_length_request = HidppFrame::long(
        device_index,
        feature.feature_index,
        FUNCTION_GET_DEVICE_NAME,
        software_ids.allocate(),
        [0; 16],
    );
    let name_length_response = transport.transact(&name_length_request, timeout)?;
    verify_response(&name_length_request, &name_length_response)?;
    let name_length =
        *name_length_response
            .params
            .first()
            .ok_or(HidppError::UnexpectedResponse(
                "device name response is missing its length",
            ))?;
    if name_length == 0 {
        return Ok(None);
    }

    let mut name = Vec::with_capacity(usize::from(name_length));
    while name.len() < usize::from(name_length) {
        let offset = u8::try_from(name.len()).map_err(|_| {
            HidppError::UnexpectedResponse("device name is longer than the protocol allows")
        })?;
        let mut params = [0; 16];
        params[0] = offset;
        let fragment_request = HidppFrame::long(
            device_index,
            feature.feature_index,
            FUNCTION_GET_DEVICE_NAME_FRAGMENT,
            software_ids.allocate(),
            params,
        );
        let fragment_response = transport.transact(&fragment_request, timeout)?;
        verify_response(&fragment_request, &fragment_response)?;
        let remaining = usize::from(name_length) - name.len();
        let fragment_length = fragment_response.params.len().min(remaining);
        if fragment_length == 0 {
            return Err(HidppError::UnexpectedResponse(
                "device name fragment is empty",
            ));
        }
        name.extend_from_slice(&fragment_response.params[..fragment_length]);
    }

    String::from_utf8(name)
        .map(Some)
        .map_err(|_| HidppError::UnexpectedResponse("device name is not valid UTF-8"))
}

/// Query a logical feature through Root using a caller-selected software ID.
pub fn get_feature(
    transport: &mut impl HidppTransport,
    device_index: u8,
    logical_feature_id: u16,
    software_id: u8,
    timeout: std::time::Duration,
) -> Result<Option<FeatureInfo>, HidppError> {
    let mut params = [0; 16];
    params[0] = (logical_feature_id >> 8) as u8;
    params[1] = logical_feature_id as u8;
    let request = HidppFrame::long(
        device_index,
        FEATURE_ROOT as u8,
        FUNCTION_GET_FEATURE,
        software_id,
        params,
    );
    let response = transport.transact(&request, timeout)?;
    verify_response(&request, &response)?;
    let feature_index = *response
        .params
        .first()
        .ok_or(HidppError::UnexpectedResponse(
            "missing feature index in Root response",
        ))?;
    if feature_index == 0 && logical_feature_id != FEATURE_ROOT {
        return Ok(None);
    }
    if response.params.len() < 3 {
        return Err(HidppError::UnexpectedResponse(
            "Root response is missing feature type or version",
        ));
    }
    Ok(Some(FeatureInfo {
        feature_index,
        feature_type: response.params[1],
        version: response.params[2],
    }))
}

/// Enumerate FeatureSet entries and resolve each through Root for its runtime index.
///
/// `software_ids` is caller-owned so command engines can share their allocator.
pub fn enumerate_features(
    transport: &mut impl HidppTransport,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
    timeout: std::time::Duration,
) -> Result<Vec<(u16, FeatureInfo)>, HidppError> {
    let feature_set = get_feature(
        transport,
        device_index,
        FEATURE_SET,
        software_ids.allocate(),
        timeout,
    )?
    .ok_or(HidppError::UnsupportedFeatureSet)?;
    let count_request = HidppFrame::long(
        device_index,
        feature_set.feature_index,
        FUNCTION_GET_FEATURE_SET_COUNT,
        software_ids.allocate(),
        [0; 16],
    );
    let count_response = transport.transact(&count_request, timeout)?;
    verify_response(&count_request, &count_response)?;
    let count = *count_response
        .params
        .first()
        .ok_or(HidppError::UnexpectedResponse(
            "FeatureSet count response is missing its count",
        ))?;

    let mut features = Vec::with_capacity(usize::from(count));
    for entry_index in 0..count {
        let mut params = [0; 16];
        params[0] = entry_index;
        let request = HidppFrame::long(
            device_index,
            feature_set.feature_index,
            FUNCTION_GET_FEATURE_SET_ID,
            software_ids.allocate(),
            params,
        );
        let response = transport.transact(&request, timeout)?;
        verify_response(&request, &response)?;
        if response.params.len() < 4 {
            return Err(HidppError::UnexpectedResponse(
                "FeatureSet entry is missing feature ID, type, or version",
            ));
        }
        let logical_id = u16::from_be_bytes([response.params[0], response.params[1]]);
        let feature = get_feature(
            transport,
            device_index,
            logical_id,
            software_ids.allocate(),
            timeout,
        )?
        .ok_or(HidppError::UnexpectedResponse(
            "FeatureSet entry is not present in Root",
        ))?;
        features.push((logical_id, feature));
    }
    Ok(features)
}

fn verify_response(request: &HidppFrame, response: &HidppFrame) -> Result<(), HidppError> {
    if response.device_index == request.device_index
        && response.feature_index == request.feature_index
        && response.function_id == request.function_id
        && response.software_id == request.software_id
    {
        Ok(())
    } else {
        Err(HidppError::UnexpectedResponse(
            "transport returned a response that does not match the request",
        ))
    }
}
