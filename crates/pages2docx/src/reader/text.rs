//! Text storages (`TSWP.StorageArchive`): the characters and the attribute tables that place
//! paragraph styles, character styles, lists, links and attachments on them.

use std::collections::BTreeSet;

use iwork::Msg;

use super::attach::Attached;
use super::{Ctx, Part, Reader, kinds};
use crate::model::{Block, CharProps, Inline, InlineKind, ListItem, Paragraph};

/// Maps Pages' UTF-16 code unit indices (it is a Cocoa app) to indices into a `char` slice.
pub(super) struct Utf16Map {
    map: Option<Vec<usize>>,
    len: usize,
}

impl Utf16Map {
    pub(super) fn new(chars: &[char]) -> Self {
        if chars.iter().all(|c| c.len_utf16() == 1) {
            return Self { map: None, len: chars.len() };
        }
        let mut map = Vec::with_capacity(chars.len() + 8);
        for (i, c) in chars.iter().enumerate() {
            map.push(i);
            if c.len_utf16() == 2 {
                map.push(i + 1);
            }
        }
        map.push(chars.len());
        Self { map: Some(map), len: chars.len() }
    }

    /// For tables whose indices are only compared with 0.
    pub(super) fn identity() -> Self {
        Self { map: None, len: usize::MAX }
    }

    fn to_char(&self, index: u64) -> usize {
        let i = usize::try_from(index).unwrap_or(usize::MAX);
        match &self.map {
            None => i.min(self.len),
            Some(map) => map.get(i).copied().unwrap_or(self.len),
        }
    }
}

/// A character-indexed attribute table (`TSWP.ObjectAttributeTable` and relatives): each entry
/// holds from its index until the next entry.
pub(super) struct AttrTable<T> {
    idx: Vec<usize>,
    vals: Vec<T>,
}

impl<T: Copy> AttrTable<T> {
    fn build(m: Option<Msg>, map: &Utf16Map, value: impl Fn(&Msg) -> T) -> Self {
        let entries = m.map(|m| m.msgs(1)).unwrap_or_default();
        let mut rows: Vec<(usize, T)> =
            entries.iter().map(|e| (map.to_char(e.uint(1).unwrap_or(0)), value(e))).collect();
        rows.sort_by_key(|r| r.0);
        Self { idx: rows.iter().map(|r| r.0).collect(), vals: rows.into_iter().map(|r| r.1).collect() }
    }

    fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    fn at(&self, i: usize) -> Option<T> {
        let k = self.idx.partition_point(|&x| x <= i);
        (k > 0).then(|| self.vals[k - 1])
    }

    fn exact(&self, i: usize) -> Option<T> {
        let k = self.idx.partition_point(|&x| x < i);
        (k < self.idx.len() && self.idx[k] == i).then(|| self.vals[k])
    }

    fn within(&self, lo: usize, hi: usize) -> impl Iterator<Item = usize> + '_ {
        self.idx.iter().copied().filter(move |&x| lo <= x && x < hi)
    }
}

impl AttrTable<Option<u64>> {
    pub(super) fn objects(m: Option<Msg>, map: &Utf16Map) -> Self {
        Self::build(m, map, |e| e.reference(2))
    }

    fn object_at(&self, i: usize) -> Option<u64> {
        self.at(i).flatten()
    }

    /// Like [`Self::object_at`], but an entry without an object continues the previous one.
    /// Paragraph style tables mark every paragraph start this way.
    pub(super) fn at_inherit(&self, i: usize) -> Option<u64> {
        let mut k = self.idx.partition_point(|&x| x <= i);
        while k > 0 {
            if let Some(v) = self.vals[k - 1] {
                return Some(v);
            }
            k -= 1;
        }
        None
    }
}

impl AttrTable<(u64, u64)> {
    fn pairs(m: Option<Msg>, map: &Utf16Map) -> Self {
        Self::build(m, map, |e| (e.uint(2).unwrap_or(0), e.uint(3).unwrap_or(0)))
    }
}

