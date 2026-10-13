//! Deterministic quote matching. Stored evidence always comes from the original bytes.
pub struct Match<'a> {
    pub quote: &'a str,
    pub byte_start: usize,
    pub normalized: bool,
}
fn punctuation(c: char) -> char {
    match c {
        '，' => ',',
        '。' => '.',
        '：' => ':',
        '；' => ';',
        '！' => '!',
        '？' => '?',
        '“' | '”' => '"',
        '‘' | '’' => '\'',
        '（' => '(',
        '）' => ')',
        '【' => '[',
        '】' => ']',
        _ => c,
    }
}
pub fn locate<'a>(text: &'a str, quote: &str) -> Option<Match<'a>> {
    if quote.trim().is_empty() || quote.chars().count() > 2000 {
        return None;
    }
    if let Some(byte_start) = text.find(quote) {
        return Some(Match {
            quote: &text[byte_start..byte_start + quote.len()],
            byte_start,
            normalized: false,
        });
    }
    let target: Vec<char> = quote
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(punctuation)
        .collect();
    // No letter case folding, word substitutions, omitted punctuation, or fuzzy matching.
    if target.is_empty() {
        return None;
    }
    let mapped: Vec<(usize, char)> = text
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .collect();
    let positions: Vec<usize> = mapped
        .windows(target.len())
        .enumerate()
        .filter(|(_, w)| {
            w.iter()
                .zip(&target)
                .all(|((_, c), q)| punctuation(*c) == *q)
        })
        .map(|(i, _)| i)
        .take(2)
        .collect();
    // A normalized quote must map unambiguously to a single original span.
    if positions.len() != 1 {
        return None;
    }
    let first = positions[0];
    let byte_start = mapped[first].0;
    let last = mapped[first + target.len() - 1];
    Some(Match {
        quote: &text[byte_start..last.0 + last.1.len_utf8()],
        byte_start,
        normalized: true,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn punctuation_and_whitespace_map_back_to_exact_original_unicode_span() {
        let text = "前文。我们说：“先看\n  证据，再决定！”后文";
        let m = locate(text, "我们说: \"先看证据,再决定!\"").unwrap();
        assert!(m.normalized);
        assert_eq!(m.quote, "我们说：“先看\n  证据，再决定！”");
        assert_eq!(text[..m.byte_start].chars().count(), 3);
    }
    #[test]
    fn semantic_changes_case_changes_and_ambiguous_normalization_are_rejected() {
        assert!(locate("ai应先核验，不能猜测。", "AI应先核验，不能猜测。").is_none());
        assert!(locate("不能猜测。", "能猜测。").is_some()); // Exact substrings remain literal evidence.
        assert!(locate("我们不能猜测。", "我们能猜测。").is_none());
        assert!(locate("成本为10元。", "成本为100元。").is_none());
        assert!(locate("a b。a  b。", "ab.").is_none());
        assert!(locate("甲—乙", "甲-乙").is_none());
    }
}
