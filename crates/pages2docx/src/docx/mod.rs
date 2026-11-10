//! Writes the [`model`](crate::model) as a Word document (Office Open XML, ECMA-376).
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

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use fonts::FontMapper;
use numbering::Numbering;
use xml::{attr, text, twips};

use crate::model::{
    Align, Block, Border, Caps, CharProps, Document, HeaderFooter, Inline, InlineKind, LineSpacing, ParaKind, ParaProps,
    Paragraph, Rgb, Strike, TabAlign, Table, Underline, VerticalAlign,
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

/// Built-in Word style for a Pages paragraph role: (style id, name, outline level).
fn heading_style(kind: ParaKind) -> Option<(String, String, Option<u8>)> {
    match kind {
        ParaKind::Normal => None,
        ParaKind::Title => Some(("Title".into(), "Title".into(), None)),
        ParaKind::Subtitle => Some(("Subtitle".into(), "Subtitle".into(), None)),
        ParaKind::Heading(n) => Some((format!("Heading{n}"), format!("heading {n}"), Some(n - 1))),
    }
}

fn hex(c: Rgb) -> String {
    format!("{:02X}{:02X}{:02X}", c.0, c.1, c.2)
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
    headings: BTreeSet<(String, String, Option<u8>)>,
    /// (extra leading, descent) of body lines in 1/100 pt → number of paragraphs.
    line_stats: HashMap<(i64, i64), usize>,
}

/// Writes `doc` to `path`, replacing an existing file only once the new one is complete.
pub(crate) fn write(doc: &Document, path: &Path) -> std::io::Result<()> {
    let mut writer = Writer {
        doc,
        fonts: FontMapper::new(),
        numbering: Numbering::new(),
        rels: Vec::new(),
        links: HashMap::new(),
        images: HashMap::new(),
        media: Vec::new(),
        drawing_id: 0,
        headings: BTreeSet::new(),
        line_stats: HashMap::new(),
    };
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
        out.push_str("<w:p>");
        let ppr = self.ppr(p, part, align);
        out.push_str(&ppr);
        let mut i = 0;
        while i < p.inlines.len() {
            let link = p.inlines[i].link.as_deref();
            let end = i + p.inlines[i..].iter().take_while(|x| x.link.as_deref() == link).count();
            match link.filter(|_| part != Part::Header) {
                Some(url) => {
                    let id = self.hyperlink(url);
                    let _ = write!(out, "<w:hyperlink r:id=\"{id}\" w:history=\"1\">");
                    for inline in &p.inlines[i..end] {
                        self.run(inline, part, out);
                    }
                    out.push_str("</w:hyperlink>");
                }
                None => {
                    for inline in &p.inlines[i..end] {
                        self.run(inline, part, out);
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

    fn run(&mut self, inline: &Inline, part: Part, out: &mut String) {
        let rpr = self.rpr(&inline.props);
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
            InlineKind::Image { index, width, height } => {
                if part != Part::Header {
                    self.image(*index, *width, *height, out);
                }
                return;
            }
        };
        let _ = write!(out, "<w:r>{rpr}{content}</w:r>");
    }

    fn rpr(&mut self, c: &CharProps) -> String {
        let mut s = String::new();
        let (mut bold, mut italic) = (c.bold.unwrap_or(false), c.italic.unwrap_or(false));
        if let Some(font) = &c.font {
            let info = self.fonts.resolve(font);
            bold |= info.implies_bold;
            italic |= info.implies_italic;
            let f = attr(&info.family);
            let _ = write!(s, "<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\" w:eastAsia=\"{f}\"/>");
        }
        if bold {
            s.push_str("<w:b/><w:bCs/>");
        }
        if italic {
            s.push_str("<w:i/><w:iCs/>");
        }
        match c.caps {
            Some(Caps::All) => s.push_str("<w:caps/>"),
            Some(Caps::Small) => s.push_str("<w:smallCaps/>"),
            _ => {}
        }
        match c.strike {
            Some(Strike::Single) => s.push_str("<w:strike/>"),
            Some(Strike::Double) => s.push_str("<w:dstrike/>"),
            _ => {}
        }
        if let Some(color) = c.color.filter(|c| *c != Rgb(0, 0, 0)) {
            let _ = write!(s, "<w:color w:val=\"{}\"/>", hex(color));
        }
        if let Some(shift) = c.baseline_shift.filter(|v| v.abs() >= 0.25) {
            #[allow(clippy::cast_possible_truncation)]
            let half_points = (shift * 2.0).round() as i64;
            let _ = write!(s, "<w:position w:val=\"{half_points}\"/>");
        }
        if let Some(size) = c.size {
            #[allow(clippy::cast_possible_truncation)]
            let half_points = (size * 2.0).round() as i64;
            let _ = write!(s, "<w:sz w:val=\"{half_points}\"/><w:szCs w:val=\"{half_points}\"/>");
        }
        match c.underline {
            Some(Underline::Single) => s.push_str("<w:u w:val=\"single\"/>"),
            Some(Underline::Double) => s.push_str("<w:u w:val=\"double\"/>"),
            _ => {}
        }
        match c.vertical {
            Some(VerticalAlign::Super) => s.push_str("<w:vertAlign w:val=\"superscript\"/>"),
            Some(VerticalAlign::Sub) => s.push_str("<w:vertAlign w:val=\"subscript\"/>"),
            _ => {}
        }
        if s.is_empty() { s } else { format!("<w:rPr>{s}</w:rPr>") }
    }

    fn ppr(&mut self, p: &Paragraph, part: Part, align: Option<&'static str>) -> String {
        let props = &p.props;
        let mut s = String::new();
        if let Some(style) = heading_style(p.kind) {
            let _ = write!(s, "<w:pStyle w:val=\"{}\"/>", style.0);
            self.headings.insert(style);
        }
        if props.keep_with_next == Some(true) {
            s.push_str("<w:keepNext/>");
        }
        if props.keep_lines == Some(true) {
            s.push_str("<w:keepLines/>");
        }
        if props.page_break_before == Some(true) {
            s.push_str("<w:pageBreakBefore/>");
        }
        if props.widow_control == Some(false) {
            s.push_str("<w:widowControl w:val=\"0\"/>");
        }
        let list = p.list.filter(|_| part != Part::Header);
        if let Some(item) = list {
            let id = self.numbering.num_id(item.style, item.restart);
            let _ = write!(s, "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{id}\"/></w:numPr>", item.level);
        }
        if let Some(tabs) = props.tabs.as_ref().filter(|t| !t.is_empty()) {
            s.push_str("<w:tabs>");
            for tab in tabs {
                let val = match tab.align {
                    TabAlign::Left => "left",
                    TabAlign::Center => "center",
                    TabAlign::Right => "right",
                    TabAlign::Decimal => "decimal",
                };
                let _ = write!(s, "<w:tab w:val=\"{val}\" w:pos=\"{}\"/>", twips(tab.position));
            }
            s.push_str("</w:tabs>");
        }
        s.push_str(&self.spacing(props, &p.mark, part));
        if list.is_none() {
            s.push_str(&indentation(props));
        }
        let jc = match align {
            Some("left") => None,
            Some(a) => Some(a),
            None => match props.align {
                Some(Align::Right) => Some("right"),
                Some(Align::Center) => Some("center"),
                Some(Align::Justify) => Some("both"),
                _ => None,
            },
        };
        if let Some(jc) = jc {
            let _ = write!(s, "<w:jc w:val=\"{jc}\"/>");
        }
        s.push_str(&self.rpr(&p.mark));
        if s.is_empty() { s } else { format!("<w:pPr>{s}</w:pPr>") }
    }

    fn spacing(&mut self, props: &ParaProps, mark: &CharProps, part: Part) -> String {
        let mut attrs = Vec::new();
        if let Some(before) = props.space_before.filter(|v| *v > 0.0) {
            attrs.push(format!("w:before=\"{}\"", twips(before)));
        }
        if let Some(after) = props.space_after.filter(|v| *v > 0.0) {
            attrs.push(format!("w:after=\"{}\"", twips(after)));
        }
        let size = mark.size.unwrap_or(11.0);
        match props.line_spacing.unwrap_or(LineSpacing::Relative(1.0)) {
            LineSpacing::Relative(amount) => match self.fonts.pages_line(mark.font.as_deref(), size, mark.is_bold()) {
                Some(line) => {
                    // "At least" still lets a large picture or font in the line grow it.
                    let rule = if amount >= 1.0 { "atLeast" } else { "exact" };
                    attrs.push(format!("w:line=\"{}\" w:lineRule=\"{rule}\"", twips(amount * line.height)));
                    if part == Part::Body {
                        #[allow(clippy::cast_possible_truncation)]
                        let key = (((amount - 1.0).max(0.0) * line.height * 100.0).round() as i64, (line.descent * 100.0).round() as i64);
                        *self.line_stats.entry(key).or_default() += 1;
                    }
                }
                None if props.line_spacing.is_some() => {
                    #[allow(clippy::cast_possible_truncation)]
                    let line = (240.0 * amount).round() as i64;
                    attrs.push(format!("w:line=\"{line}\" w:lineRule=\"auto\""));
                }
                None => {}
            },
            LineSpacing::AtLeast(v) => attrs.push(format!("w:line=\"{}\" w:lineRule=\"atLeast\"", twips(v))),
            LineSpacing::Exactly(v) => attrs.push(format!("w:line=\"{}\" w:lineRule=\"exact\"", twips(v))),
            LineSpacing::Between(v) => attrs.push(format!("w:line=\"{}\" w:lineRule=\"atLeast\"", twips(size * 1.2 + v))),
        }
        if attrs.is_empty() { String::new() } else { format!("<w:spacing {}/>", attrs.join(" ")) }
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
            if let Some(h) = row.height.filter(|h| *h > 0.0) {
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

    fn image(&mut self, index: usize, width: f64, height: f64, out: &mut String) {
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
        let (cx, cy) = ((width * EMU_PER_PT).round() as i64, (height * EMU_PER_PT).round() as i64);
        let _ = write!(
            out,
            "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
             <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Image {n}\"/>\
             <wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>\
             <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"image{n}.{ext}\"/><pic:cNvPicPr/></pic:nvPicPr>\
             <pic:blipFill><a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
             </a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
            ext = image.ext
        );
    }

    fn header_footer(&mut self, hf: &HeaderFooter, root: &str) -> String {
        let fields = [&hf.left, &hf.center, &hf.right];
        let filled: Vec<usize> = (0..3).filter(|i| !fields[*i].is_empty()).collect();
        let body = if let [only] = filled[..] {
            self.blocks(fields[only], Part::Header, Some(["left", "center", "right"][only]))
        } else {
            // Several fields: one line with a centre and a right tab stop, as Pages lays them out.
            let width = self.doc.page.text_width();
            let mut runs = String::new();
            for (i, blocks) in fields.iter().enumerate() {
                if i > 0 {
                    runs.push_str("<w:r><w:tab/></w:r>");
                }
                if let Some(Block::Paragraph(p)) = blocks.first() {
                    for inline in &p.inlines {
                        self.run(inline, Part::Header, &mut runs);
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

    fn styles_xml(&mut self) -> String {
        let defaults = &self.doc.defaults;
        let family = attr(&defaults.font.as_deref().map_or_else(|| "Arial".to_owned(), |f| self.fonts.resolve(f).family));
        #[allow(clippy::cast_possible_truncation)]
        let size = (defaults.size.unwrap_or(11.0) * 2.0).round() as i64;
        let mut styles = String::from(
            "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
             <w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\"><w:name w:val=\"Default Paragraph Font\"/>\
             <w:uiPriority w:val=\"1\"/><w:semiHidden/></w:style>\
             <w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:tblPr>\
             <w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/>\
             <w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>",
        );
        // Heading styles carry only their role; the look comes from direct formatting.
        for (id, name, outline) in &self.headings {
            let level = outline.map(|l| format!("<w:outlineLvl w:val=\"{l}\"/>")).unwrap_or_default();
            let _ = write!(
                styles,
                "<w:style w:type=\"paragraph\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"Normal\"/>\
                 <w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/>{level}</w:pPr></w:style>"
            );
        }
        format!(
            "{XML_DECL}<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" \
             w:cs=\"{family}\" w:eastAsia=\"{family}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:rPrDefault>\
             <w:pPrDefault><w:pPr><w:widowControl/><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
             </w:pPrDefault></w:docDefaults>{styles}</w:styles>"
        )
    }
}

fn indentation(props: &ParaProps) -> String {
    let mut attrs = Vec::new();
    if let Some(left) = props.left_indent.filter(|v| *v != 0.0) {
        attrs.push(format!("w:left=\"{}\"", twips(left)));
    }
    if let Some(right) = props.right_indent.filter(|v| *v != 0.0) {
        attrs.push(format!("w:right=\"{}\"", twips(right)));
    }
    match props.first_indent {
        Some(first) if first > 0.0 => attrs.push(format!("w:firstLine=\"{}\"", twips(first))),
        Some(first) if first < 0.0 => attrs.push(format!("w:hanging=\"{}\"", twips(-first))),
        _ => {}
    }
    if attrs.is_empty() { String::new() } else { format!("<w:ind {}/>", attrs.join(" ")) }
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
