//! Validate provider completion from SSE records; diagnostics never expose payloads.
use crate::error::{Error, Result};
use serde_json::Value;

#[derive(Default)]
struct Stream {
    sse: bool,
    json_events: usize,
    error_event: bool,
    ended: bool,
    truncated: bool,
    malformed: bool,
    done: bool,
}

fn record(protocol: &str, name: &str, data: &mut Vec<&str>, stream: &mut Stream) {
    stream.error_event |= name == "error";
    if data.is_empty() {
        return;
    }
    let payload = data.join("\n");
    data.clear();
    if payload == "[DONE]" && protocol == "openai" {
        stream.done = true;
        return;
    }
    let anthropic_message = matches!(
        name,
        "message_start"
            | "message_delta"
            | "message_stop"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
    );
    if protocol == "anthropic" && !anthropic_message {
        return;
    }
    let Ok(event) = serde_json::from_str::<Value>(&payload) else {
        stream.malformed = true;
        return;
    };
    stream.json_events += 1;
    stream.error_event |=
        event.get("error").is_some_and(|error| !error.is_null()) || event["type"] == "error";
    // These are either ignored or yielded with a wrapper by the native adapters,
    // so their payload cannot establish a usable model completion.
    if (protocol == "openai" && name.starts_with("thread."))
        || (protocol == "anthropic"
            && (name != "message_delta" || event["type"] != "message_delta"))
    {
        return;
    }
    let (finish, complete, truncated): (_, &[&str], _) = match protocol {
        "openai" => (
            event["choices"][0]["finish_reason"].as_str(),
            &["stop", "tool_calls", "function_call"],
            "length",
        ),
        "anthropic" => (
            event["delta"]["stop_reason"].as_str(),
            &["end_turn", "stop_sequence", "tool_use"],
            "max_tokens",
        ),
        "gemini" => (
            event["candidates"][0]["finishReason"].as_str(),
            &["STOP"],
            "MAX_TOKENS",
        ),
        _ => (None, &[], ""),
    };
    if let Some(finish) = finish {
        stream.ended |= complete.contains(&finish);
        stream.truncated |= finish == truncated;
    }
}

fn parse(protocol: &str, body: &str) -> Stream {
    let mut stream = Stream::default();
    let mut remaining = body.strip_prefix('\u{feff}').unwrap_or(body);
    let mut name = "";
    let mut data = Vec::new();
    while !remaining.is_empty() {
        let end = remaining.find(['\r', '\n']).unwrap_or(remaining.len());
        let line = &remaining[..end];
        remaining = &remaining[end..];
        remaining = remaining
            .strip_prefix("\r\n")
            .or_else(|| remaining.strip_prefix(['\r', '\n']))
            .unwrap_or(remaining);
        if line.is_empty() {
            record(protocol, name, &mut data, &mut stream);
            if stream.done {
                break;
            }
            name = "";
            continue;
        }
        if line.starts_with(':') {
            stream.sse = true;
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                stream.sse = true;
                data.push(value);
            }
            "event" => {
                stream.sse = true;
                name = value;
            }
            "id" | "retry" => stream.sse = true,
            _ => {}
        }
    }
    // Only OpenAI's native adapter flushes a final SSE record at EOF. Anthropic
    // requires a named, blank-line-terminated event. DONE ends OpenAI consumption.
    if protocol == "openai" && !stream.done {
        record(protocol, name, &mut data, &mut stream);
    }
    stream
}

pub(super) fn validate(protocol: &str, body: &str) -> Result<()> {
    let stream = parse(protocol, body);
    if stream.malformed {
        return Err(Error::upstream("invalid_response", "模型流包含无效事件"));
    }
    if stream.truncated {
        return Err(Error::upstream(
            "truncated",
            "模型输出达到上限，未发布不完整回答",
        ));
    }
    if stream.error_event {
        return Err(Error::upstream("invalid_response", "模型流返回错误事件"));
    }
    if !stream.ended {
        return Err(Error::upstream("invalid_response", "模型流未完整结束"));
    }
    Ok(())
}

pub(super) struct Diagnostic {
    pub shape: &'static str,
    pub bytes: usize,
    pub json_events: usize,
    pub error_event: bool,
}

