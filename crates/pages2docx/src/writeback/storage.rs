//! Rebuilding the body text storage (`TSWP.StorageArchive`) from edited paragraphs.
//!
//! Every character and paragraph of the new text gets its attributes from the original it
//! corresponds to; new text takes them from its neighbours. Only objects the original storage
//! already references are used, so the storage's reference list stays within what the package
//! metadata knows about.

use std::collections::{HashMap, HashSet};

use iwork::{Encoder, Msg, Package};

use super::diff::align;
use super::docx_in::{Block as DocxBlock, Format, Para as DocxPara};
use super::{Report, Unsupported};
use crate::messages;
use crate::model::{Block, ListLabel};
use crate::reader::attach::{is_block_attachment, single_refs};
use crate::reader::kinds;
use crate::reader::styles::Styles;
use crate::reader::text::{AttrTable, Utf16Map, ends_paragraph, paragraph_ranges};

const CHARACTER_STYLE: u32 = 2021;
const LIST_STYLE: u32 = 2023;

/// Storage fields this module writes anew.
const REBUILT: &[u32] = &[3, 5, 6, 7, 8, 9, 11, 14, 19];

/// Attribute tables write-back does not rebuild. With entries only at index 0 they hold for
/// any text and are kept; with more, the text carries something write-back would destroy.
const KEPT_IF_TRIVIAL: &[(u32, &str)] = &[
    (12, messages::CONTENT_COLUMNS),
    (13, messages::CONTENT_PARAGRAPH_DATA),
    (15, messages::CONTENT_BOOKMARKS),
    (16, messages::CONTENT_FOOTNOTES),
    (17, messages::CONTENT_SECTIONS),
    (18, messages::CONTENT_RUBY),
    (20, messages::CONTENT_DICTATION),
    (21, messages::CONTENT_TRACKED_CHANGES),
    (22, messages::CONTENT_TRACKED_CHANGES),
    (23, messages::CONTENT_COMMENTS),
    (24, messages::CONTENT_BIDI),
    (25, messages::CONTENT_COMMENTS),
    (26, messages::CONTENT_ANNOTATIONS),
    (27, messages::CONTENT_TATECHUYOKO),
    (28, messages::CONTENT_DROP_CAPS),
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ParaAttrs {
    style: Option<u64>,
    /// `None` when the storage leaves the list style to the paragraph style.
    list_style: Option<u64>,
    level: u64,
    /// Raw entry of the list-start table where numbering restarts.
    list_start: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CharAttrs {
    style: Option<u64>,
    /// Smart field: a hyperlink or another field object.
    field: Option<u64>,
    attachment: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OutPara {
    attrs: ParaAttrs,
    chars: Vec<char>,
    char_attrs: Vec<CharAttrs>,
    /// Attributes of the paragraph mark that follows. Pages formats it like any character, and
    /// it ends the runs of character styles and links before it.
    mark: CharAttrs,
}

/// The original body storage, analysed.
pub(crate) struct Original<'p> {
    pkg: &'p Package,
    pub(crate) id: u64,
    msg: Msg<'p>,
    chars: Vec<char>,
    paragraphs: Vec<(usize, usize)>,
    para_styles: AttrTable<Option<u64>>,
    levels: AttrTable<(u64, u64)>,
    list_styles: AttrTable<Option<u64>>,
    char_styles: AttrTable<Option<u64>>,
    attachments: AttrTable<Option<u64>>,
    fields: AttrTable<Option<u64>>,
    list_starts: AttrTable<(u64, u64)>,
    language: Option<String>,
    refs: Vec<u64>,
    /// Character positions of attachments that are blocks of their own (tables, text boxes).
    block_positions: HashSet<usize>,
    /// Table or text-box storage id → position of its attachment.
    attachment_of: HashMap<u64, usize>,
}

impl<'p> Original<'p> {
    pub(crate) fn load(pkg: &'p Package, id: u64) -> Result<Self, Unsupported> {
        let msg = pkg.object(id).ok_or(Unsupported::LayoutDocument)?;
        for &(field, what) in KEPT_IF_TRIVIAL {
            let nontrivial = msg.msg(field).is_some_and(|t| t.msgs(1).iter().any(|e| e.uint(1).unwrap_or(0) != 0));
            if nontrivial {
                return Err(Unsupported::Content(what));
            }
        }
        let chars: Vec<char> = msg.strings(3).concat().chars().collect();
        let map = Utf16Map::new(&chars);
        let attachments = AttrTable::objects(msg.msg(9), &map);
        let mut block_positions = HashSet::new();
        let mut attachment_of = HashMap::new();
        for pos in attachments.within(0, chars.len()).collect::<Vec<_>>() {
            let Some(att) = attachments.object_at(pos) else { continue };
            if !is_block_attachment(pkg, att) {
                continue;
            }
            block_positions.insert(pos);
            if let Some(drawable) = pkg.object(att).and_then(|m| m.reference(1)) {
                attachment_of.insert(drawable, pos);
                if let Some(d) = pkg.object(drawable) {
                    for f in d.field_numbers().collect::<Vec<_>>() {
                        for r in single_refs(&d, f) {
                            if pkg.kind(r) == Some(kinds::STORAGE) {
                                attachment_of.insert(r, pos);
                            }
                        }
                    }
                }
            }
        }
        let language = msg.msg(19).and_then(|t| t.msgs(1).first().and_then(|e| e.string(2)));
        Ok(Self {
            pkg,
            id,
            paragraphs: paragraph_ranges(&chars),
            para_styles: AttrTable::objects(msg.msg(5), &map),
            levels: AttrTable::pairs(msg.msg(6), &map),
            list_styles: AttrTable::objects(msg.msg(7), &map),
            char_styles: AttrTable::objects(msg.msg(8), &map),
            fields: AttrTable::objects(msg.msg(11), &map),
            list_starts: AttrTable::pairs(msg.msg(14), &map),
            attachments,
            language,
            refs: pkg.references(id).to_vec(),
            block_positions,
            attachment_of,
            chars,
            msg,
        })
    }

    fn para_attrs(&self, k: usize) -> ParaAttrs {
        let start = self.paragraphs[k].0;
        ParaAttrs {
            style: self.para_styles.at_inherit(start),
            list_style: if self.list_styles.is_empty() { None } else { self.list_styles.at_inherit(start) },
            level: self.levels.at(start).map_or(0, |(l, _)| l),
            list_start: self.list_starts.exact(start).filter(|(flag, _)| *flag != 0),
        }
    }

    /// Attributes of the paragraph mark ending paragraph `k`; none after the last paragraph.
    fn mark_attrs(&self, k: usize) -> CharAttrs {
        let end = self.paragraphs[k].1;
        let is_mark = self.chars.get(end).is_some_and(|c| matches!(c, '\n' | '\u{2029}'));
        if is_mark { self.char_attrs(end) } else { CharAttrs::default() }
    }

    fn char_attrs(&self, i: usize) -> CharAttrs {
        CharAttrs {
            style: self.char_styles.object_at(i),
            field: self.fields.object_at(i),
            attachment: if self.chars[i] == '\u{FFFC}' { self.attachments.exact(i).flatten() } else { None },
        }
    }

    /// Ranges of paragraph `k` between its block attachments, as the reader splits it.
    fn pieces(&self, k: usize) -> Vec<(usize, usize)> {
        let (start, end) = self.paragraphs[k];
        let mut out = Vec::new();
        let mut from = start;
        for pos in start..end {
            if self.block_positions.contains(&pos) {
                out.push((from, pos));
                from = pos + 1;
            }
        }
        out.push((from, end));
        out
    }

    fn paragraph_of(&self, pos: usize) -> usize {
        self.paragraphs.partition_point(|&(_, end)| end <= pos).min(self.paragraphs.len() - 1)
    }

    /// The storage as it is, in the shape [`rebuild`] produces, to tell whether anything changed.
    pub(crate) fn identity(&self) -> Vec<OutPara> {
        (0..self.paragraphs.len())
            .map(|k| {
                let (start, end) = self.paragraphs[k];
                OutPara {
                    attrs: self.para_attrs(k),
                    chars: self.chars[start..end].to_vec(),
                    char_attrs: (start..end).map(|i| self.char_attrs(i)).collect(),
                    mark: self.mark_attrs(k),
                }
            })
            .collect()
    }

    fn referenced(&self, kind: u32) -> impl Iterator<Item = u64> + '_ {
        self.refs.iter().copied().filter(move |r| self.pkg.kind(*r) == Some(kind))
    }

    fn link_target(&self, field: u64) -> Option<String> {
        (self.pkg.kind(field) == Some(kinds::HYPERLINK)).then(|| self.pkg.object(field).and_then(|f| f.string(2))).flatten()
    }
}

/// Builds the new paragraphs from the edited document.
pub(crate) fn rebuild(
    orig: &Original,
    blocks0: &[Block],
    edited: &[DocxBlock],
    alignment: &[Option<usize>],
    styles: &mut Styles,
    report: &mut Report,
) -> Vec<OutPara> {
    let mut b = Builder { orig, styles, report, out: Vec::new(), open: None, emitted_attachments: HashSet::new() };
    for (j, block) in edited.iter().enumerate() {
        let original = alignment.get(j).copied().flatten().map(|i| &blocks0[i]);
        match (block, original) {
            (DocxBlock::Para(p1), Some(Block::Paragraph(p0))) => match p0.origin {
                Some(o) if o.storage == orig.id => {
                    let ranges = orig.pieces(o.paragraph);
                    let range = ranges.get(o.piece).copied().unwrap_or(orig.paragraphs[o.paragraph]);
                    if p0.plain_text() != p1.text() {
                        b.report.paragraphs_changed += 1;
                    }
                    b.piece(o.paragraph, o.piece, range, p1);
                }
                Some(o) => {
                    // A paragraph of a text box: keep the box, edits inside are not written.
                    if p0.plain_text() != p1.text() {
                        b.report.skip(messages::NOT_WRITTEN_TEXTBOX);
                    }
                    b.block_attachment(o.storage);
                }
                None => b.new_paragraph(p1),
            },
            (DocxBlock::Para(p1), _) => {
                b.report.paragraphs_changed += 1;
                b.new_paragraph(p1);
            }
            (DocxBlock::Table(_), Some(Block::Table(t0))) => match t0.source {
                Some(source) if orig.attachment_of.contains_key(&source) => b.block_attachment(source),
                _ => b.report.skip(messages::NOT_WRITTEN_TABLE),
            },
            (DocxBlock::Table(_), _) => b.report.skip(messages::NOT_WRITTEN_TABLE),
        }
    }
    let mut out = b.out;
    if out.is_empty() {
        out.push(OutPara { attrs: orig.para_attrs(0), ..OutPara::default() });
    }
    // An explicit list style anywhere means every paragraph needs its effective one.
    if out.iter().any(|p| p.attrs.list_style.is_some()) {
        for p in &mut out {
            if p.attrs.list_style.is_none() {
                p.attrs.list_style = styles.para_list_style(p.attrs.style);
            }
        }
    }
    out
}

struct Builder<'a, 'p, 's> {
    orig: &'a Original<'p>,
    styles: &'a mut Styles<'s>,
    report: &'a mut Report,
    out: Vec<OutPara>,
    /// Original paragraph and last piece of the paragraph being built, which a following
    /// piece or attachment of the same original paragraph continues.
    open: Option<(usize, Option<usize>)>,
    emitted_attachments: HashSet<usize>,
}

