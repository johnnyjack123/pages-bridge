//! A small XML tokenizer for the WordprocessingML parts this program reads back. It handles
//! what those parts contain (elements, attributes, text, entities, CDATA, comments and
//! processing instructions) and no DTDs.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event<'a> {
    Start { name: &'a str, attrs: Vec<(&'a str, String)>, empty: bool },
    End { name: &'a str },
    Text(String),
}

pub(crate) struct Scanner<'a> {
    src: &'a str,
    pos: usize,
}

/// The part of a qualified name after the namespace prefix.
pub(crate) fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

/// The value of the attribute with this local name.
pub(crate) fn attr<'v>(attrs: &'v [(&str, String)], name: &str) -> Option<&'v str> {
    attrs.iter().find(|(n, _)| local(n) == name).map(|(_, v)| v.as_str())
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Self { src: src.trim_start_matches('\u{FEFF}'), pos: 0 }
    }

    pub(crate) fn next_event(&mut self) -> Result<Option<Event<'a>>, String> {
        loop {
            let rest = &self.src[self.pos..];
            if rest.is_empty() {
                return Ok(None);
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Ok(Some(Event::Text(unescape(&rest[..end]))));
            }
            let skip_to = |marker: &str| rest.find(marker).map(|i| i + marker.len()).ok_or_else(|| format!("unclosed {marker}"));
            if rest.starts_with("<?") {
                self.pos += skip_to("?>")?;
            } else if rest.starts_with("<!--") {
                self.pos += skip_to("-->")?;
            } else if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
                let end = cdata.find("]]>").ok_or("unclosed CDATA")?;
                self.pos += 9 + end + 3;
                return Ok(Some(Event::Text(cdata[..end].to_owned())));
            } else if rest.starts_with("<!") {
                self.pos += skip_to(">")?;
            } else if let Some(close) = rest.strip_prefix("</") {
                let end = close.find('>').ok_or("unclosed end tag")?;
                self.pos += 2 + end + 1;
                return Ok(Some(Event::End { name: close[..end].trim() }));
            } else {
                let end = tag_end(rest).ok_or("unclosed tag")?;
                let inner = &rest[1..end];
                let empty = inner.ends_with('/');
                let inner = inner.strip_suffix('/').unwrap_or(inner);
                let split = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
                let name = &inner[..split];
                let attrs = parse_attrs(&inner[split..])?;
                self.pos += end + 1;
                return Ok(Some(Event::Start { name, attrs, empty }));
            }
        }
    }
}

/// Index of the `>` closing the tag at the start of `s`, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_attrs(mut s: &str) -> Result<Vec<(&str, String)>, String> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Ok(out);
        }
        let eq = s.find('=').ok_or("attribute without value")?;
        let name = s[..eq].trim();
        let after = s[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("unquoted attribute")?;
        let close = after[1..].find(quote).ok_or("unclosed attribute value")?;
        out.push((name, unescape(&after[1..1 + close])));
        s = &after[1 + close + 1..];
    }
}

pub(crate) fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|i| *i <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_elements_attributes_and_text() {
        let mut s = Scanner::new(r#"<?xml version="1.0"?><w:p a="x &amp; y"><w:t xml:space='preserve'> a&lt;b&#x263A;</w:t><w:br/></w:p>"#);
        let mut events = Vec::new();
        while let Some(e) = s.next_event().unwrap() {
            events.push(e);
        }
        assert_eq!(events[0], Event::Start { name: "w:p", attrs: vec![("a", "x & y".into())], empty: false });
        assert_eq!(events[2], Event::Text(" a<b☺".into()));
        assert_eq!(events[4], Event::Start { name: "w:br", attrs: vec![], empty: true });
        assert_eq!(events[5], Event::End { name: "w:p" });
    }
}
