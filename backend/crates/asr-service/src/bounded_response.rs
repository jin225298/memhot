use crate::error::{AsrError, Result};
use serde::de::DeserializeOwned;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Bound both advertised and actual (including chunked) provider response size.
pub(crate) async fn json<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(AsrError::InvalidResponse(
            "ASR response exceeds 2 MiB limit".into(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AsrError::NetworkError("ASR response read failed".into()))?
    {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(AsrError::InvalidResponse(
                "ASR response exceeds 2 MiB limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| AsrError::InvalidResponse("Invalid ASR JSON response".into()))
}

pub(crate) fn finish(reason: Option<&str>) -> Result<()> {
    match reason {
        Some("stop") => Ok(()),
        Some("length") => Err(AsrError::TruncatedResponse),
        _ => Err(AsrError::InvalidResponse(
            "missing or unsuccessful finish_reason".into(),
        )),
    }
}
