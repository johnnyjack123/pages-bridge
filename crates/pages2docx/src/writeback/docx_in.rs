//! Reads the body of an edited `.docx`: paragraphs with their characters and paragraph style,
//! the formatting that write-back can carry (bold, italic, underline, strikethrough), links,
//! list levels and tables.
//!
//! Characters are represented the way Pages stores them: `\t`, U+2028 for a line break, U+000C
//! for a page break, U+000E for a column break and U+FFFC for a picture or other object.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use super::xml::{Event, Scanner, attr, local};

/// A run's effective formatting: its own properties over its character style, its paragraph's
/// style and the document defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Format {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// The properties one layer (document defaults, a style, a run) sets; `None` where it leaves
/// them to the layer below. Single and double strikethrough are separate properties in Word.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Layer {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    double_strike: Option<bool>,
}

impl Layer {
    fn set(&mut self, element: &str, on: bool) {
        let field = match element {
            "b" => &mut self.bold,
            "i" => &mut self.italic,
            "u" => &mut self.underline,
            "strike" => &mut self.strike,
            "dstrike" => &mut self.double_strike,
            _ => return,
        };
        *field = Some(on);
    }

    fn over(self, below: Layer) -> Layer {
        Layer {
            bold: self.bold.or(below.bold),
            italic: self.italic.or(below.italic),
            underline: self.underline.or(below.underline),
            strike: self.strike.or(below.strike),
            double_strike: self.double_strike.or(below.double_strike),
        }
    }

    fn format(self) -> Format {
        Format {
            bold: self.bold.unwrap_or(false),
            italic: self.italic.unwrap_or(false),
            underline: self.underline.unwrap_or(false),
            strike: self.strike.unwrap_or(false) || self.double_strike.unwrap_or(false),
        }
    }
}

/// Character properties of `styles.xml`: document defaults, paragraph and character styles.
#[derive(Debug, Default)]
struct StyleSheet {
    defaults: Layer,
    /// Style id → (based on, properties).
    paragraph: HashMap<String, (Option<String>, Layer)>,
    character: HashMap<String, (Option<String>, Layer)>,
    /// The paragraph style of paragraphs that name none.
    default_paragraph: Option<String>,
}

impl StyleSheet {
    fn parse(xml: &str) -> Result<Self, String> {
        let mut sheet = StyleSheet::default();
        // (paragraph style?, id, based on, properties) of the style being read.
        let mut current: Option<(bool, String, Option<String>, Layer)> = None;
        let (mut in_defaults, mut in_rpr) = (false, false);
        let mut skipping = 0usize;
        let mut scanner = Scanner::new(xml);
        while let Some(event) = scanner.next_event()? {
            match event {
                Event::Start { name, attrs, empty } => {
                    let name = local(name);
                    if skipping > 0 {
                        skipping += usize::from(!empty);
                        continue;
                    }
                    match name {
                        // Former formatting of tracked changes, and table style parts.
                        "rPrChange" | "pPrChange" | "tblStylePr" if !empty => skipping = 1,
                        "rPrDefault" => in_defaults = !empty,
                        "style" => {
                            let kind = attr(&attrs, "type").unwrap_or("paragraph");
                            let id = attr(&attrs, "styleId").unwrap_or_default().to_owned();
                            let is_default = attr(&attrs, "default").is_some_and(|v| matches!(v, "1" | "true" | "on"));
                            if kind == "paragraph" && is_default {
                                sheet.default_paragraph = Some(id.clone());
                            }
                            if matches!(kind, "paragraph" | "character") && !empty {
                                current = Some((kind == "paragraph", id, None, Layer::default()));
                            }
                        }
                        "basedOn" => {
                            if let Some(style) = current.as_mut() {
                                style.2 = attr(&attrs, "val").map(str::to_owned);
                            }
                        }
                        "rPr" if !empty => in_rpr = true,
                        "b" | "i" | "u" | "strike" | "dstrike" if in_rpr => {
                            let on = !is_off(&attrs);
                            if let Some(style) = current.as_mut() {
                                style.3.set(name, on);
                            } else if in_defaults {
                                sheet.defaults.set(name, on);
                            }
                        }
                        _ => {}
                    }
                }
                Event::End { name } => {
                    if skipping > 0 {
                        skipping -= 1;
                        continue;
                    }
                    match local(name) {
                        "rPrDefault" => in_defaults = false,
                        "rPr" => in_rpr = false,
                        "style" => {
                            if let Some((is_paragraph, id, based_on, layer)) = current.take() {
                                let map = if is_paragraph { &mut sheet.paragraph } else { &mut sheet.character };
                                map.insert(id, (based_on, layer));
                            }
                        }
                        _ => {}
                    }
                }
                Event::Text(_) => {}
            }
        }
        Ok(sheet)
    }

