//! Writes the [`model`](crate::model) as a Word document (Office Open XML, ECMA-376).
//!
//! Pages' paragraph styles become Word styles, and paragraphs and runs carry only what differs
//! from their style, so that choosing another style in the word processor changes the text.
//!
//! Pages documents often paginate with blank lines, so page breaks only survive if lines are
//! as tall as in Pages. Measured against Pages PDF exports, two differences need compensating:
//!
//! * Pages measures lines with its own font metrics, so line heights are written as absolute
//!   values from [`fonts::FontMapper::pages_line`] rather than as multiples of the substitute
//!   font's height.
//! * Pages puts the extra leading of 1.5 or double spacing below each line and lets the last
//!   line of a page reach into the bottom margin by its descent; Word puts the extra leading
//!   above the line and keeps the whole line inside the margins. Moving the top margin up by the
//!   extra leading and the bottom margin down by the descent of the document's most common body
//!   line gives the same lines per page and the same line positions.

mod fonts;
mod numbering;
mod xml;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use fonts::FontMapper;
use numbering::Numbering;
use xml::{attr, text, twips};

use crate::messages;
use crate::model::{
    Align, Block, Border, Caps, CharProps, Document, Float, HeaderFooter, Inline, InlineKind, LineSpacing, ParaBorder,
    ParaKind, ParaProps, ParaStyle, Paragraph, Rgb, Strike, TabAlign, TabStop, Table, Underline, VerticalAlign, Wrap,
};

pub(crate) const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
     xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
     xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
     xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
     xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const EMU_PER_PT: f64 = 12_700.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Body,
    Cell,
    /// Headers and footers have their own relationship parts; links and pictures are left out.
    Header,
}

struct Relationship {
    id: String,
    kind: &'static str,
    target: String,
    external: bool,
}

/// Word's built-in style for a Pages paragraph role: (style id, name).
fn builtin_style(kind: ParaKind) -> Option<(String, String)> {
    match kind {
        ParaKind::Normal => None,
        ParaKind::Title => Some(("Title".into(), "Title".into())),
        ParaKind::Subtitle => Some(("Subtitle".into(), "Subtitle".into())),
        ParaKind::Heading(n) => Some((format!("Heading{n}"), format!("heading {n}"))),
    }
}

/// The style id each paragraph style of `doc` is written with. The template's body style
/// becomes `Normal` and the first style of each heading role Word's built-in style, so that
/// a word processor's "Heading 1" applies Pages' heading. Ids depend only on the styles, not on
/// the text, so a document read again after write-back gets the same ones.
pub(crate) fn style_ids(doc: &Document) -> BTreeMap<u64, String> {
    let body = doc.para_styles.iter().find(|(_, s)| s.is_body && s.parent.is_none()).map(|(id, _)| *id);
    let mut taken = BTreeSet::new();
    let mut ids = BTreeMap::new();
    for (&id, style) in &doc.para_styles {
        let builtin = if Some(id) == body {
            Some("Normal".to_owned())
        } else if style.parent.is_none() {
            builtin_style(style.kind).map(|(id, _)| id)
        } else {
            None
        };
        let style_id = builtin.filter(|b| taken.insert(b.clone())).unwrap_or_else(|| format!("Pages{id}"));
        ids.insert(id, style_id);
    }
    ids
}

