//! Style inheritance (`TSS.StyleArchive` parent chains) and the property fields of character,
//! paragraph and list styles.

use std::collections::{HashMap, HashSet};

use iwork::{Msg, Package};

use crate::model::{
    Align, Caps, CharProps, LineSpacing, ListLabel, ListLevel, ListStyle, NumberFormat, ParaKind, ParaProps, Rgb,
    Strike, TabAlign, TabStop, Underline, VerticalAlign,
};

/// Field of a character or paragraph style that holds its character properties.
pub(super) const CHAR_PROPS: u32 = 11;
/// Field of a paragraph style that holds its paragraph properties.
pub(super) const PARA_PROPS: u32 = 12;

pub(super) struct Styles<'p> {
    pkg: &'p Package,
    chars: HashMap<Option<u64>, CharProps>,
    paras: HashMap<Option<u64>, (ParaProps, Option<u64>)>,
    kinds: HashMap<Option<u64>, ParaKind>,
}

impl<'p> Styles<'p> {
    pub(super) fn new(pkg: &'p Package) -> Self {
        Self { pkg, chars: HashMap::new(), paras: HashMap::new(), kinds: HashMap::new() }
    }

    /// The style and its ancestors, root first.
    fn chain(&self, id: Option<u64>) -> Vec<Msg<'p>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut current = id;
        while let Some(id) = current {
            if !seen.insert(id) || out.len() >= 32 {
                break;
            }
            let Some(style) = self.pkg.object(id) else { break };
            current = style.msg(1).and_then(|sup| sup.reference(3));
            out.push(style);
        }
        out.reverse();
        out
    }

    /// Property message `field` with every ancestor's values overridden by the descendants'.
    pub(super) fn merged(&self, id: Option<u64>, field: u32) -> Msg<'p> {
        let mut merged = Msg::default();
        for style in self.chain(id) {
            if let Some(props) = style.msg(field) {
                merged.overlay(&props);
            }
        }
        merged
    }

    pub(super) fn char_props(&mut self, id: Option<u64>) -> CharProps {
        if let Some(c) = self.chars.get(&id) {
            return c.clone();
        }
        let c = char_props_from(&self.merged(id, CHAR_PROPS));
        self.chars.insert(id, c.clone());
        c
    }

    fn para_entry(&mut self, id: Option<u64>) -> &(ParaProps, Option<u64>) {
        if !self.paras.contains_key(&id) {
            let m = self.merged(id, PARA_PROPS);
            self.paras.insert(id, (para_props_from(&m), m.reference(40)));
        }
        &self.paras[&id]
    }

    pub(super) fn para_props(&mut self, id: Option<u64>) -> ParaProps {
        self.para_entry(id).0.clone()
    }

    /// The list style a paragraph style applies when the text sets none itself.
    pub(super) fn para_list_style(&mut self, id: Option<u64>) -> Option<u64> {
        self.para_entry(id).1
    }

    /// Built-in role of a paragraph style, from the closest built-in identifier in its chain.
    pub(super) fn kind(&mut self, id: Option<u64>) -> ParaKind {
        if let Some(k) = self.kinds.get(&id) {
            return *k;
        }
        let ident = self
            .chain(id)
            .iter()
            .rev()
            .find_map(|s| s.msg(1).and_then(|sup| sup.string(2)).filter(|i| !i.is_empty()))
            .unwrap_or_default();
        let kind = match ident.rsplit_once("paragraphstyle-").map(|(_, name)| name) {
            Some("Title") => ParaKind::Title,
            Some("Subtitle") => ParaKind::Subtitle,
            Some(name) => name
                .strip_prefix("Heading ")
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| (1..=9).contains(n))
                .map_or(ParaKind::Normal, ParaKind::Heading),
            None => ParaKind::Normal,
        };
        self.kinds.insert(id, kind);
        kind
    }

    /// List styles keep their properties at the top level of the style object.
    pub(super) fn list_style(&self, id: u64) -> ListStyle {
        let mut merged = Msg::default();
        for style in self.chain(Some(id)) {
            merged.overlay(&style);
        }
        list_style_from(&merged)
    }
}

pub(super) fn color(m: &Msg) -> Option<Rgb> {
    if !m.has(3) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let channel = |n| (m.float(n).unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Rgb(channel(3), channel(4), channel(5)))
}

