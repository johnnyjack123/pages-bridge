//! Writes the edits made to a converted `.docx` back into the Pages document it came from.
//!
//! The Pages file changes as little as possible: the body text storage is rebuilt from the
//! edited paragraphs, edited table cells get their new text, and the view state forgets the old
//! layout and selection. Everything else keeps its bytes. Only objects the document already
//! references are reused (paragraph, character and list styles, links), so no object, stylesheet
//! entry or metadata record has to be created; edits that would need one are reported instead.
//!
//! The written file is read back before it is accepted.

mod diff;
mod docx_in;
mod storage;
mod table;
mod view;
mod xml;

use std::collections::BTreeMap;
use std::path::Path;

use iwork::{Edits, Package};

use crate::Error;
use crate::model::Block;
use crate::reader::{self, kinds, styles::Styles};
use docx_in::Block as DocxBlock;

/// Why a document cannot be written back without losing something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// A page layout document without running text.
    LayoutDocument,
    /// The text carries content write-back would destroy (a user-facing description).
    Content(&'static str),
}

/// What write-back did, and what it could not carry over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub paragraphs_changed: usize,
    pub cells_changed: usize,
    /// Kind of edit (user-facing) → count.
    pub not_written: BTreeMap<String, usize>,
}

impl Report {
    pub(crate) fn skip(&mut self, what: &str) {
        *self.not_written.entry(what.to_owned()).or_default() += 1;
    }

    pub fn is_unchanged(&self) -> bool {
        self.paragraphs_changed == 0 && self.cells_changed == 0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Leave the cached layout and selection alone (for testing what Pages accepts).
    pub keep_view_state: bool,
    /// Re-encode the text storage even without edits (for testing the writer on its own).
    pub rewrite_unchanged: bool,
}

/// Writes `pages` with the edits of `docx` applied to `out`, which must differ from `pages`.
pub fn write_back(pages: &Path, docx: &Path, out: &Path, options: Options) -> Result<Report, Error> {
    let pkg = Package::open(pages)?;
    let title = pages.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (document, _) = reader::read(&pkg, &title)?;
    let mut edited = docx_in::read(docx).map_err(Error::Docx)?;

    let doc_id = *pkg.ids_of_kind(kinds::DOCUMENT).first().ok_or(Error::NoDocument)?;
    let body_id = pkg
        .object(doc_id)
        .and_then(|d| d.reference(4))
        .filter(|id| pkg.kind(*id) == Some(kinds::STORAGE))
        .ok_or(Error::Unsupported(Unsupported::LayoutDocument))?;
    let original = storage::Original::load(&pkg, body_id).map_err(Error::Unsupported)?;

    // The converter ends a body that ends with a table with an empty paragraph Word requires.
    if matches!(document.body.last(), Some(Block::Table(_)))
        && matches!(edited.last(), Some(DocxBlock::Para(p)) if p.chars.is_empty())
    {
        edited.pop();
    }
    let alignment = align_blocks(&document.body, &edited);

    let mut report = Report::default();
    let mut styles = Styles::new(&pkg);
    let paragraphs = storage::rebuild(&original, &document.body, &edited, &alignment, &mut styles, &mut report);
    let mut edits = Edits::new();
    let encoded = storage::encode(&original, &paragraphs);
    let text_changed = paragraphs != original.identity();
    if text_changed || options.rewrite_unchanged {
        edits.replace_object(body_id, encoded.payload.clone(), encoded.references.clone());
    }
    for (j, block) in edited.iter().enumerate() {
        if let (DocxBlock::Table(rows), Some(Block::Table(t0))) =
            (block, alignment[j].map(|i| &document.body[i]))
        {
            table::apply(&pkg, t0, rows, &mut edits, &mut report);
        }
    }
    if edits.is_empty() {
        return Ok(report);
    }
    if text_changed && !options.keep_view_state {
        view::reset(&pkg, &mut edits);
    }
    pkg.save_as(out, &edits)?;
    drop(pkg);
    if let Err(why) = verify(out, body_id, Some(encoded.text.as_str())) {
        let _ = std::fs::remove_file(out);
        return Err(Error::Verification(why));
    }
    Ok(report)
}

/// Text of a block for matching: paragraphs by their characters, tables all alike.
fn key_of_model(block: &Block) -> Option<String> {
    match block {
        Block::Paragraph(p) => Some(p.plain_text()),
        Block::Table(_) => None,
    }
}

fn key_of_docx(block: &DocxBlock) -> Option<String> {
    match block {
        DocxBlock::Para(p) => Some(p.text()),
        DocxBlock::Table(_) => None,
    }
}

/// For each edited block, the original block it corresponds to.
fn align_blocks(original: &[Block], edited: &[DocxBlock]) -> Vec<Option<usize>> {
    let a: Vec<Option<String>> = original.iter().map(key_of_model).collect();
    let b: Vec<Option<String>> = edited.iter().map(key_of_docx).collect();
    diff::align(&a, &b, |x, y| x == y, |x, y| x.is_some() == y.is_some())
}

/// Reads the written file back: it must open, convert, and hold exactly the intended text.
fn verify(path: &Path, body_id: u64, expected_text: Option<&str>) -> Result<(), String> {
    let pkg = Package::open(path).map_err(|e| e.to_string())?;
    reader::read(&pkg, "").map_err(|e| e.to_string())?;
    if let Some(expected) = expected_text {
        let actual = pkg.object(body_id).map(|s| s.strings(3).concat()).unwrap_or_default();
        if actual != expected {
            return Err("text differs after writing".to_owned());
        }
    }
    Ok(())
}
