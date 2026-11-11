//! Format-neutral document model between the Pages reader and the docx writer.
//!
//! Values keep Pages' meaning (points, Pages font names, relative line spacing); translating
//! them into another format's conventions is the writer's job.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Underline {
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strike {
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caps {
    None,
    All,
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Baseline,
    Super,
    Sub,
}

/// Character formatting; `None` means "not set here, inherit".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharProps {
    /// PostScript name as Pages stores it, e.g. `HelveticaNeue-Bold`.
    pub font: Option<String>,
    /// Points.
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<Underline>,
    pub strike: Option<Strike>,
    pub caps: Option<Caps>,
    pub vertical: Option<VerticalAlign>,
    pub color: Option<Rgb>,
    /// Points, positive raises the text.
    pub baseline_shift: Option<f64>,
}

impl CharProps {
    pub fn overlay(&mut self, top: &CharProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if top.$f.is_some() { self.$f = top.$f.clone(); } )* };
        }
        take!(font, size, bold, italic, underline, strike, caps, vertical, color, baseline_shift);
    }

    pub fn overlaid(&self, top: &CharProps) -> CharProps {
        let mut out = self.clone();
        out.overlay(top);
        out
    }

    /// Bold either by flag or by a bold face of the font family.
    pub fn is_bold(&self) -> bool {
        self.bold.unwrap_or(false)
            || self.font.as_deref().is_some_and(|f| f.split_once('-').is_some_and(|(_, face)| face.contains("Bold")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
    Justify,
    /// Follows the writing direction.
    Natural,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpacing {
    /// A multiple of the font's natural line height.
    Relative(f64),
    /// Points.
    AtLeast(f64),
    /// Points.
    Exactly(f64),
    /// Points added between lines.
    Between(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAlign {
    Left,
    Center,
    Right,
    Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabStop {
    /// Points from the left margin.
    pub position: f64,
    pub align: TabAlign,
}

/// A rule drawn along the paragraph (Pages "Borders & Rules").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParaBorder {
    pub top: bool,
    pub bottom: bool,
    /// Points.
    pub width: f64,
    pub color: Rgb,
}

/// Paragraph formatting; `None` means "not set here, inherit". Lengths are points.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParaProps {
    pub border: Option<ParaBorder>,
    pub align: Option<Align>,
    pub first_indent: Option<f64>,
    pub left_indent: Option<f64>,
    pub right_indent: Option<f64>,
    pub space_before: Option<f64>,
    pub space_after: Option<f64>,
    pub line_spacing: Option<LineSpacing>,
    pub keep_with_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub page_break_before: Option<bool>,
    pub widow_control: Option<bool>,
    pub tabs: Option<Vec<TabStop>>,
}

/// The built-in role of a paragraph style, used for outline levels and navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParaKind {
    #[default]
    Normal,
    Title,
    Subtitle,
    /// 1-based heading level.
    Heading(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListItem {
    /// Key into [`Document::lists`].
    pub style: u64,
    /// 0-based.
    pub level: u8,
    /// Numbering starts again here, at this value.
    pub restart: Option<u32>,
}

/// How text flows around a floating picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// Text on the sides where there is room.
    Around,
    /// Text only above and below.
    AboveAndBelow,
}

/// A picture placed apart from the text flow, anchored at its paragraph ("move with text").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Float {
    /// Points from the left edge of the text area.
    pub x: f64,
    /// Points from the top of the anchor paragraph.
    pub y: f64,
    pub wrap: Wrap,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InlineKind {
    Text(String),
    Tab,
    LineBreak,
    PageBreak,
    ColumnBreak,
    SoftHyphen,
    /// Index into [`Document::images`]; size in points; `float` set when it is not in the line.
    Image { index: usize, width: f64, height: f64, float: Option<Float> },
    PageNumber,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inline {
    pub kind: InlineKind,
    pub props: CharProps,
    pub link: Option<String>,
}

/// Where a paragraph came from in the source document. Writers ignore it; writing edits back
/// uses it to find the original text again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    /// Object id of the text storage.
    pub storage: u64,
    /// Paragraph index within the storage.
    pub paragraph: usize,
    /// Pages keeps a table inside a paragraph; around it the paragraph becomes several pieces.
    pub piece: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    pub kind: ParaKind,
    pub props: ParaProps,
    /// Formatting of the paragraph mark: what an empty paragraph is measured with.
    pub mark: CharProps,
    pub list: Option<ListItem>,
    pub inlines: Vec<Inline>,
    pub origin: Option<Origin>,
}

impl Paragraph {
    /// The paragraph's characters, with tabs, breaks and pictures as the control characters
    /// Pages stores for them; fields are left out.
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for inline in &self.inlines {
            match &inline.kind {
                InlineKind::Text(t) => s.push_str(t),
                InlineKind::Tab => s.push('\t'),
                InlineKind::LineBreak => s.push('\u{2028}'),
                InlineKind::PageBreak => s.push('\u{0C}'),
                InlineKind::ColumnBreak => s.push('\u{0E}'),
                InlineKind::SoftHyphen => s.push('\u{AD}'),
                InlineKind::Image { .. } => s.push('\u{FFFC}'),
                InlineKind::PageNumber => {}
            }
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    /// Points.
    pub width: f64,
    pub color: Rgb,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub fill: Option<Rgb>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// Minimum height in points.
    pub height: Option<f64>,
    pub header: bool,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    /// Column widths in points.
    pub columns: Vec<f64>,
    pub rows: Vec<Row>,
    pub border: Option<Border>,
    /// Points: top, right, bottom, left.
    pub cell_padding: [f64; 4],
    /// Object id of the source table (`TST.TableInfoArchive`), for writing edits back.
    pub source: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberFormat {
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperLetter,
    LowerLetter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListLabel {
    None,
    Bullet(String),
    Number { format: NumberFormat, prefix: &'static str, suffix: &'static str, tiered: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListLevel {
    pub label: ListLabel,
    /// Points from the paragraph's left edge to the label.
    pub label_indent: f64,
    /// Points from the label to the text.
    pub text_offset: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListStyle {
    pub levels: Vec<ListLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub data: Vec<u8>,
    /// File extension Word accepts: png, jpeg, gif, bmp or tiff.
    pub ext: &'static str,
    pub pixel_size: Option<(u32, u32)>,
}

/// Points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    pub width: f64,
    pub height: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub header_margin: f64,
    pub footer_margin: f64,
}

impl Default for PageSetup {
    fn default() -> Self {
        // A4 with 2 cm margins, Pages' default for German templates.
        Self {
            width: 595.28,
            height: 841.89,
            margin_left: 56.69,
            margin_right: 56.69,
            margin_top: 56.69,
            margin_bottom: 56.69,
            header_margin: 35.43,
            footer_margin: 42.52,
        }
    }
}

impl PageSetup {
    pub fn text_width(&self) -> f64 {
        (self.width - self.margin_left - self.margin_right).max(72.0)
    }
}

/// Header or footer content: Pages has a left, a centre and a right field.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeaderFooter {
    pub left: Vec<Block>,
    pub center: Vec<Block>,
    pub right: Vec<Block>,
}

impl HeaderFooter {
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.center.is_empty() && self.right.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub title: String,
    pub page: PageSetup,
    /// The template's body text formatting.
    pub defaults: CharProps,
    pub body: Vec<Block>,
    pub header: Option<HeaderFooter>,
    pub footer: Option<HeaderFooter>,
    pub lists: BTreeMap<u64, ListStyle>,
    pub images: Vec<Image>,
}

/// What a conversion could not carry over, for telling the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Kind of object (in the user's language) → count.
    pub skipped: BTreeMap<String, usize>,
    pub notes: Vec<String>,
}

impl Report {
    pub fn skip(&mut self, kind: impl Into<String>) {
        *self.skipped.entry(kind.into()).or_default() += 1;
    }
}