impl Builder<'_, '_, '_> {
    fn piece(&mut self, k: usize, piece: usize, range: (usize, usize), p1: &DocxPara) {
        let continues = matches!(self.open, Some((ok, last)) if ok == k && last.is_none_or(|l| piece > l));
        if !continues {
            let mut attrs = self.orig.para_attrs(k);
            self.apply_list(&mut attrs, p1.list_level);
            self.out.push(OutPara { attrs, mark: self.orig.mark_attrs(k), ..OutPara::default() });
        }
        let attrs = self.out.last().expect("pushed above").attrs;
        let (chars, char_attrs) = self.map_chars(range, p1, attrs.style);
        let para = self.out.last_mut().expect("pushed above");
        para.chars.extend(chars);
        para.char_attrs.extend(char_attrs);
        self.open = Some((k, Some(piece)));
    }

    fn new_paragraph(&mut self, p1: &DocxPara) {
        let mut attrs = self
            .out
            .last()
            .map_or_else(|| self.orig.para_attrs(0), |p| ParaAttrs { list_start: None, ..p.attrs });
        self.apply_list(&mut attrs, p1.list_level);
        let (chars, char_attrs) = self.map_chars((0, 0), p1, attrs.style);
        self.out.push(OutPara { attrs, chars, char_attrs, mark: CharAttrs::default() });
        self.open = None;
    }

    /// Puts the attachment of a table or text box back at its place in the text.
    fn block_attachment(&mut self, source: u64) {
        let Some(&pos) = self.orig.attachment_of.get(&source) else { return };
        if !self.emitted_attachments.insert(pos) {
            return;
        }
        let k = self.orig.paragraph_of(pos);
        let pieces_before = self.orig.pieces(k).iter().take_while(|(_, end)| *end <= pos).count();
        let piece = pieces_before.saturating_sub(1);
        let continues = matches!(self.open, Some((ok, last)) if ok == k && last.is_none_or(|l| piece >= l));
        if !continues {
            self.out.push(OutPara { attrs: self.orig.para_attrs(k), mark: self.orig.mark_attrs(k), ..OutPara::default() });
        }
        let para = self.out.last_mut().expect("pushed above");
        para.chars.push('\u{FFFC}');
        para.char_attrs.push(self.orig.char_attrs(pos));
        self.open = Some((k, Some(piece)));
    }

    /// The new characters with attributes: from the original character each one corresponds
    /// to, otherwise from the preceding one; formatting and links then follow the edit.
    fn map_chars(&mut self, range: (usize, usize), p1: &DocxPara, para_style: Option<u64>) -> (Vec<char>, Vec<CharAttrs>) {
        let orig = self.orig;
        let original = &orig.chars[range.0..range.1];
        let mapping = align(original, &p1.chars, |a, b| a == b, |_, _| false);
        let mark = self.styles.char_props(para_style);
        let mut chars = Vec::with_capacity(p1.chars.len());
        let mut attrs: Vec<CharAttrs> = Vec::with_capacity(p1.chars.len());
        let mut format_lost = false;
        for (i, &c) in p1.chars.iter().enumerate() {
            let base = mapping[i].map(|o| orig.char_attrs(range.0 + o));
            if c == '\u{FFFC}' && base.and_then(|b| b.attachment).is_none() {
                self.report.skip(messages::NOT_WRITTEN_OBJECT);
                continue;
            }
            let inherited_style = base.map_or_else(
                || attrs.last().map(|a| a.style).unwrap_or_else(|| next_style(orig, &mapping, i, range.0)),
                |b| b.style,
            );
            let (format, link) = p1.char_format(i);
            let field = self.field_for(base.and_then(|b| b.field), link);
            let style = self.style_for(inherited_style, &mark, format, field, &mut format_lost);
            chars.push(c);
            attrs.push(CharAttrs { style, field, attachment: base.and_then(|b| b.attachment) });
        }
        if format_lost {
            self.report.skip(messages::NOT_WRITTEN_FORMAT);
        }
        (chars, attrs)
    }

    /// The smart field a character keeps or gets: fields other than links stay as they were,
    /// links follow the edited document where the original has a link with that address.
    fn field_for(&mut self, base: Option<u64>, link: Option<&str>) -> Option<u64> {
        let orig = self.orig;
        let base_target = base.and_then(|f| orig.link_target(f));
        if base.is_some() && base_target.is_none() {
            return base;
        }
        match link {
            None => None,
            Some(url) if base_target.as_deref() == Some(url) => base,
            Some(url) => {
                let existing = orig.referenced(kinds::HYPERLINK).find(|f| orig.link_target(*f).as_deref() == Some(url));
                if existing.is_none() {
                    self.report.skip(messages::NOT_WRITTEN_LINK);
                }
                existing
            }
        }
    }

    /// Keeps `style` if it gives the edited formatting, otherwise picks a character style the
    /// storage already uses that does.
    fn style_for(
        &mut self,
        style: Option<u64>,
        mark: &crate::model::CharProps,
        wanted: Format,
        field: Option<u64>,
        lost: &mut bool,
    ) -> Option<u64> {
        let orig = self.orig;
        // Word underlines links by itself; that underline says nothing about the text's style.
        let compare = |f: Format| if field.is_some() { Format { underline: wanted.underline, ..f } } else { f };
        let mut format_of = |s: Option<u64>| {
            let props = match s {
                Some(_) => mark.overlaid(&self.styles.char_props(s)),
                None => mark.clone(),
            };
            compare(Format {
                bold: props.is_bold(),
                italic: props.italic.unwrap_or(false),
                underline: props.underline.is_some_and(|u| u != crate::model::Underline::None),
                strike: props.strike.is_some_and(|s| s != crate::model::Strike::None),
            })
        };
        if format_of(style) == wanted {
            return style;
        }
        let candidates: Vec<Option<u64>> = std::iter::once(None).chain(orig.referenced(CHARACTER_STYLE).map(Some)).collect();
        if let Some(found) = candidates.into_iter().find(|c| format_of(*c) == wanted) {
            return found;
        }
        *lost = true;
        style
    }

    fn is_list(&mut self, list_style: Option<u64>, level: u64) -> bool {
        let Some(id) = list_style else { return false };
        let style = self.styles.list_style(id);
        let level = usize::try_from(level).unwrap_or(0);
        style.levels.get(level).is_some_and(|l| l.label != ListLabel::None)
    }

    /// Makes the paragraph a list item at `level`, or no list item, as in the edited document.
    fn apply_list(&mut self, attrs: &mut ParaAttrs, level: Option<u8>) {
        let effective = attrs.list_style.or_else(|| self.styles.para_list_style(attrs.style));
        match level {
            Some(level) => {
                let level = u64::from(level);
                if self.is_list(effective, level) {
                    attrs.level = level;
                    return;
                }
                let previous: Vec<Option<u64>> = self.out.iter().rev().map(|p| p.attrs.list_style).collect();
                let from_document: Vec<Option<u64>> = self.orig.referenced(LIST_STYLE).map(Some).collect();
                match previous.into_iter().chain(from_document).find(|s| self.is_list(*s, level)) {
                    Some(style) => {
                        attrs.list_style = style;
                        attrs.level = level;
                    }
                    None => self.report.skip(messages::NOT_WRITTEN_LIST),
                }
            }
            None if self.is_list(effective, attrs.level) => {
                let plain: Vec<u64> = self.orig.referenced(LIST_STYLE).collect();
                match plain.into_iter().find(|s| !self.is_list(Some(*s), 0)) {
                    Some(style) => {
                        attrs.list_style = Some(style);
                        attrs.list_start = None;
                    }
                    None => self.report.skip(messages::NOT_WRITTEN_LIST),
                }
            }
            None => {}
        }
    }
}

