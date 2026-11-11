//! Reads a Pages document (Pages 5 / 2013 and later) into the [`model`](crate::model).
//!
//! Apple documents none of this. Every field number below was established from sample
//! documents: by changing one setting in Pages and seeing which field changed, or by matching a
//! document's objects against Pages' own rendering of it (PDF export and preview images).

pub(crate) mod attach;
pub(crate) mod styles;
mod table;
pub(crate) mod text;

use std::collections::{BTreeMap, HashSet};

use iwork::{Msg, Package};

use crate::messages;
use crate::model::{CharProps, Document, HeaderFooter, Image, ListStyle, PageSetup, Report};
use styles::Styles;
use text::{AttrTable, Utf16Map};

/// IWA message types used by Pages.
pub(crate) mod kinds {
    pub const DOCUMENT: u32 = 10000;
    pub const STORAGE: u32 = 2001;
    pub const DRAWABLE_ATTACHMENT: u32 = 2003;
    pub const PARAGRAPH_STYLE: u32 = 2022;
    pub const HYPERLINK: u32 = 2032;
    pub const TABLE_INFO: u32 = 6000;
    pub const DATA_LIST: u32 = 6005;
}

/// `TSWP.StorageArchive.kind` of a free text box.
const STORAGE_KIND_TEXTBOX: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Body,
    Cell,
    /// Headers and footers: no tables, pictures or links, attachments are page numbers.
    Header,
}

/// Where a storage is being read and how wide its content may become (points).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ctx {
    pub part: Part,
    pub max_width: f64,
}

pub(crate) struct Reader<'p> {
    pkg: &'p Package,
    styles: Styles<'p>,
    report: Report,
    lists: BTreeMap<u64, ListStyle>,
    images: Vec<Image>,
    /// Storages being read; drawables point back at the storage they sit in.
    reading: HashSet<u64>,
    decimal_comma: bool,
}

/// Reads the whole document. Fails only when the package holds no Pages document at all.
pub(crate) fn read(pkg: &Package, title: &str) -> Result<(Document, Report), crate::Error> {
    let doc_id = *pkg.ids_of_kind(kinds::DOCUMENT).first().ok_or(crate::Error::NoDocument)?;
    let doc = pkg.object(doc_id).ok_or(crate::Error::NoDocument)?;
    let mut reader = Reader {
        pkg,
        styles: Styles::new(pkg),
        report: Report::default(),
        lists: BTreeMap::new(),
        images: Vec::new(),
        reading: HashSet::new(),
        decimal_comma: true,
    };
    let page = page_setup(&doc);
    let locale = doc.msg(15).and_then(|m| m.msg(1)).and_then(|m| m.string(4)).unwrap_or_default();
    reader.decimal_comma = !["en", "ja", "zh", "ko", "he", "th"].iter().any(|l| locale.starts_with(l));

    let body_ctx = Ctx { part: Part::Body, max_width: page.text_width() };
    let body_id = doc.reference(4);
    let mut body = Vec::new();
    let (mut header, mut footer) = (None, None);
    match body_id.filter(|id| pkg.kind(*id) == Some(kinds::STORAGE)) {
        Some(id) => {
            body = reader.read_storage(id, body_ctx);
            if let Some(storage) = pkg.object(id) {
                (header, footer) = reader.header_footer(&storage, page.text_width());
            }
        }
        None => {
            // Page layout documents have no running text, only text boxes placed on pages.
            reader.report.notes.push(messages::NOTE_NO_BODY.to_owned());
            for id in pkg.ids_of_kind(kinds::STORAGE) {
                let Some(st) = pkg.object(id) else { continue };
                if st.uint(1) == Some(STORAGE_KIND_TEXTBOX) && !st.strings(3).concat().trim().is_empty() {
                    body.extend(reader.read_storage(id, body_ctx));
                }
            }
        }
    }
    let defaults = reader.default_char_props(body_id);
    let document = Document {
        title: title.to_owned(),
        page,
        defaults,
        body,
        header,
        footer,
        lists: reader.lists,
        images: reader.images,
    };
    Ok((document, reader.report))
}

/// Page size and margins of `TP.DocumentArchive`, in points.
fn page_setup(doc: &Msg) -> PageSetup {
    let d = PageSetup::default();
    let f = |n, default| doc.float(n).filter(|v| *v > 0.0).unwrap_or(default);
    PageSetup {
        width: f(30, d.width),
        height: f(31, d.height),
        margin_left: doc.float(32).unwrap_or(d.margin_left),
        margin_right: doc.float(33).unwrap_or(d.margin_right),
        margin_top: doc.float(34).unwrap_or(d.margin_top),
        margin_bottom: doc.float(35).unwrap_or(d.margin_bottom),
        header_margin: f(36, d.header_margin),
        footer_margin: f(37, d.footer_margin),
    }
}

impl<'p> Reader<'p> {
    /// The template's body text style: Word's document defaults are taken from it.
    fn default_char_props(&mut self, body_id: Option<u64>) -> CharProps {
        let pkg = self.pkg;
        let body_style = pkg.ids_of_kind(kinds::PARAGRAPH_STYLE).into_iter().find(|id| {
            pkg.object(*id)
                .and_then(|s| s.msg(1))
                .and_then(|sup| sup.string(2))
                .is_some_and(|ident| ident.ends_with("paragraphstyle-Body 1") || ident.ends_with("paragraphstyle-Body"))
        });
        let style = body_style.or_else(|| {
            let storage = body_id.and_then(|id| pkg.object(id))?;
            let chars = Utf16Map::identity();
            AttrTable::objects(storage.msg(5), &chars).at_inherit(0)
        });
        self.styles.char_props(style)
    }

    /// Headers and footers of the first section (`TP.SectionArchive`), if any has content.
    fn header_footer(&mut self, body: &Msg, text_width: f64) -> (Option<HeaderFooter>, Option<HeaderFooter>) {
        let pkg = self.pkg;
        let section_id = AttrTable::objects(body.msg(17), &Utf16Map::identity()).at_inherit(0);
        let Some(section) = section_id.and_then(|id| pkg.object(id)) else { return (None, None) };
        let ctx = Ctx { part: Part::Header, max_width: text_width };
        let mut result = [None, None];
        for (slot, part_field) in result.iter_mut().zip([1u32, 2]) {
            // Three page masters (fields 23-25); the first one with content becomes Word's default.
            for master_field in [23u32, 24, 25] {
                let Some(master) = section.reference(master_field).and_then(|id| pkg.object(id)) else { continue };
                let storages = master.references(part_field);
                if !storages.iter().any(|s| self.has_content(*s)) {
                    continue;
                }
                let mut read = |i: usize| match storages.get(i) {
                    Some(&s) if self.has_content(s) => self.read_storage(s, ctx),
                    _ => Vec::new(),
                };
                let hf = HeaderFooter { left: read(0), center: read(1), right: read(2) };
                *slot = (!hf.is_empty()).then_some(hf);
                break;
            }
        }
        let [header, footer] = result;
        (header, footer)
    }

    fn has_content(&self, storage: u64) -> bool {
        self.pkg.object(storage).is_some_and(|st| {
            !st.strings(3).concat().trim().is_empty() || st.msg(9).is_some_and(|t| !t.msgs(1).is_empty())
        })
    }
}