fn hex(c: Rgb) -> String {
    format!("{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

/// A paragraph's or paragraph style's formatting as Word sees it, with every property decided,
/// so that a paragraph can be written as its style plus what differs.
#[derive(Debug, Clone, PartialEq)]
struct ParaFormat {
    keep_next: bool,
    keep_lines: bool,
    page_break_before: bool,
    widow_control: bool,
    border: Option<ParaBorder>,
    tabs: Vec<TabStop>,
    before: i64,
    after: i64,
    /// `w:line` and `w:lineRule`.
    line: (i64, &'static str),
    left: i64,
    right: i64,
    /// Positive for a first-line indent, negative for a hanging one.
    first: i64,
    jc: &'static str,
}

impl Default for ParaFormat {
    /// What the document defaults in styles.xml give.
    fn default() -> Self {
        Self {
            keep_next: false,
            keep_lines: false,
            page_break_before: false,
            widow_control: true,
            border: None,
            tabs: Vec::new(),
            before: 0,
            after: 0,
            line: (240, "auto"),
            left: 0,
            right: 0,
            first: 0,
            jc: "left",
        }
    }
}

/// Character formatting as Word sees it; see [`ParaFormat`].
#[derive(Debug, Clone, PartialEq)]
struct RunFormat {
    family: String,
    bold: bool,
    italic: bool,
    caps: Caps,
    strike: Strike,
    color: Rgb,
    /// Half points, positive raises the text.
    position: i64,
    /// Half points.
    size: i64,
    underline: Underline,
    vertical: VerticalAlign,
}

/// A style as written: its id and the formatting a paragraph using it gets.
#[derive(Debug, Clone)]
struct StyleFormat {
    id: Option<String>,
    para: ParaFormat,
    run: RunFormat,
}

struct Writer<'d> {
    doc: &'d Document,
    fonts: FontMapper,
    numbering: Numbering,
    rels: Vec<Relationship>,
    links: HashMap<String, String>,
    images: HashMap<usize, String>,
    media: Vec<(String, usize)>,
    drawing_id: u32,
    /// What the document defaults give, the base of `Normal`.
    root: StyleFormat,
    /// Paragraphs without a style of their own are `Normal`.
    normal: StyleFormat,
    styles: HashMap<u64, StyleFormat>,
    styles_xml: String,
    /// (extra leading, descent) of body lines in 1/100 pt → number of paragraphs.
    line_stats: HashMap<(i64, i64), usize>,
}

/// Writes `doc` to `path`, replacing an existing file only once the new one is complete.
pub(crate) fn write(doc: &Document, path: &Path) -> std::io::Result<()> {
    let mut fonts = FontMapper::new();
    let defaults = &doc.defaults;
    let family = defaults.font.as_deref().map_or_else(|| "Arial".to_owned(), |f| fonts.resolve(f).family);
    #[allow(clippy::cast_possible_truncation)]
    let size = (defaults.size.unwrap_or(11.0) * 2.0).round() as i64;
    let root_run = RunFormat {
        family,
        bold: false,
        italic: false,
        caps: Caps::None,
        strike: Strike::None,
        color: Rgb(0, 0, 0),
        position: 0,
        size,
        underline: Underline::None,
        vertical: VerticalAlign::Baseline,
    };
    let root = StyleFormat { id: None, para: ParaFormat::default(), run: root_run };
    let mut writer = Writer {
        doc,
        fonts,
        numbering: Numbering::new(),
        rels: Vec::new(),
        links: HashMap::new(),
        images: HashMap::new(),
        media: Vec::new(),
        drawing_id: 0,
        normal: root.clone(),
        root,
        styles: HashMap::new(),
        styles_xml: String::new(),
        line_stats: HashMap::new(),
    };
    writer.prepare_styles();
    let parts = writer.parts();
    let tmp = path.with_extension("docx.tmp");
    write_zip(&tmp, &parts)?;
    std::fs::rename(&tmp, path)
}

fn write_zip(path: &Path, parts: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in parts {
        zip.start_file(name.as_str(), options).map_err(std::io::Error::other)?;
        zip.write_all(data)?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
}

impl Writer<'_> {
    fn parts(&mut self) -> Vec<(String, Vec<u8>)> {
        let doc = self.doc;
        let mut body = self.blocks(&doc.body, Part::Body, None);
        if !matches!(doc.body.last(), Some(Block::Paragraph(_))) {
            body.push_str("<w:p/>");
        }
        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        let mut sect = String::new();
        for (hf, root, file, kind) in [(&doc.header, "hdr", "header1.xml", "header"), (&doc.footer, "ftr", "footer1.xml", "footer")] {
            let Some(hf) = hf.as_ref().filter(|h| !h.is_empty()) else { continue };
            let xml = self.header_footer(hf, root);
            parts.push((format!("word/{file}"), xml.into_bytes()));
            let id = self.relationship(kind, file.to_owned(), false);
            let _ = write!(sect, "<w:{kind}Reference w:type=\"default\" r:id=\"{id}\"/>");
        }

        let page = doc.page;
        let (mut top, mut bottom) = (page.margin_top, page.margin_bottom);
        if let Some((&(extra, descent), _)) = self.line_stats.iter().max_by_key(|(k, n)| (**n, **k)) {
            #[allow(clippy::cast_precision_loss)]
            {
                top = (top - extra as f64 / 100.0).max(0.0);
                bottom = (bottom - descent as f64 / 100.0).max(0.0);
            }
        }
        let orient = if page.width > page.height { " w:orient=\"landscape\"" } else { "" };
        let _ = write!(
            sect,
            "<w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" \
             w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/>",
            twips(page.width),
            twips(page.height),
            twips(top),
            twips(page.margin_right),
            twips(bottom),
            twips(page.margin_left),
            twips(page.header_margin),
            twips(page.footer_margin)
        );
        let document = format!("{XML_DECL}<w:document {NS}><w:body>{body}<w:sectPr>{sect}</w:sectPr></w:body></w:document>");
        parts.insert(0, ("word/document.xml".into(), document.into_bytes()));
        parts.push(("word/styles.xml".into(), self.styles_xml().into_bytes()));
        parts.push(("word/settings.xml".into(), SETTINGS_XML.as_bytes().to_vec()));

        let mut rels = vec![
            Relationship { id: "rIdStyles".into(), kind: "styles", target: "styles.xml".into(), external: false },
            Relationship { id: "rIdSettings".into(), kind: "settings", target: "settings.xml".into(), external: false },
        ];
        if !self.numbering.is_empty() {
            parts.push(("word/numbering.xml".into(), self.numbering.xml(&doc.lists).into_bytes()));
            rels.push(Relationship { id: "rIdNumbering".into(), kind: "numbering", target: "numbering.xml".into(), external: false });
        }
        for (target, index) in &self.media {
            parts.push((format!("word/{target}"), doc.images[*index].data.clone()));
        }
        rels.append(&mut self.rels);
        let mut rels_xml = format!("{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
        for r in &rels {
            let mode = if r.external { " TargetMode=\"External\"" } else { "" };
            let _ = write!(rels_xml, "<Relationship Id=\"{}\" Type=\"{REL}/{}\" Target=\"{}\"{mode}/>", r.id, r.kind, attr(&r.target));
        }
        rels_xml.push_str("</Relationships>");
        parts.push(("word/_rels/document.xml.rels".into(), rels_xml.into_bytes()));
        parts.push(("_rels/.rels".into(), ROOT_RELS.as_bytes().to_vec()));
        parts.push(("docProps/core.xml".into(), core_xml(&doc.title).into_bytes()));
        // Some readers sniff a package by its first entry, so the content types go first.
        let content_types = content_types(&parts);
        parts.insert(0, ("[Content_Types].xml".into(), content_types.into_bytes()));
        parts
    }

    fn relationship(&mut self, kind: &'static str, target: String, external: bool) -> String {
        let id = format!("rId{}", self.rels.len() + 10);
        self.rels.push(Relationship { id: id.clone(), kind, target, external });
        id
    }

    fn blocks(&mut self, blocks: &[Block], part: Part, align: Option<&'static str>) -> String {
        let mut out = String::new();
        for block in blocks {
            match block {
                Block::Paragraph(p) => self.paragraph(p, part, align, &mut out),
                Block::Table(t) => self.table(t, &mut out),
            }
        }
        out
    }

    fn paragraph(&mut self, p: &Paragraph, part: Part, align: Option<&'static str>, out: &mut String) {
        let style = p.style.and_then(|id| self.styles.get(&id)).unwrap_or(&self.normal).clone();
        let (mut format, line_key) = self.para_format(&p.props, &p.mark);
        if let (Part::Body, Some(key)) = (part, line_key) {
            *self.line_stats.entry(key).or_default() += 1;
        }
        if let Some(jc) = align {
            format.jc = jc;
        }
        let mut numbering = String::new();
        if let Some(item) = p.list.filter(|_| part != Part::Header) {
            // The list level supplies the indentation; any written here would override it.
            (format.left, format.right, format.first) = (style.para.left, style.para.right, style.para.first);
            let id = self.numbering.num_id(item.style, item.restart);
            numbering = format!("<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{id}\"/></w:numPr>", item.level);
        }
        let mark = self.run_format(&p.mark);
        let mut ppr = String::new();
        if let Some(id) = &style.id {
            let _ = write!(ppr, "<w:pStyle w:val=\"{id}\"/>");
        }
        ppr.push_str(&ppr_diff(&format, &style.para, &numbering, None));
        ppr.push_str(&wrapped("rPr", &rpr_diff(&mark, &style.run)));
        out.push_str("<w:p>");
        out.push_str(&wrapped("pPr", &ppr));
        let mut i = 0;
        while i < p.inlines.len() {
            let link = p.inlines[i].link.as_deref();
            let end = i + p.inlines[i..].iter().take_while(|x| x.link.as_deref() == link).count();
            match link.filter(|_| part != Part::Header) {
                Some(url) => {
                    let id = self.hyperlink(url);
                    let _ = write!(out, "<w:hyperlink r:id=\"{id}\" w:history=\"1\">");
                    for inline in &p.inlines[i..end] {
                        self.run(inline, part, &style.run, out);
                    }
                    out.push_str("</w:hyperlink>");
                }
                None => {
                    for inline in &p.inlines[i..end] {
                        self.run(inline, part, &style.run, out);
                    }
                }
            }
            i = end;
        }
        out.push_str("</w:p>");
    }

    fn hyperlink(&mut self, url: &str) -> String {
        if let Some(id) = self.links.get(url) {
            return id.clone();
        }
        let id = self.relationship("hyperlink", url.to_owned(), true);
        self.links.insert(url.to_owned(), id.clone());
        id
    }

    fn run(&mut self, inline: &Inline, part: Part, base: &RunFormat, out: &mut String) {
        let format = self.run_format(&inline.props);
        let rpr = wrapped("rPr", &rpr_diff(&format, base));
        let content = match &inline.kind {
            InlineKind::Text(t) => format!("<w:t xml:space=\"preserve\">{}</w:t>", text(t)),
            InlineKind::Tab => "<w:tab/>".into(),
            InlineKind::LineBreak => "<w:br/>".into(),
            InlineKind::PageBreak => "<w:br w:type=\"page\"/>".into(),
            InlineKind::ColumnBreak => "<w:br w:type=\"column\"/>".into(),
            InlineKind::SoftHyphen => "<w:softHyphen/>".into(),
            InlineKind::PageNumber => {
                let _ = write!(out, "<w:fldSimple w:instr=\" PAGE \"><w:r>{rpr}<w:t>1</w:t></w:r></w:fldSimple>");
                return;
            }
            InlineKind::Image { index, width, height, float } => {
                if part != Part::Header {
                    self.image(*index, *width, *height, *float, out);
                }
                return;
            }
        };
        let _ = write!(out, "<w:r>{rpr}{content}</w:r>");
    }

    fn run_format(&mut self, c: &CharProps) -> RunFormat {
        let (mut family, mut bold, mut italic) = (self.root.run.family.clone(), c.bold.unwrap_or(false), c.italic.unwrap_or(false));
        if let Some(font) = &c.font {
            let info = self.fonts.resolve(font);
            bold |= info.implies_bold;
            italic |= info.implies_italic;
            family = info.family;
        }
        #[allow(clippy::cast_possible_truncation)]
        let half_points = |pt: f64| (pt * 2.0).round() as i64;
        RunFormat {
            family,
            bold,
            italic,
            caps: c.caps.unwrap_or(Caps::None),
            strike: c.strike.unwrap_or(Strike::None),
            color: c.color.unwrap_or(Rgb(0, 0, 0)),
            position: c.baseline_shift.filter(|v| v.abs() >= 0.25).map_or(0, half_points),
            size: c.size.map_or(self.root.run.size, half_points),
            underline: c.underline.unwrap_or(Underline::None),
            vertical: c.vertical.unwrap_or(VerticalAlign::Baseline),
        }
    }

    /// The paragraph formatting, and for lines measured like Pages the (extra leading, descent)
    /// key of the margin compensation.
    fn para_format(&mut self, props: &ParaProps, mark: &CharProps) -> (ParaFormat, Option<(i64, i64)>) {
        let (line, line_key) = self.line(props, mark);
        let positive = |v: Option<f64>| v.filter(|v| *v > 0.0).map_or(0, twips);
        let format = ParaFormat {
            keep_next: props.keep_with_next == Some(true),
            keep_lines: props.keep_lines == Some(true),
            page_break_before: props.page_break_before == Some(true),
            widow_control: props.widow_control != Some(false),
            border: props.border,
            tabs: props.tabs.clone().unwrap_or_default(),
            before: positive(props.space_before),
            after: positive(props.space_after),
            line,
            left: props.left_indent.map_or(0, twips),
            right: props.right_indent.map_or(0, twips),
            first: props.first_indent.map_or(0, twips),
            jc: match props.align {
                Some(Align::Right) => "right",
                Some(Align::Center) => "center",
                Some(Align::Justify) => "both",
                _ => "left",
            },
        };
        (format, line_key)
    }

    fn line(&mut self, props: &ParaProps, mark: &CharProps) -> ((i64, &'static str), Option<(i64, i64)>) {
        let size = mark.size.unwrap_or(11.0);
        match props.line_spacing.unwrap_or(LineSpacing::Relative(1.0)) {
            LineSpacing::Relative(amount) => match self.fonts.pages_line(mark.font.as_deref(), size, mark.is_bold()) {
                Some(line) => {
                    // "At least" still lets a large picture or font in the line grow it.
                    let rule = if amount >= 1.0 { "atLeast" } else { "exact" };
                    #[allow(clippy::cast_possible_truncation)]
                    let key = (((amount - 1.0).max(0.0) * line.height * 100.0).round() as i64, (line.descent * 100.0).round() as i64);
                    ((twips(amount * line.height), rule), Some(key))
                }
                #[allow(clippy::cast_possible_truncation)]
                None if props.line_spacing.is_some() => (((240.0 * amount).round() as i64, "auto"), None),
                None => ((240, "auto"), None),
            },
            LineSpacing::AtLeast(v) => ((twips(v), "atLeast"), None),
            LineSpacing::Exactly(v) => ((twips(v), "exact"), None),
            LineSpacing::Between(v) => ((twips(size * 1.2 + v), "atLeast"), None),
        }
    }

    fn table(&mut self, t: &Table, out: &mut String) {
        let widths: Vec<i64> = t.columns.iter().map(|w| twips(*w).max(200)).collect();
        let total: i64 = widths.iter().sum();
        let border = t.border.unwrap_or(Border { width: 0.5, color: Rgb(0, 0, 0) });
        #[allow(clippy::cast_possible_truncation)]
        let size = ((border.width * 8.0).round() as i64).clamp(2, 96);
        let color = hex(border.color);
        let _ = write!(out, "<w:tbl><w:tblPr><w:tblW w:w=\"{total}\" w:type=\"dxa\"/><w:tblLayout w:type=\"fixed\"/><w:tblBorders>");
        for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
            let _ = write!(out, "<w:{side} w:val=\"single\" w:sz=\"{size}\" w:space=\"0\" w:color=\"{color}\"/>");
        }
        let [pt, pr, pb, pl] = t.cell_padding;
        let _ = write!(
            out,
            "</w:tblBorders><w:tblCellMar><w:top w:w=\"{}\" w:type=\"dxa\"/><w:left w:w=\"{}\" w:type=\"dxa\"/>\
             <w:bottom w:w=\"{}\" w:type=\"dxa\"/><w:right w:w=\"{}\" w:type=\"dxa\"/></w:tblCellMar>\
             <w:tblLook w:val=\"0000\"/></w:tblPr><w:tblGrid>",
            twips(pt),
            twips(pl),
            twips(pb),
            twips(pr)
        );
        for w in &widths {
            let _ = write!(out, "<w:gridCol w:w=\"{w}\"/>");
        }
        out.push_str("</w:tblGrid>");
        for row in &t.rows {
            out.push_str("<w:tr>");
            let mut trpr = String::new();
            // Pages' row height includes the cell padding; Word and WordCraft both add the
            // padding to the height written here (compared against a Pages PDF), so leave it out.
            if let Some(h) = row.height.map(|h| h - pt - pb).filter(|h| *h > 0.0) {
                let _ = write!(trpr, "<w:trHeight w:val=\"{}\"/>", twips(h));
            }
            if row.header {
                trpr.push_str("<w:tblHeader/>");
            }
            if !trpr.is_empty() {
                let _ = write!(out, "<w:trPr>{trpr}</w:trPr>");
            }
            for (i, width) in widths.iter().enumerate() {
                let cell = row.cells.get(i);
                let shading = cell
                    .and_then(|c| c.fill)
                    .map(|f| format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", hex(f)))
                    .unwrap_or_default();
                let _ = write!(out, "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{shading}</w:tcPr>");
                let blocks = cell.map(|c| c.blocks.as_slice()).unwrap_or_default();
                out.push_str(&self.blocks(blocks, Part::Cell, None));
                // A cell must end with a paragraph.
                if !matches!(blocks.last(), Some(Block::Paragraph(_))) {
                    out.push_str("<w:p/>");
                }
                out.push_str("</w:tc>");
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl>");
    }

    fn image(&mut self, index: usize, width: f64, height: f64, float: Option<Float>, out: &mut String) {
        let Some(image) = self.doc.images.get(index) else { return };
        let id = match self.images.get(&index) {
            Some(id) => id.clone(),
            None => {
                let target = format!("media/image{}.{}", index + 1, image.ext);
                self.media.push((target.clone(), index));
                let id = self.relationship("image", target, false);
                self.images.insert(index, id.clone());
                id
            }
        };
        self.drawing_id += 1;
        let n = self.drawing_id;
        #[allow(clippy::cast_possible_truncation)]
        let emu = |pt: f64| (pt * EMU_PER_PT).round() as i64;
        let (cx, cy) = (emu(width), emu(height));
        let graphic = format!(
            "<wp:docPr id=\"{n}\" name=\"Image {n}\"/>\
             <wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>\
             <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"image{n}.{ext}\"/><pic:cNvPicPr/></pic:nvPicPr>\
             <pic:blipFill><a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
             </a:graphicData></a:graphic>",
            ext = image.ext
        );
        match float {
            None => {
                let _ = write!(
                    out,
                    "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
                     <wp:extent cx=\"{cx}\" cy=\"{cy}\"/>{graphic}</wp:inline></w:drawing></w:r>"
                );
            }
            Some(float) => {
                let wrap = match float.wrap {
                    Wrap::Around => "<wp:wrapSquare wrapText=\"bothSides\"/>",
                    Wrap::AboveAndBelow => "<wp:wrapTopAndBottom/>",
                };
                // Pages' default wrap margin of 12 pt, but not above: Pages places the picture
                // by its offset alone, and a margin above makes Word push a table that ends just
                // above the picture down below it.
                let gap = emu(12.0);
                let _ = write!(
                    out,
                    "<w:r><w:drawing><wp:anchor distT=\"0\" distB=\"{gap}\" distL=\"{gap}\" distR=\"{gap}\" \
                     simplePos=\"0\" relativeHeight=\"{n}\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"0\">\
                     <wp:simplePos x=\"0\" y=\"0\"/>\
                     <wp:positionH relativeFrom=\"column\"><wp:posOffset>{}</wp:posOffset></wp:positionH>\
                     <wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>{}</wp:posOffset></wp:positionV>\
                     <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>{wrap}{graphic}\
                     </wp:anchor></w:drawing></w:r>",
                    emu(float.x),
                    emu(float.y)
                );
            }
        }
    }

    fn header_footer(&mut self, hf: &HeaderFooter, root: &str) -> String {
        let fields = [&hf.left, &hf.center, &hf.right];
        let filled: Vec<usize> = (0..3).filter(|i| !fields[*i].is_empty()).collect();
        let body = if let [only] = filled[..] {
            self.blocks(fields[only], Part::Header, Some(["left", "center", "right"][only]))
        } else {
            // Several fields: one line with a centre and a right tab stop, as Pages lays them out.
            let width = self.doc.page.text_width();
            let base = self.normal.run.clone();
            let mut runs = String::new();
            for (i, blocks) in fields.iter().enumerate() {
                if i > 0 {
                    runs.push_str("<w:r><w:tab/></w:r>");
                }
                if let Some(Block::Paragraph(p)) = blocks.first() {
                    for inline in &p.inlines {
                        self.run(inline, Part::Header, &base, &mut runs);
                    }
                }
            }
            format!(
                "<w:p><w:pPr><w:tabs><w:tab w:val=\"center\" w:pos=\"{}\"/><w:tab w:val=\"right\" w:pos=\"{}\"/>\
                 </w:tabs></w:pPr>{runs}</w:p>",
                twips(width / 2.0),
                twips(width)
            )
        };
        format!("{XML_DECL}<w:{root} {NS}>{body}</w:{root}>")
    }

    /// Works out every paragraph style's formatting and writes the style definitions.
    ///
    /// Paragraphs then name their style and carry only what differs from it, so that applying
    /// another style in a word processor changes how they look.
    fn prepare_styles(&mut self) {
        let doc = self.doc;
        let ids = style_ids(doc);
        let used = used_styles(doc);
        let normal_id = ids.iter().find(|(_, s)| s.as_str() == "Normal").map(|(id, _)| *id);
        if let Some(id) = normal_id {
            self.normal = self.style_format("Normal", &doc.para_styles[&id]);
            self.styles.insert(id, self.normal.clone());
        }
        let root = self.root.clone();
        let mut xml = style_xml("Normal", "Normal", None, true, None, &self.normal, &root);
        xml.push_str(
            "<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\"><w:name w:val=\"Default Paragraph Font\"/>\
             <w:uiPriority w:val=\"1\"/><w:semiHidden/></w:style>\
             <w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:tblPr>\
             <w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/>\
             <w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>",
        );
        let mut names = reserved_names();
        // Word also knows its built-in styles by their names in the user's language, and a
        // second style called like that collides with them: in German Word, Pages' caption
        // style "Untertitel" became the subtitle. Pages names its styles in the same language.
        for (id, style) in &doc.para_styles {
            if ids[id] == "Normal" || builtin_style(style.kind).is_some_and(|(b, _)| b == ids[id]) {
                names.extend(style.name.as_ref().map(|n| n.to_lowercase()));
            }
        }
        let mut variations: HashMap<u64, usize> = HashMap::new();
        // Parents before their variations, which are written as changes of them.
        let mut order: Vec<u64> = ids.keys().copied().filter(|id| Some(*id) != normal_id).collect();
        order.sort_by_key(|id| (ancestors(doc, *id).count(), *id));
        for id in order {
            let style = &doc.para_styles[&id];
            let style_id = ids[&id].clone();
            let parent = style.parent.and_then(|p| self.styles.get(&p)).cloned();
            let base = parent.clone().unwrap_or_else(|| self.normal.clone());
            let based_on = parent.and_then(|p| p.id).unwrap_or_else(|| "Normal".to_owned());
            let builtin = builtin_style(style.kind).filter(|(builtin_id, _)| *builtin_id == style_id).map(|(_, name)| name);
            let wanted = match (&builtin, &style.name, style.parent) {
                (Some(name), ..) | (None, Some(name), _) => name.clone(),
                (None, None, Some(parent)) => {
                    let n = variations.entry(parent).or_default();
                    *n += 1;
                    messages::style_variation(&display_name(doc, &ids, parent), *n)
                }
                (None, None, None) => style_id.clone(),
            };
            // Built-in styles keep their exact names, which are reserved for them.
            let name = if builtin.is_some() { wanted } else { unique_name(&mut names, wanted) };
            let format = self.style_format(&style_id, style);
            let outline = match (style.kind, style.parent) {
                (ParaKind::Heading(n), None) => Some(n - 1),
                _ => None,
            };
            let quick = builtin.is_some() || (style.name.is_some() && used.contains(&id));
            xml.push_str(&style_xml(&style_id, &name, Some(&based_on), quick, outline, &format, &base));
            self.styles.insert(id, format);
        }
        self.styles_xml = xml;
    }

    fn style_format(&mut self, id: &str, style: &ParaStyle) -> StyleFormat {
        StyleFormat {
            id: Some(id.to_owned()),
            para: self.para_format(&style.props, &style.chars).0,
            run: self.run_format(&style.chars),
        }
    }

    fn styles_xml(&self) -> String {
        let root = &self.root.run;
        let family = attr(&root.family);
        let size = root.size;
        format!(
            "{XML_DECL}<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" \
             w:cs=\"{family}\" w:eastAsia=\"{family}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:rPrDefault>\
             <w:pPrDefault><w:pPr><w:widowControl/><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
             </w:pPrDefault></w:docDefaults>{}</w:styles>",
            self.styles_xml
        )
    }
}

/// The paragraph styles some paragraph uses.
fn used_styles(doc: &Document) -> HashSet<u64> {
    fn walk(blocks: &[Block], out: &mut HashSet<u64>) {
        for block in blocks {
            match block {
                Block::Paragraph(p) => out.extend(p.style),
                Block::Table(t) => t.rows.iter().flat_map(|r| &r.cells).for_each(|c| walk(&c.blocks, out)),
            }
        }
    }
    let mut out = HashSet::new();
    walk(&doc.body, &mut out);
    for hf in doc.header.iter().chain(&doc.footer) {
        for field in [&hf.left, &hf.center, &hf.right] {
            walk(field, &mut out);
        }
    }
    out
}

/// The styles a style varies, nearest first.
fn ancestors(doc: &Document, id: u64) -> impl Iterator<Item = u64> + '_ {
    std::iter::successors(doc.para_styles.get(&id).and_then(|s| s.parent), |p| doc.para_styles.get(p).and_then(|s| s.parent))
        .take(32)
}

/// The name a variation of `id` is called after: the Pages name of the nearest named style.
fn display_name(doc: &Document, ids: &BTreeMap<u64, String>, id: u64) -> String {
    std::iter::once(id)
        .chain(ancestors(doc, id))
        .find_map(|a| doc.para_styles.get(&a).and_then(|s| s.name.clone()))
        .unwrap_or_else(|| ids.get(&id).cloned().unwrap_or_default())
}

/// Names of the styles this writer defines as Word's own, in English and German, lowercase.
fn reserved_names() -> HashSet<String> {
    let mut names: HashSet<String> =
        ["normal", "default paragraph font", "normal table", "title", "subtitle", "standard", "titel", "untertitel"]
            .map(str::to_owned)
            .into();
    for n in 1..=9 {
        names.extend([format!("heading {n}"), format!("überschrift {n}")]);
    }
    names
}

/// Word needs style names to be unique, ignoring case.
fn unique_name(taken: &mut HashSet<String>, wanted: String) -> String {
    let mut name = wanted.clone();
    let mut n = 2;
    while !taken.insert(name.to_lowercase()) {
        name = format!("{wanted} {n}");
        n += 1;
    }
    name
}

fn style_xml(
    id: &str,
    name: &str,
    based_on: Option<&str>,
    quick: bool,
    outline: Option<u8>,
    format: &StyleFormat,
    base: &StyleFormat,
) -> String {
    let default = if id == "Normal" { " w:default=\"1\"" } else { "" };
    let mut s = format!("<w:style w:type=\"paragraph\"{default} w:styleId=\"{}\"><w:name w:val=\"{}\"/>", attr(id), attr(name));
    if let Some(based_on) = based_on {
        let _ = write!(s, "<w:basedOn w:val=\"{}\"/>", attr(based_on));
    }
    if outline.is_some() || matches!(id, "Title" | "Subtitle") {
        s.push_str("<w:next w:val=\"Normal\"/>");
    }
    if quick {
        s.push_str("<w:qFormat/>");
    }
    s.push_str(&wrapped("pPr", &ppr_diff(&format.para, &base.para, "", outline)));
    s.push_str(&wrapped("rPr", &rpr_diff(&format.run, &base.run)));
    s.push_str("</w:style>");
    s
}

fn wrapped(tag: &str, inner: &str) -> String {
    if inner.is_empty() { String::new() } else { format!("<w:{tag}>{inner}</w:{tag}>") }
}

/// The `w:pPr` content that turns `base` into `p`, in the schema's element order.
fn ppr_diff(p: &ParaFormat, base: &ParaFormat, numbering: &str, outline: Option<u8>) -> String {
    let mut s = String::new();
    let mut flag = |name: &str, on: bool, was: bool| {
        if on != was {
            let _ = if on { write!(s, "<w:{name}/>") } else { write!(s, "<w:{name} w:val=\"0\"/>") };
        }
    };
    flag("keepNext", p.keep_next, base.keep_next);
    flag("keepLines", p.keep_lines, base.keep_lines);
    flag("pageBreakBefore", p.page_break_before, base.page_break_before);
    flag("widowControl", p.widow_control, base.widow_control);
    s.push_str(numbering);
    if p.border != base.border {
        let sides = |b: Option<ParaBorder>| [("top", b.is_some_and(|b| b.top)), ("bottom", b.is_some_and(|b| b.bottom))];
        let mut border = String::new();
        for ((side, drawn), (_, was_drawn)) in sides(p.border).into_iter().zip(sides(base.border)) {
            match p.border.filter(|_| drawn) {
                Some(b) => {
                    #[allow(clippy::cast_possible_truncation)]
                    let size = ((b.width * 8.0).round() as i64).clamp(2, 96);
                    let _ = write!(border, "<w:{side} w:val=\"single\" w:sz=\"{size}\" w:space=\"4\" w:color=\"{}\"/>", hex(b.color));
                }
                None if was_drawn => {
                    let _ = write!(border, "<w:{side} w:val=\"nil\"/>");
                }
                None => {}
            }
        }
        s.push_str(&wrapped("pBdr", &border));
    }
    if p.tabs != base.tabs {
        // Word adds a paragraph's tab stops to its style's; Pages replaces them.
        let mut tabs = String::new();
        for tab in &base.tabs {
            if !p.tabs.iter().any(|t| twips(t.position) == twips(tab.position)) {
                let _ = write!(tabs, "<w:tab w:val=\"clear\" w:pos=\"{}\"/>", twips(tab.position));
            }
        }
        for tab in p.tabs.iter().filter(|t| !base.tabs.contains(t)) {
            let val = match tab.align {
                TabAlign::Left => "left",
                TabAlign::Center => "center",
                TabAlign::Right => "right",
                TabAlign::Decimal => "decimal",
            };
            let _ = write!(tabs, "<w:tab w:val=\"{val}\" w:pos=\"{}\"/>", twips(tab.position));
        }
        s.push_str(&wrapped("tabs", &tabs));
    }
    let mut spacing = Vec::new();
    if p.before != base.before {
        spacing.push(format!("w:before=\"{}\"", p.before));
    }
    if p.after != base.after {
        spacing.push(format!("w:after=\"{}\"", p.after));
    }
    if p.line != base.line {
        spacing.push(format!("w:line=\"{}\" w:lineRule=\"{}\"", p.line.0, p.line.1));
    }
    if !spacing.is_empty() {
        let _ = write!(s, "<w:spacing {}/>", spacing.join(" "));
    }
    let mut ind = Vec::new();
    if p.left != base.left {
        ind.push(format!("w:left=\"{}\"", p.left));
    }
    if p.right != base.right {
        ind.push(format!("w:right=\"{}\"", p.right));
    }
    if p.first != base.first {
        ind.push(if p.first < 0 { format!("w:hanging=\"{}\"", -p.first) } else { format!("w:firstLine=\"{}\"", p.first) });
    }
    if !ind.is_empty() {
        let _ = write!(s, "<w:ind {}/>", ind.join(" "));
    }
    if p.jc != base.jc {
        let _ = write!(s, "<w:jc w:val=\"{}\"/>", p.jc);
    }
    if let Some(level) = outline {
        let _ = write!(s, "<w:outlineLvl w:val=\"{level}\"/>");
    }
    s
}

/// The `w:rPr` content that turns `base` into `r`, in the schema's element order.
fn rpr_diff(r: &RunFormat, base: &RunFormat) -> String {
    let mut s = String::new();
    if r.family != base.family {
        let f = attr(&r.family);
        let _ = write!(s, "<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\" w:eastAsia=\"{f}\"/>");
    }
    let mut toggle = |names: &[&str], on: bool, was: bool| {
        if on != was {
            for name in names {
                let _ = if on { write!(s, "<w:{name}/>") } else { write!(s, "<w:{name} w:val=\"0\"/>") };
            }
        }
    };
    toggle(&["b", "bCs"], r.bold, base.bold);
    toggle(&["i", "iCs"], r.italic, base.italic);
    toggle(&["caps"], r.caps == Caps::All, base.caps == Caps::All);
    toggle(&["smallCaps"], r.caps == Caps::Small, base.caps == Caps::Small);
    toggle(&["strike"], r.strike == Strike::Single, base.strike == Strike::Single);
    toggle(&["dstrike"], r.strike == Strike::Double, base.strike == Strike::Double);
    if r.color != base.color {
        let _ = write!(s, "<w:color w:val=\"{}\"/>", hex(r.color));
    }
    if r.position != base.position {
        let _ = write!(s, "<w:position w:val=\"{}\"/>", r.position);
    }
    if r.size != base.size {
        let _ = write!(s, "<w:sz w:val=\"{0}\"/><w:szCs w:val=\"{0}\"/>", r.size);
    }
    if r.underline != base.underline {
        let val = match r.underline {
            Underline::Single => "single",
            Underline::Double => "double",
            Underline::None => "none",
        };
        let _ = write!(s, "<w:u w:val=\"{val}\"/>");
    }
    if r.vertical != base.vertical {
        let val = match r.vertical {
            VerticalAlign::Super => "superscript",
            VerticalAlign::Sub => "subscript",
            VerticalAlign::Baseline => "baseline",
        };
        let _ = write!(s, "<w:vertAlign w:val=\"{val}\"/>");
    }
    s
}

fn core_xml(title: &str) -> String {
    let now = crate::utc_timestamp();
    format!(
        "{XML_DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>{}</dc:title>\
         <dc:description>{}</dc:description><dcterms:created xsi:type=\"dcterms:W3CDTF\">{now}</dcterms:created>\
         <dcterms:modified xsi:type=\"dcterms:W3CDTF\">{now}</dcterms:modified></cp:coreProperties>",
        text(title),
        crate::CONVERTED_MARKER
    )
}

fn content_types(parts: &[(String, Vec<u8>)]) -> String {
    let wml = "application/vnd.openxmlformats-officedocument.wordprocessingml";
    let overrides = [
        ("word/document.xml", format!("{wml}.document.main+xml")),
        ("word/styles.xml", format!("{wml}.styles+xml")),
        ("word/settings.xml", format!("{wml}.settings+xml")),
        ("word/numbering.xml", format!("{wml}.numbering+xml")),
        ("word/header1.xml", format!("{wml}.header+xml")),
        ("word/footer1.xml", format!("{wml}.footer+xml")),
        ("docProps/core.xml", "application/vnd.openxmlformats-package.core-properties+xml".to_owned()),
    ];
    let mut xml = format!("{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">");
    for (ext, kind) in [
        ("rels", "application/vnd.openxmlformats-package.relationships+xml"),
        ("xml", "application/xml"),
        ("png", "image/png"),
        ("jpeg", "image/jpeg"),
        ("gif", "image/gif"),
        ("bmp", "image/bmp"),
        ("tiff", "image/tiff"),
    ] {
        let _ = write!(xml, "<Default Extension=\"{ext}\" ContentType=\"{kind}\"/>");
    }
    for (part, kind) in &overrides {
        if parts.iter().any(|(name, _)| name == part) {
            let _ = write!(xml, "<Override PartName=\"/{part}\" ContentType=\"{kind}\"/>");
        }
    }
    xml.push_str("</Types>");
    xml
}

const SETTINGS_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
    <w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
    <w:defaultTabStop w:val=\"720\"/><w:characterSpacingControl w:val=\"doNotCompress\"/>\
    <w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/>\
    </w:compat></w:settings>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
    <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
    <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
    Target=\"word/document.xml\"/>\
    <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" \
    Target=\"docProps/core.xml\"/></Relationships>";
