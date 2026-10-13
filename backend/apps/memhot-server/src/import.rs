//! Preserve original imports while extracting only explicitly attributed user spans.
use serde_json::Value;
fn content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    if let Some(parts) = value.as_array() {
        return parts
            .iter()
            .map(content)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    for key in ["parts", "text", "content"] {
        if let Some(v) = value.get(key) {
            let text = content(v);
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}
fn message(value: &Value) -> Option<String> {
    let role = value["role"]
        .as_str()
        .or_else(|| value["author"]["role"].as_str());
    if matches!(role, Some("user" | "human")) {
        Some(content(&value["content"]))
    } else {
        None
    }
}
pub fn user_text(text: &str, kind: &str, speaker_mode: &str) -> String {
    if kind != "import" || matches!(speaker_mode, "self" | "unknown") {
        return text.to_owned();
    }
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        let mut users = Vec::new();
        if let Some(messages) = value.as_array().or_else(|| value["messages"].as_array()) {
            users.extend(messages.iter().filter_map(message));
        }
        if let Some(mapping) = value["mapping"].as_object() {
            let mut messages: Vec<&Value> =
                mapping.values().filter_map(|v| v.get("message")).collect();
            messages.sort_by(|a, b| {
                a["create_time"]
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&b["create_time"].as_f64().unwrap_or(0.0))
            });
            users.extend(messages.into_iter().filter_map(message));
        }
        if let Some(t) = message(&value) {
            users.push(t);
        }
        return users
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    let mut role = "unknown";
    let mut result = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches('#').trim();
        let lower = trimmed.to_lowercase();
        if lower.starts_with("user:")
            || lower.starts_with("user：")
            || trimmed.starts_with("用户：")
            || trimmed.starts_with("用户:")
        {
            role = "user";
            result.push(
                trimmed
                    .split_once([':', '：'])
                    .map(|(_, t)| t)
                    .unwrap_or("")
                    .to_owned(),
            );
        } else if lower.starts_with("assistant:")
            || lower.starts_with("assistant：")
            || trimmed.starts_with("助手：")
            || trimmed.starts_with("助手:")
        {
            role = "assistant";
        } else if role == "user" {
            result.push(line.to_owned());
        }
    }
    result.join("\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_and_self_never_promote_assistant_into_user() {
        let json = r#"{"messages":[{"role":"user","content":"my idea"},{"role":"assistant","content":"AI assertion"}]}"#;
        assert_eq!(user_text(json, "import", "roles"), "my idea");
        assert_eq!(user_text("unattributed file", "import", "roles"), "");
        assert_eq!(
            user_text("unattributed file", "import", "self"),
            "unattributed file"
        );
        let mapping = r#"{"mapping":{"a":{"message":{"author":{"role":"user"},"content":{"parts":["user span"]},"create_time":1}},"b":{"message":{"author":{"role":"assistant"},"content":{"parts":["false assertion"]}}}}}"#;
        assert_eq!(user_text(mapping, "import", "roles"), "user span");
    }
    #[test]
    fn unknown_meeting_text_remains_available_without_implying_self_attribution() {
        assert_eq!(
            user_text(
                "Speaker A: a question\nSpeaker B: a reply",
                "import",
                "unknown"
            ),
            "Speaker A: a question\nSpeaker B: a reply"
        );
    }
}