fn next_style(orig: &Original, mapping: &[Option<usize>], from: usize, offset: usize) -> Option<u64> {
    mapping[from..].iter().flatten().next().and_then(|o| orig.char_styles.object_at(offset + o))
}

/// The encoded storage and the objects it references.
pub(crate) struct Encoded {
    pub(crate) payload: Vec<u8>,
    pub(crate) references: Vec<u64>,
    pub(crate) text: String,
}

fn is_break(c: char) -> bool {
    ends_paragraph(c) && !matches!(c, '\n' | '\u{2029}')
}

pub(crate) fn encode(orig: &Original, paragraphs: &[OutPara]) -> Encoded {
    let mut text = String::new();
    let mut utf16 = 0u64;
    let mut starts = Vec::with_capacity(paragraphs.len());
    let mut char_entries: Vec<(u64, CharAttrs)> = Vec::new();
    for (i, p) in paragraphs.iter().enumerate() {
        starts.push(utf16);
        for (c, a) in p.chars.iter().zip(&p.char_attrs) {
            char_entries.push((utf16, *a));
            text.push(*c);
            utf16 += c.len_utf16() as u64;
        }
        if i + 1 < paragraphs.len() && !p.chars.last().is_some_and(|c| is_break(*c)) {
            char_entries.push((utf16, CharAttrs { attachment: None, ..p.mark }));
            text.push('\n');
            utf16 += 1;
        }
    }

    let mut e = Encoder::new();
    e.copy_except(&orig.msg, REBUILT);
    e.string(3, &text);
    let mut used: Vec<u64> = Vec::new();
    let mut note = |id: Option<u64>| {
        if let Some(id) = id {
            if !used.contains(&id) {
                used.push(id);
            }
        }
    };

    // Paragraph styles: an entry at every paragraph start, the object only where it changes.
    let mut table = Vec::new();
    let mut previous = None;
    for (p, &start) in paragraphs.iter().zip(&starts) {
        let changed = p.attrs.style != previous || start == 0;
        table.push(object_entry(start, if changed { p.attrs.style } else { None }));
        if changed {
            note(p.attrs.style);
        }
        previous = p.attrs.style;
    }
    e.bytes(5, &table_message(table));

    let levels: Vec<(u64, u64)> = paragraphs.iter().zip(&starts).map(|(p, s)| (*s, p.attrs.level)).collect();
    e.bytes(6, &table_message(runs(&levels).into_iter().map(|(at, level)| pair_entry(at, level, 0)).collect()));

    if paragraphs.iter().any(|p| p.attrs.list_style.is_some()) {
        let lists: Vec<(u64, Option<u64>)> = paragraphs.iter().zip(&starts).map(|(p, s)| (*s, p.attrs.list_style)).collect();
        let entries = runs(&lists);
        entries.iter().for_each(|(_, s)| note(*s));
        e.bytes(7, &table_message(entries.into_iter().map(|(at, s)| object_entry(at, s)).collect()));
    }

    let styles: Vec<(u64, Option<u64>)> = char_entries.iter().map(|(at, a)| (*at, a.style)).collect();
    let style_runs = runs_from_zero(&styles);
    style_runs.iter().for_each(|(_, s)| note(*s));
    e.bytes(8, &table_message(style_runs.into_iter().map(|(at, s)| object_entry(at, s)).collect()));

    let attachments: Vec<Vec<u8>> =
        char_entries.iter().filter_map(|(at, a)| a.attachment.map(|id| (*at, id))).map(|(at, id)| object_entry(at, Some(id))).collect();
    char_entries.iter().for_each(|(_, a)| note(a.attachment));
    if !attachments.is_empty() {
        e.bytes(9, &table_message(attachments));
    }

    let fields: Vec<(u64, Option<u64>)> = char_entries.iter().map(|(at, a)| (*at, a.field)).collect();
    let field_runs = runs_from_zero(&fields);
    field_runs.iter().for_each(|(_, f)| note(*f));
    if field_runs.iter().any(|(_, f)| f.is_some()) || orig.msg.has(11) {
        e.bytes(11, &table_message(field_runs.into_iter().map(|(at, f)| object_entry(at, f)).collect()));
    }

    // List restarts: the restarting paragraph's entry, then a neutral one at the next start.
    let mut restarts = vec![pair_entry(0, 0, 0)];
    for (i, (p, &start)) in paragraphs.iter().zip(&starts).enumerate() {
        if let Some((flag, value)) = p.attrs.list_start {
            if start == 0 {
                restarts.clear();
            }
            restarts.push(pair_entry(start, flag, value));
            if let Some(&next) = starts.get(i + 1) {
                if paragraphs[i + 1].attrs.list_start.is_none() {
                    restarts.push(pair_entry(next, 0, 0));
                }
            }
        }
    }
    e.bytes(14, &table_message(restarts));

    if let Some(language) = &orig.language {
        let mut entry = Encoder::new();
        entry.uint(1, 0).string(2, language);
        e.bytes(19, &table_message(vec![entry.finish()]));
    }

    // References: everything the original referenced outside the rebuilt tables, plus what the
    // rebuilt tables use now.
    let rebuilt_before: HashSet<u64> = [&orig.para_styles, &orig.list_styles, &orig.char_styles, &orig.attachments, &orig.fields]
        .iter()
        .flat_map(|t| t.values().iter().flatten().copied())
        .collect();
    let mut references: Vec<u64> = orig.refs.iter().copied().filter(|r| !rebuilt_before.contains(r)).collect();
    for id in used {
        if !references.contains(&id) {
            references.push(id);
        }
    }
    Encoded { payload: e.finish(), references, text }
}