pub(super) fn diagnostic(protocol: &str, body: &str) -> Diagnostic {
    let stream = parse(protocol, body);
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    let html = ["<!doctype html", "<html", "<head", "<body"]
        .iter()
        .any(|prefix| {
            trimmed
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        });
    let shape = if trimmed.is_empty() {
        "empty"
    } else if stream.sse {
        "sse"
    } else if serde_json::from_str::<Value>(trimmed).is_ok() {
        "json"
    } else if html {
        "html"
    } else {
        "other"
    };
    Diagnostic {
        shape,
        bytes: body.len(),
        json_events: stream.json_events,
        error_event: stream.error_event,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FINISH: &str = "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n";

    #[test]
    fn standard_and_legal_sse_variants_have_provider_finish() {
        for body in [
            format!("{FINISH}data: [DONE]\n\n"),
            format!("\u{feff}{FINISH}"),
            format!("data: {{\"choices\":\ndata: [{{\"finish_reason\":\"stop\"}}]}}\n\n"),
            format!("data: {{\"choices\":[]}}\r\r{FINISH}").replace('\n', "\r"),
            FINISH.replace('\n', "\r\n"),
            FINISH.trim_end().to_owned(),
        ] {
            assert!(validate("openai", &body).is_ok());
            assert!(diagnostic("openai", &body).json_events > 0);
        }
    }

    #[test]
    fn illegal_leading_space_and_done_only_remain_incomplete() {
        for body in [FINISH.replace("data:", " data:"), "data: [DONE]\n\n".into()] {
            assert_eq!(
                validate("openai", &body).unwrap_err().code,
                "invalid_response"
            );
            assert_eq!(diagnostic("openai", &body).json_events, 0);
        }
    }

    #[test]
    fn error_event_or_non_null_error_rejects_even_after_finish() {
        for error in [
            "event: error\ndata: {}\n\n",
            "data: {\"error\":{}}\n\n",
            "data: {\"type\":\"error\",\"error\":null}\n\n",
        ] {
            let body = format!("{FINISH}{error}");
            assert_eq!(
                validate("openai", &body).unwrap_err().code,
                "invalid_response"
            );
            assert!(diagnostic("openai", &body).error_event);
        }
        let body = format!("{FINISH}data: {{\"error\":null}}\n\n");
        assert!(validate("openai", &body).is_ok());
        assert!(!diagnostic("openai", &body).error_event);
    }

    #[test]
    fn finish_reasons_and_truncation_are_protocol_specific() {
        for (protocol, finish, cutoff) in [
            (
                "openai",
                FINISH,
                "data: {\"choices\":[{\"finish_reason\":\"length\"}]}",
            ),
            (
                "anthropic",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
            ),
            (
                "gemini",
                "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
                "data: {\"candidates\":[{\"finishReason\":\"MAX_TOKENS\"}]}\n\n",
            ),
        ] {
            assert!(validate(protocol, finish).is_ok());
            assert_eq!(validate(protocol, cutoff).unwrap_err().code, "truncated");
        }
        let wrong = "data: {\"choices\":[{\"finish_reason\":\"STOP\"}]}";
        assert_eq!(
            validate("openai", wrong).unwrap_err().code,
            "invalid_response"
        );
        assert!(validate("anthropic", FINISH).is_err());
    }

    #[test]
    fn native_adapter_rejection_and_done_boundary_are_preserved() {
        for body in [
            format!("data: invalid\n\n{FINISH}"),
            format!("data: [DONE]\n\n{FINISH}"),
            format!("event: thread.fake\n{FINISH}"),
        ] {
            assert!(validate("openai", &body).is_err());
        }
        assert!(validate(
            "openai",
            &format!("{FINISH}data: [DONE]\n\ndata: invalid\n\n")
        )
        .is_ok());
        let delta = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}";
        assert!(validate("anthropic", &format!("{delta}\n\n")).is_err());
        assert!(validate("anthropic", &format!("event: message_delta\n{delta}")).is_err());
        assert!(validate("anthropic", &format!("event: message_delta\n{delta}\n\n")).is_ok());
        let wrong = "event: message_delta\ndata: {\"type\":\"ping\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n";
        assert!(validate("anthropic", wrong).is_err());
    }
    #[test]
    fn diagnostics_only_classify_fixed_shapes_and_counts() {
        for (body, shape) in [
            (" \n", "empty"),
            ("{\"error\":{\"message\":\"private\"}}", "json"),
            ("<!DOCTYPE HTML><html>private</html>", "html"),
            ("private", "other"),
            (FINISH, "sse"),
        ] {
            let diagnostic = diagnostic("openai", body);
            assert_eq!(diagnostic.shape, shape);
            assert_eq!(diagnostic.bytes, body.len());
        }
    }
}