    /// The properties of style `id` with everything it inherits.
    fn resolve(map: &HashMap<String, (Option<String>, Layer)>, id: Option<&str>) -> Layer {
        let mut layer = Layer::default();
        let mut current = id;
        for _ in 0..32 {
            let Some((based_on, own)) = current.and_then(|id| map.get(id)) else { break };
            layer = layer.over(*own);
            current = based_on.as_deref();
        }
        layer
    }

    /// Word's order: run properties over the character style over the paragraph style over the
    /// document defaults. (Bold and italic in styles toggle in Word; Pages-made documents never
    /// rely on that.)
    fn format(&self, paragraph_style: Option<&str>, run_style: Option<&str>, run: Layer) -> Format {
        let paragraph_style = paragraph_style.or(self.default_paragraph.as_deref());
        run.over(Self::resolve(&self.character, run_style))
            .over(Self::resolve(&self.paragraph, paragraph_style))
            .over(self.defaults)
            .format()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Run {
    pub start: usize,
    pub end: usize,
    pub format: Format,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Para {
    /// Style id; `None` for the default paragraph style.
    pub style: Option<String>,
    pub chars: Vec<char>,
    pub runs: Vec<Run>,
    /// 0-based level when the paragraph is numbered or bulleted.
    pub list_level: Option<u8>,
}

impl Para {
    pub(crate) fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Formatting and link of the character at `i`.
    pub(crate) fn char_format(&self, i: usize) -> (Format, Option<&str>) {
        self.runs
            .iter()
            .find(|r| r.start <= i && i < r.end)
            .map_or((Format::default(), None), |r| (r.format, r.link.as_deref()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    Para(Para),
    /// Rows of cells, each cell a list of paragraphs.
    Table(Vec<Vec<Vec<Para>>>),
}

pub(crate) fn read(path: &Path) -> Result<Vec<Block>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut part = |name: &str| -> Result<Option<String>, String> {
        let Ok(mut entry) = archive.by_name(name) else { return Ok(None) };
        let mut text = String::new();
        entry.read_to_string(&mut text).map_err(|e| e.to_string())?;
        Ok(Some(text))
    };
    let document = part("word/document.xml")?.ok_or("word/document.xml missing")?;
    let links = part("word/_rels/document.xml.rels")?.map(|rels| hyperlink_targets(&rels)).transpose()?.unwrap_or_default();
    let styles = part("word/styles.xml")?.map(|xml| StyleSheet::parse(&xml)).transpose()?.unwrap_or_default();
    parse_body(&document, &links, &styles)
}

fn hyperlink_targets(rels: &str) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    let mut scanner = Scanner::new(rels);
    while let Some(event) = scanner.next_event()? {
        if let Event::Start { name, attrs, .. } = event {
            if local(name) == "Relationship" && attr(&attrs, "Type").is_some_and(|t| t.ends_with("/hyperlink")) {
                if let (Some(id), Some(target)) = (attr(&attrs, "Id"), attr(&attrs, "Target")) {
                    out.insert(id.to_owned(), target.to_owned());
                }
            }
        }
    }
    Ok(out)
}

/// Elements whose content is not part of the document's current text, or would be counted
/// twice: deleted text, field instructions, the old formatting of tracked changes, the
/// fallback copy of alternate content, and the text inside drawings.
const SKIPPED: &[&str] = &[
    "del", "moveFrom", "instrText", "delInstrText", "rPrChange", "pPrChange", "Fallback", "drawing", "pict", "object",
    "footnoteReference", "endnoteReference", "commentReference",
];

/// A run being read: where it starts, its own properties and its character style.
struct OpenRun {
    start: usize,
    own: Layer,
    style: Option<String>,
}

struct State<'s> {
    styles: &'s StyleSheet,
    blocks: Vec<Block>,
    tables: Vec<Vec<Vec<Vec<Para>>>>,
    para: Option<Para>,
    run: Option<OpenRun>,
    links: Vec<Option<String>>,
    in_ppr: bool,
    in_rpr: bool,
    in_text: bool,
    /// Depth inside a skipped element; 0 when not skipping.
    skipping: usize,
}

impl<'s> State<'s> {
    fn new(styles: &'s StyleSheet) -> Self {
        Self {
            styles,
            blocks: Vec::new(),
            tables: Vec::new(),
            para: None,
            run: None,
            links: Vec::new(),
            in_ppr: false,
            in_rpr: false,
            in_text: false,
            skipping: 0,
        }
    }

    fn push_char(&mut self, c: char) {
        if let Some(p) = self.para.as_mut() {
            p.chars.push(c);
        }
    }

    fn end_run(&mut self) {
        let (Some(run), Some(p)) = (self.run.take(), self.para.as_mut()) else { return };
        if p.chars.len() > run.start {
            let link = self.links.last().cloned().flatten();
            let format = self.styles.format(p.style.as_deref(), run.style.as_deref(), run.own);
            p.runs.push(Run { start: run.start, end: p.chars.len(), format, link });
        }
    }

    fn end_paragraph(&mut self) {
        self.end_run();
        let Some(p) = self.para.take() else { return };
        match self.tables.last_mut().and_then(|t| t.last_mut()).and_then(|row| row.last_mut()) {
            Some(cell) => cell.push(p),
            None => self.blocks.push(Block::Para(p)),
        }
    }
}

fn is_off(attrs: &[(&str, String)]) -> bool {
    attr(attrs, "val").is_some_and(|v| matches!(v, "false" | "0" | "off" | "none"))
}

fn parse_body(xml: &str, links: &HashMap<String, String>, styles: &StyleSheet) -> Result<Vec<Block>, String> {
    let mut s = State::new(styles);
    let mut scanner = Scanner::new(xml);
    while let Some(event) = scanner.next_event()? {
        match event {
            Event::Start { name, attrs, empty } => {
                let name = local(name);
                if s.skipping > 0 {
                    if !empty {
                        s.skipping += 1;
                    }
                    continue;
                }
                if SKIPPED.contains(&name) {
                    // A picture still occupies its place in the text.
                    if matches!(name, "drawing" | "pict" | "object") && s.run.is_some() {
                        s.push_char('\u{FFFC}');
                    }
                    if !empty {
                        s.skipping = 1;
                    }
                    continue;
                }
                match name {
                    "tbl" if !empty => s.tables.push(Vec::new()),
                    "tr" => {
                        if let Some(t) = s.tables.last_mut() {
                            t.push(Vec::new());
                        }
                    }
                    "tc" => {
                        if let Some(row) = s.tables.last_mut().and_then(|t| t.last_mut()) {
                            row.push(Vec::new());
                        }
                    }
                    "p" => {
                        s.end_paragraph();
                        s.para = Some(Para::default());
                        if empty {
                            s.end_paragraph();
                        }
                    }
                    "pPr" if !empty => s.in_ppr = true,
                    "pStyle" if s.in_ppr => {
                        if let Some(p) = s.para.as_mut() {
                            p.style = attr(&attrs, "val").map(str::to_owned);
                        }
                    }
                    "ilvl" if s.in_ppr => {
                        if let Some(p) = s.para.as_mut() {
                            p.list_level = Some(attr(&attrs, "val").and_then(|v| v.parse().ok()).unwrap_or(0));
                        }
                    }
                    "numId" if s.in_ppr => {
                        if let Some(p) = s.para.as_mut() {
                            if attr(&attrs, "val") == Some("0") {
                                p.list_level = None;
                            } else if p.list_level.is_none() {
                                p.list_level = Some(0);
                            }
                        }
                    }
                    "hyperlink" if !empty => s.links.push(attr(&attrs, "id").and_then(|id| links.get(id).cloned())),
                    "r" if !s.in_ppr => {
                        s.end_run();
                        let start = s.para.as_ref().map_or(0, |p| p.chars.len());
                        s.run = Some(OpenRun { start, own: Layer::default(), style: None });
                    }
                    "rPr" if s.run.is_some() && !empty => s.in_rpr = true,
                    "rStyle" if s.in_rpr => {
                        if let Some(run) = s.run.as_mut() {
                            run.style = attr(&attrs, "val").map(str::to_owned);
                        }
                    }
                    "b" | "i" | "u" | "strike" | "dstrike" if s.in_rpr => {
                        if let Some(run) = s.run.as_mut() {
                            run.own.set(name, !is_off(&attrs));
                        }
                    }
                    "t" if !empty && s.run.is_some() => s.in_text = true,
                    "tab" if s.run.is_some() && !s.in_ppr => s.push_char('\t'),
                    "br" if s.run.is_some() => s.push_char(match attr(&attrs, "type") {
                        Some("page") => '\u{0C}',
                        Some("column") => '\u{0E}',
                        _ => '\u{2028}',
                    }),
                    "cr" if s.run.is_some() => s.push_char('\u{2028}'),
                    "softHyphen" if s.run.is_some() => s.push_char('\u{AD}'),
                    "noBreakHyphen" if s.run.is_some() => s.push_char('\u{2011}'),
                    _ => {}
                }
            }
            Event::End { name } => {
                if s.skipping > 0 {
                    s.skipping -= 1;
                    continue;
                }
                match local(name) {
                    "tbl" => {
                        s.end_paragraph();
                        if let Some(table) = s.tables.pop() {
                            match s.tables.last_mut().and_then(|t| t.last_mut()).and_then(|row| row.last_mut()) {
                                // A table inside a cell: keep its text in the outer cell.
                                Some(cell) => cell.extend(table.into_iter().flatten().flatten()),
                                None => s.blocks.push(Block::Table(table)),
                            }
                        }
                    }
                    "p" => s.end_paragraph(),
                    "pPr" => s.in_ppr = false,
                    "hyperlink" => {
                        s.end_run();
                        s.links.pop();
                    }
                    "r" => s.end_run(),
                    "rPr" => s.in_rpr = false,
                    "t" => s.in_text = false,
                    _ => {}
                }
            }
            Event::Text(text) => {
                if s.in_text && s.skipping == 0 {
                    for c in text.chars() {
                        s.push_char(c);
                    }
                }
            }
        }
    }
    s.end_paragraph();
    Ok(s.blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_runs_links_lists_breaks_and_tables() {
        let xml = r#"<w:document><w:body>
            <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="3"/></w:numPr><w:rPr><w:b/></w:rPr></w:pPr>
              <w:r><w:rPr><w:b/><w:i w:val="0"/></w:rPr><w:t xml:space="preserve">Fett </w:t></w:r>
              <w:hyperlink r:id="rId9"><w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t>Link</w:t></w:r></w:hyperlink>
              <w:r><w:tab/><w:t>x</w:t><w:br w:type="page"/></w:r>
              <w:del><w:r><w:delText>weg</w:delText></w:r></w:del>
              <w:r><w:drawing><wp:inline><w:p><w:r><w:t>im Bild</w:t></w:r></w:p></wp:inline></w:drawing></w:r>
            </w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl>
            <w:p/>
        </w:body></w:document>"#;
        let links = HashMap::from([("rId9".to_owned(), "https://example.org".to_owned())]);
        let blocks = parse_body(xml, &links, &StyleSheet::default()).unwrap();
        let Block::Para(p) = &blocks[0] else { panic!("paragraph expected") };
        assert_eq!(p.text(), "Fett Link\tx\u{0C}\u{FFFC}");
        assert_eq!(p.list_level, Some(1));
        assert_eq!(p.char_format(0).0, Format { bold: true, ..Format::default() });
        assert_eq!(p.char_format(5), (Format { underline: true, ..Format::default() }, Some("https://example.org")));
        let Block::Table(rows) = &blocks[1] else { panic!("table expected") };
        assert_eq!((rows.len(), rows[0].len(), rows[0][0][0].text(), rows[0][1][0].text()), (1, 2, "A1".to_owned(), String::new()));
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn formatting_comes_from_styles_unless_the_run_sets_it() {
        let styles = r#"<w:styles><w:docDefaults><w:rPrDefault><w:rPr><w:i/></w:rPr></w:rPrDefault></w:docDefaults>
            <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:rPr><w:i w:val="0"/></w:rPr></w:style>
            <w:style w:type="paragraph" w:styleId="Heading1"><w:basedOn w:val="Normal"/><w:rPr><w:b/>
              <w:rPrChange><w:rPr><w:u w:val="single"/></w:rPr></w:rPrChange></w:rPr></w:style>
            <w:style w:type="paragraph" w:styleId="Heading2"><w:basedOn w:val="Heading1"/></w:style>
            <w:style w:type="character" w:styleId="Strong"><w:rPr><w:strike/></w:rPr></w:style>
            </w:styles>"#;
        let sheet = StyleSheet::parse(styles).unwrap();
        let xml = r#"<w:document><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>A</w:t></w:r>
              <w:r><w:rPr><w:rStyle w:val="Strong"/><w:b w:val="0"/></w:rPr><w:t>B</w:t></w:r></w:p>
            <w:p><w:r><w:t>C</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let blocks = parse_body(xml, &HashMap::new(), &sheet).unwrap();
        let (Block::Para(heading), Block::Para(normal)) = (&blocks[0], &blocks[1]) else { panic!("paragraphs expected") };
        assert_eq!(heading.style.as_deref(), Some("Heading2"));
        assert_eq!(heading.char_format(0).0, Format { bold: true, ..Format::default() }, "inherited through basedOn");
        assert_eq!(heading.char_format(1).0, Format { strike: true, ..Format::default() }, "run over character style");
        assert_eq!((normal.style.as_deref(), normal.char_format(0).0), (None, Format::default()), "Normal over the defaults");
    }
}