/// Entries where the value changes, starting with one at the first position.
fn runs<T: Copy + PartialEq>(values: &[(u64, T)]) -> Vec<(u64, T)> {
    let mut out: Vec<(u64, T)> = Vec::new();
    for &(at, v) in values {
        if out.last().is_none_or(|(_, last)| *last != v) {
            out.push((at, v));
        }
    }
    out
}

/// Like [`runs`], but always with an entry at index 0, as Pages writes these tables.
fn runs_from_zero(values: &[(u64, Option<u64>)]) -> Vec<(u64, Option<u64>)> {
    let mut out = vec![(0, values.first().and_then(|(_, v)| *v))];
    for &(at, v) in values.iter().skip(1) {
        if out.last().is_some_and(|(_, last)| *last != v) {
            out.push((at, v));
        }
    }
    out
}

fn object_entry(at: u64, object: Option<u64>) -> Vec<u8> {
    let mut e = Encoder::new();
    e.uint(1, at);
    if let Some(id) = object {
        e.reference(2, id);
    }
    e.finish()
}

fn pair_entry(at: u64, first: u64, second: u64) -> Vec<u8> {
    let mut e = Encoder::new();
    e.uint(1, at).uint(2, first).uint(3, second);
    e.finish()
}

fn table_message(entries: Vec<Vec<u8>>) -> Vec<u8> {
    let mut e = Encoder::new();
    for entry in &entries {
        e.bytes(1, entry);
    }
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-encoding the unchanged storage of a real document must reproduce every attribute of
    /// every character and paragraph, and the same references.
    #[test]
    fn encoding_the_original_reproduces_its_attributes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/Cuba hoy.pages");
        if !path.is_file() {
            eprintln!("reference document missing, skipped");
            return;
        }
        let pkg = Package::open(&path).unwrap();
        let doc = pkg.object(pkg.ids_of_kind(kinds::DOCUMENT)[0]).unwrap();
        let orig = Original::load(&pkg, doc.reference(4).unwrap()).unwrap();
        let encoded = encode(&orig, &orig.identity());
        let msg = Msg::parse(&encoded.payload).unwrap();
        assert_eq!(msg.strings(3).concat(), orig.chars.iter().collect::<String>());

        let map = Utf16Map::new(&orig.chars);
        let objects = |n| AttrTable::objects(msg.msg(n), &map);
        let (char_styles, fields, attachments) = (objects(8), objects(11), objects(9));
        for i in 0..orig.chars.len() {
            assert_eq!(char_styles.object_at(i), orig.char_styles.object_at(i), "character style at {i}");
            assert_eq!(fields.object_at(i), orig.fields.object_at(i), "link at {i}");
            assert_eq!(attachments.exact(i).flatten(), orig.attachments.exact(i).flatten(), "attachment at {i}");
        }
        let (para_styles, list_styles) = (objects(5), objects(7));
        let (levels, starts) = (AttrTable::pairs(msg.msg(6), &map), AttrTable::pairs(msg.msg(14), &map));
        for &(start, _) in &orig.paragraphs {
            assert_eq!(para_styles.at_inherit(start), orig.para_styles.at_inherit(start), "paragraph style at {start}");
            assert_eq!(list_styles.at_inherit(start), orig.list_styles.at_inherit(start), "list style at {start}");
            assert_eq!(levels.at(start).map(|l| l.0), orig.levels.at(start).map(|l| l.0), "list level at {start}");
            let restart = |t: &AttrTable<(u64, u64)>| t.exact(start).filter(|(flag, _)| *flag != 0);
            assert_eq!(restart(&starts), restart(&orig.list_starts), "list restart at {start}");
        }
        let mut before = orig.refs.clone();
        let mut after = encoded.references.clone();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(after, before);
    }
}
