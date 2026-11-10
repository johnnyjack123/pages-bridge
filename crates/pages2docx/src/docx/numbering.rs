//! `word/numbering.xml`: one abstract numbering per Pages list style, one instance per run of a
//! list that restarts.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::xml::{attr, twips};
use super::NS;
use crate::model::{ListLabel, ListStyle, NumberFormat};

pub(super) struct Numbering {
    abstract_ids: BTreeMap<u64, usize>,
    /// (numId, abstract id, restart value)
    instances: Vec<(usize, usize, Option<u32>)>,
    current: BTreeMap<u64, usize>,
}

impl Numbering {
    pub(super) fn new() -> Self {
        Self { abstract_ids: BTreeMap::new(), instances: Vec::new(), current: BTreeMap::new() }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The `w:numId` for a list paragraph; a restart opens a new instance.
    pub(super) fn num_id(&mut self, style: u64, restart: Option<u32>) -> usize {
        let next_abstract = self.abstract_ids.len();
        let abstract_id = *self.abstract_ids.entry(style).or_insert(next_abstract);
        if restart.is_some() || !self.current.contains_key(&style) {
            let id = self.instances.len() + 1;
            // Word continues numbering across instances of one abstract numbering unless a
            // later instance overrides its start; the first needs no override.
            let start = if self.current.contains_key(&style) { restart } else { None };
            self.instances.push((id, abstract_id, start));
            self.current.insert(style, id);
        }
        self.current[&style]
    }

    pub(super) fn xml(&self, lists: &BTreeMap<u64, ListStyle>) -> String {
        let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering {NS}>");
        for (style, abstract_id) in &self.abstract_ids {
            let Some(list) = lists.get(style) else { continue };
            let _ = write!(out, "<w:abstractNum w:abstractNumId=\"{abstract_id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
            for (level, def) in list.levels.iter().enumerate().take(9) {
                let (format, text) = match &def.label {
                    ListLabel::Number { format, prefix, suffix, tiered } => {
                        let number = if *tiered {
                            (1..=level + 1).map(|k| format!("%{k}")).collect::<Vec<_>>().join(".")
                        } else {
                            format!("%{}", level + 1)
                        };
                        (num_fmt(*format), format!("{prefix}{number}{suffix}"))
                    }
                    ListLabel::Bullet(b) => ("bullet", b.clone()),
                    ListLabel::None => ("none", String::new()),
                };
                let _ = write!(
                    out,
                    "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{format}\"/>\
                     <w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" \
                     w:hanging=\"{}\"/></w:pPr></w:lvl>",
                    attr(&text),
                    twips(def.label_indent + def.text_offset),
                    twips(def.text_offset)
                );
            }
            out.push_str("</w:abstractNum>");
        }
        for (id, abstract_id, start) in &self.instances {
            let _ = write!(out, "<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"{abstract_id}\"/>");
            if let Some(start) = start {
                for level in 0..9 {
                    let value = if level == 0 { *start } else { 1 };
                    let _ = write!(out, "<w:lvlOverride w:ilvl=\"{level}\"><w:startOverride w:val=\"{value}\"/></w:lvlOverride>");
                }
            }
            out.push_str("</w:num>");
        }
        out.push_str("</w:numbering>");
        out
    }
}

fn num_fmt(format: NumberFormat) -> &'static str {
    match format {
        NumberFormat::Decimal => "decimal",
        NumberFormat::UpperRoman => "upperRoman",
        NumberFormat::LowerRoman => "lowerRoman",
        NumberFormat::UpperLetter => "upperLetter",
        NumberFormat::LowerLetter => "lowerLetter",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_open_new_instances_with_overrides() {
        let mut n = Numbering::new();
        assert_eq!(n.num_id(7, Some(1)), 1);
        assert_eq!(n.num_id(7, None), 1);
        assert_eq!(n.num_id(7, Some(1)), 2);
        assert_eq!(n.instances, vec![(1, 0, None), (2, 0, Some(1))]);
    }
}