/// `TSWP.CharacterStylePropertiesArchive`. Verified against Pages' named styles: "Kursiv" sets
/// 2, "Unterstrichen" 11, "Durchgestrichen" 12, "Rot, fett" 1 and 7.
fn char_props_from(m: &Msg) -> CharProps {
    CharProps {
        bold: m.bool(1),
        italic: m.bool(2),
        size: m.float(3).filter(|s| *s > 0.0),
        font: m.string(5).filter(|f| !f.is_empty()),
        color: m.msg(7).as_ref().and_then(color),
        vertical: m.uint(10).map(|v| match v {
            1 => VerticalAlign::Super,
            2 => VerticalAlign::Sub,
            _ => VerticalAlign::Baseline,
        }),
        underline: m.uint(11).map(|v| match v {
            0 => Underline::None,
            2 => Underline::Double,
            _ => Underline::Single,
        }),
        strike: m.uint(12).map(|v| match v {
            0 => Strike::None,
            2 => Strike::Double,
            _ => Strike::Single,
        }),
        caps: m.uint(13).map(|v| match v {
            1 => Caps::All,
            2 => Caps::Small,
            _ => Caps::None,
        }),
        baseline_shift: m.float(14),
    }
}

/// `TSWP.ParagraphStylePropertiesArchive`.
fn para_props_from(m: &Msg) -> ParaProps {
    ParaProps {
        align: m.uint(1).map(|v| match v {
            0 => Align::Left,
            1 => Align::Right,
            2 => Align::Center,
            3 => Align::Justify,
            _ => Align::Natural,
        }),
        first_indent: m.float(7),
        keep_lines: m.bool(9),
        keep_with_next: m.bool(10),
        left_indent: m.float(11),
        line_spacing: m.msg(13).map(|ls| {
            let amount = ls.float(2).unwrap_or(1.0);
            match ls.uint(1).unwrap_or(0) {
                0 => LineSpacing::Relative(amount),
                1 => LineSpacing::AtLeast(amount),
                2 | 3 => LineSpacing::Exactly(amount),
                _ => LineSpacing::Between(amount),
            }
        }),
        page_break_before: m.bool(14),
        right_indent: m.float(19),
        // Heading styles set 21 to the larger value, so 21 is the space above, 20 below.
        space_after: m.float(20),
        space_before: m.float(21),
        tabs: m.msg(25).map(|tabs| {
            tabs.msgs(1)
                .iter()
                .filter_map(|t| {
                    Some(TabStop {
                        position: t.float(1)?,
                        align: match t.uint(2).unwrap_or(0) {
                            1 => TabAlign::Center,
                            2 => TabAlign::Right,
                            3 => TabAlign::Decimal,
                            _ => TabAlign::Left,
                        },
                    })
                })
                .collect()
        }),
        widow_control: m.bool(26),
    }
}

/// `TSWP.ListStyleArchive.NumberType` → (format, prefix, suffix).
fn number_format(kind: u64) -> (NumberFormat, &'static str, &'static str) {
    let format = match kind / 3 {
        1 => NumberFormat::UpperRoman,
        2 => NumberFormat::LowerRoman,
        3 => NumberFormat::UpperLetter,
        4 => NumberFormat::LowerLetter,
        _ => NumberFormat::Decimal,
    };
    let (prefix, suffix) = match kind % 3 {
        1 => ("(", ")"),
        2 => ("", ")"),
        _ => ("", "."),
    };
    if kind > 14 { (NumberFormat::Decimal, "", ".") } else { (format, prefix, suffix) }
}

fn list_style_from(m: &Msg) -> ListStyle {
    let label_types = m.uints(11);
    // Text offset per level in multiples of the template's 11 pt body size: 12 pt and 18 pt for
    // the built-in dash and numbered lists.
    let scales = m.floats(12);
    let indents = m.floats(13);
    let number_types = m.uints(15);
    let bullets = m.strings(16);
    let tiered = m.uints(25);
    fn pick<T: Clone>(values: &[T], level: usize, default: T) -> T {
        values.get(level).or_else(|| values.last()).cloned().unwrap_or(default)
    }
    let levels = (0..9)
        .map(|level| {
            let label = match pick(&label_types, level, 0) {
                3 => {
                    let (format, prefix, suffix) = number_format(pick(&number_types, level, 0));
                    ListLabel::Number { format, prefix, suffix, tiered: pick(&tiered, level, 0) != 0 }
                }
                1 | 2 => {
                    let bullet = pick(&bullets, level, String::new());
                    ListLabel::Bullet(if bullet.is_empty() { "•".to_owned() } else { bullet })
                }
                _ => ListLabel::None,
            };
            #[allow(clippy::cast_precision_loss)]
            let default_indent = level as f64 * 18.0;
            ListLevel {
                label,
                label_indent: pick(&indents, level, default_indent),
                text_offset: (pick(&scales, level, 1.6364) * 11.0).max(9.0),
            }
        })
        .collect();
    ListStyle { levels }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_number_types() {
        assert_eq!(number_format(0), (NumberFormat::Decimal, "", "."));
        assert_eq!(number_format(4), (NumberFormat::UpperRoman, "(", ")"));
        assert_eq!(number_format(14), (NumberFormat::LowerLetter, "", ")"));
        assert_eq!(number_format(99), (NumberFormat::Decimal, "", "."));
    }
}