/// The attribute tables of one storage, by `TSWP.StorageArchive` field.
struct Tables {
    para_styles: AttrTable<Option<u64>>,  // 5
    list_levels: AttrTable<(u64, u64)>,   // 6: list level in the first value
    list_styles: AttrTable<Option<u64>>,  // 7
    char_styles: AttrTable<Option<u64>>,  // 8: entries without object end a run
    attachments: AttrTable<Option<u64>>,  // 9
    smart_fields: AttrTable<Option<u64>>, // 11: hyperlinks among others
    list_starts: AttrTable<(u64, u64)>,   // 14: first value 1 where a list starts again
}

/// Characters that end a paragraph; only the first two are not shown as a break.
fn ends_paragraph(c: char) -> bool {
    matches!(c, '\n' | '\u{2029}' | '\u{0C}' | '\u{04}' | '\u{0E}')
}

impl Reader<'_> {
    /// Reads a text storage into blocks.
    pub(super) fn read_storage(&mut self, id: u64, ctx: Ctx) -> Vec<Block> {
        if !self.reading.insert(id) {
            return Vec::new();
        }
        let blocks = self.read_storage_unguarded(id, ctx);
        self.reading.remove(&id);
        blocks
    }

    fn read_storage_unguarded(&mut self, id: u64, ctx: Ctx) -> Vec<Block> {
        let Some(st) = self.pkg.object(id) else { return Vec::new() };
        let chars: Vec<char> = st.strings(3).concat().chars().collect();
        let map = Utf16Map::new(&chars);
        let tables = Tables {
            para_styles: AttrTable::objects(st.msg(5), &map),
            list_levels: AttrTable::pairs(st.msg(6), &map),
            list_styles: AttrTable::objects(st.msg(7), &map),
            char_styles: AttrTable::objects(st.msg(8), &map),
            attachments: AttrTable::objects(st.msg(9), &map),
            smart_fields: AttrTable::objects(st.msg(11), &map),
            list_starts: AttrTable::pairs(st.msg(14), &map),
        };
        let mut blocks = Vec::new();
        let mut start = 0;
        for (i, &c) in chars.iter().enumerate() {
            if ends_paragraph(c) {
                let end = if matches!(c, '\n' | '\u{2029}') { i } else { i + 1 };
                blocks.extend(self.read_paragraph(&chars, start, end, ctx, &tables));
                start = i + 1;
            }
        }
        blocks.extend(self.read_paragraph(&chars, start, chars.len(), ctx, &tables));
        blocks
    }

    fn read_paragraph(&mut self, chars: &[char], start: usize, end: usize, ctx: Ctx, t: &Tables) -> Vec<Block> {
        let pkg = self.pkg;
        let style = t.para_styles.at_inherit(start);
        let props = self.styles.para_props(style);
        let mark = self.styles.char_props(style);
        let kind = self.styles.kind(style);

        let mut list = None;
        if ctx.part != Part::Header {
            let list_style =
                if t.list_styles.is_empty() { self.styles.para_list_style(style) } else { t.list_styles.at_inherit(start) };
            let level = t.list_levels.at(start).map_or(0, |(l, _)| l.min(8));
            if let Some(ls) = list_style {
                let style_def = self.lists.entry(ls).or_insert_with(|| self.styles.list_style(ls));
                let is_list = style_def
                    .levels
                    .get(usize::try_from(level).unwrap_or(0))
                    .is_some_and(|l| l.label != crate::model::ListLabel::None);
                if is_list {
                    let restart = t
                        .list_starts
                        .exact(start)
                        .filter(|(flag, _)| *flag != 0)
                        .map(|(_, value)| u32::try_from(value).ok().filter(|v| *v > 0).unwrap_or(1));
                    list = Some(ListItem { style: ls, level: u8::try_from(level).unwrap_or(0), restart });
                }
            }
        }

        let mut bounds: BTreeSet<usize> = [start, end].into();
        bounds.extend(t.char_styles.within(start + 1, end));
        bounds.extend(t.smart_fields.within(start + 1, end));
        for a in t.attachments.within(start, end) {
            bounds.extend([a, a + 1]);
        }
        let bounds: Vec<usize> = bounds.into_iter().filter(|b| (start..=end).contains(b)).collect();

        // Runs between block-level attachments (tables, text boxes) and the blocks after each.
        let mut pieces: Vec<Vec<Inline>> = vec![Vec::new()];
        let mut blocks_after: Vec<Vec<Block>> = Vec::new();
        for pair in bounds.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a >= b {
                continue;
            }
            let char_style = t.char_styles.object_at(a);
            let run = match char_style {
                Some(_) => mark.overlaid(&self.styles.char_props(char_style)),
                None => mark.clone(),
            };
            let link = if ctx.part == Part::Header {
                None
            } else {
                t.smart_fields
                    .object_at(a)
                    .filter(|f| pkg.kind(*f) == Some(kinds::HYPERLINK))
                    .and_then(|f| pkg.object(f))
                    .and_then(|f| f.string(2))
            };
            if b - a == 1 && chars[a] == '\u{FFFC}' {
                match self.attachment(t.attachments.exact(a).flatten(), ctx) {
                    Attached::Inline(kinds) => {
                        let current = pieces.last_mut().expect("pieces is never empty");
                        current.extend(kinds.into_iter().map(|kind| Inline { kind, props: run.clone(), link: link.clone() }));
                    }
                    Attached::Blocks(blocks) => {
                        blocks_after.push(blocks);
                        pieces.push(Vec::new());
                    }
                    Attached::Nothing => {}
                }
                continue;
            }
            push_text(pieces.last_mut().expect("pieces is never empty"), &chars[a..b], &run, link.as_ref());
        }

        let make = |inlines: Vec<Inline>, list: Option<ListItem>| {
            Block::Paragraph(Paragraph { kind, props: props.clone(), mark: mark.clone(), list, inlines })
        };
        if blocks_after.is_empty() {
            return vec![make(pieces.pop().unwrap_or_default(), list)];
        }
        let mut out = Vec::new();
        let mut after = blocks_after.into_iter();
        for piece in pieces {
            if !piece.is_empty() {
                out.push(make(piece, list.take()));
            }
            if let Some(blocks) = after.next() {
                out.extend(blocks);
            }
        }
        out
    }
}

