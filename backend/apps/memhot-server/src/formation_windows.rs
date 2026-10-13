//! Source coverage uses Unicode character offsets and disjoint ownership windows.
use serde_json::Value;
#[derive(Clone, Debug)]
pub struct Window {
    pub start: usize,
    pub end: usize,
}
pub const WINDOW_CHARS: usize = 10_000;
pub fn windows(text: &str) -> Vec<Window> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + WINDOW_CHARS).min(chars.len());
        if end < chars.len() {
            if let Some(boundary) = ((end - 1000)..end).rev().find(|i| chars[*i] == '\n') {
                end = boundary + 1;
            }
        }
        out.push(Window { start, end });
        start = end;
    }
    out
}
pub fn slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}
pub fn split(window: &Window) -> Option<[Window; 2]> {
    if window.end - window.start <= 1000 {
        return None;
    }
    let mid = (window.start + window.end) / 2;
    Some([
        Window {
            start: window.start,
            end: mid,
        },
        Window {
            start: mid,
            end: window.end,
        },
    ])
}
pub fn covered(records: &[Value], total: usize) -> usize {
    let mut spans: Vec<(usize, usize)> = records
        .iter()
        .filter(|r| r["status"] == "ready")
        .filter_map(|r| Some((r["start"].as_u64()? as usize, r["end"].as_u64()? as usize)))
        .collect();
    spans.sort_unstable();
    let (mut count, mut edge) = (0, 0);
    for (start, end) in spans {
        let end = end.min(total);
        if end > edge {
            count += end - start.max(edge).min(end);
            edge = end;
        }
    }
    count
}
pub fn completed(records: &[Value], window: &Window) -> bool {
    let terminal: Vec<_> = records
        .iter()
        .filter(|r| matches!(r["status"].as_str(), Some("ready" | "partial")))
        .filter_map(|r| {
            let start = (r["start"].as_u64()? as usize).max(window.start);
            let end = (r["end"].as_u64()? as usize).min(window.end);
            (end > start).then(|| serde_json::json!({"start":start,"end":end,"status":"ready"}))
        })
        .collect();
    covered(&terminal, window.end) == window.end - window.start
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn long_unicode_source_has_no_holes_and_reaches_the_tail() {
        let text = format!("{}\n末尾独立问题", "汉字讨论\n".repeat(10_000));
        let parts = windows(&text);
        assert!(parts.len() > 3);
        let reassembled = parts
            .iter()
            .map(|w| slice(&text, w.start, w.end))
            .collect::<String>();
        assert_eq!(text, reassembled);
        assert!(parts.iter().all(|w| w.end - w.start <= WINDOW_CHARS));
        assert_eq!(
            covered(
                &[
                    json!({"status":"ready","start":0,"end":10}),
                    json!({"status":"ready","start":5,"end":15})
                ],
                20
            ),
            15
        );
    }
}
