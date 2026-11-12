//! Text and attribute escaping for WordprocessingML.

/// Escapes text content and drops characters XML 1.0 does not allow.
pub(crate) fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if c < ' ' || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn attr(s: &str) -> String {
    text(s).replace('"', "&quot;")
}

/// Points to twentieths of a point, Word's usual length unit.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn twips(pt: f64) -> i64 {
    (pt * 20.0).round() as i64
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_and_filters() {
        assert_eq!(super::attr("a<b>&\"c\u{1}"), "a&lt;b&gt;&amp;&quot;c");
    }
}