/// Turns characters into text runs and the special characters into breaks and tabs.
fn push_text(out: &mut Vec<Inline>, chars: &[char], props: &CharProps, link: Option<&String>) {
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            out.push(Inline { kind: InlineKind::Text(std::mem::take(text)), props: props.clone(), link: link.cloned() });
        }
    };
    for &c in chars {
        let special = match c {
            '\t' => InlineKind::Tab,
            '\u{2028}' | '\u{0B}' => InlineKind::LineBreak,
            '\u{0C}' | '\u{04}' => InlineKind::PageBreak,
            '\u{0E}' => InlineKind::ColumnBreak,
            '\u{AD}' => InlineKind::SoftHyphen,
            '\u{FFFC}' | '\u{FFFE}' | '\u{FFFF}' => continue,
            c if c < ' ' => continue,
            c => {
                text.push(c);
                continue;
            }
        };
        flush(&mut text, out);
        out.push(Inline { kind: special, props: props.clone(), link: link.cloned() });
    }
    flush(&mut text, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_utf16_indices_past_astral_characters() {
        let chars: Vec<char> = "a😀b".chars().collect();
        let map = Utf16Map::new(&chars);
        assert_eq!(map.to_char(0), 0);
        assert_eq!(map.to_char(3), 2); // 'b' follows the two code units of the emoji
        assert_eq!(map.to_char(4), 3);
    }

    #[test]
    fn special_characters_become_breaks() {
        let mut out = Vec::new();
        let chars: Vec<char> = "a\tb\u{2028}c\u{FFFC}".chars().collect();
        push_text(&mut out, &chars, &CharProps::default(), None);
        let kinds: Vec<&InlineKind> = out.iter().map(|i| &i.kind).collect();
        assert_eq!(
            kinds,
            [
                &InlineKind::Text("a".into()),
                &InlineKind::Tab,
                &InlineKind::Text("b".into()),
                &InlineKind::LineBreak,
                &InlineKind::Text("c".into())
            ]
        );
    }
}
