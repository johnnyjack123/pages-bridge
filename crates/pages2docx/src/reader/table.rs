//! Tables (`TST.TableInfoArchive` → `TST.TableModelArchive`): cell storage tiles, string and
//! rich-text lists, row and column headers, and the table and cell styles.

use std::collections::HashMap;

use iwork::Msg;

use super::attach::single_refs;
use super::styles::{CHAR_PROPS, color};
use super::{Ctx, Part, Reader, kinds};
use crate::model::{Block, Border, Cell, Paragraph, Rgb, Row, Table};

/// `TST.TableDataList.ListType` values.
const LIST_STRINGS: u64 = 1;
const LIST_RICH_TEXT: u64 = 8;

/// Seconds between 1970-01-01 and Apple's reference date 2001-01-01, both UTC.
const APPLE_EPOCH_OFFSET: i64 = 978_307_200;

/// A decoded cell of the storage format version 5.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value {
    Number(f64),
    Decimal { mantissa: u128, exponent: i32, negative: bool },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct CellData {
    kind: u8,
    number: Option<Value>,
    seconds: Option<f64>,
    string_id: Option<u64>,
    rich_id: Option<u64>,
}

impl Reader<'_> {
    pub(super) fn read_table(&mut self, info_id: u64, ctx: Ctx) -> Table {
        let pkg = self.pkg;
        let Some(model) = pkg.object(info_id).and_then(|i| i.reference(2)).and_then(|id| pkg.object(id)) else {
            return Table::default();
        };
        let to_usize = |v: Option<u64>| usize::try_from(v.unwrap_or(0)).unwrap_or(0);
        let (rows, cols) = (to_usize(model.uint(6)), to_usize(model.uint(7)));
        let header_rows = to_usize(model.uint(9));
        let header_cols = to_usize(model.uint(10));
        let footer_rows = to_usize(model.uint(11));
        let store = model.msg(4).unwrap_or_default();

        let mut lists: HashMap<u64, HashMap<u64, Msg>> = HashMap::new();
        for f in store.field_numbers() {
            for list_id in single_refs(&store, f) {
                if pkg.kind(list_id) != Some(kinds::DATA_LIST) {
                    continue;
                }
                let Some(list) = pkg.object(list_id) else { continue };
                let entries = lists.entry(list.uint(1).unwrap_or(0)).or_default();
                for entry in list.msgs(3) {
                    if let Some(key) = entry.uint(1) {
                        entries.insert(key, entry);
                    }
                }
            }
        }
        let empty = HashMap::new();
        let strings = lists.get(&LIST_STRINGS).unwrap_or(&empty);
        let rich = lists.get(&LIST_RICH_TEXT).unwrap_or(&empty);

        let (col_sizes, col_hidden) = self.header_sizes(store.reference(2).into_iter().collect());
        let (row_sizes, row_hidden) = self.header_sizes(store.msg(1).map(|m| m.references(2)).unwrap_or_default());
        let cells = self.cells(&store);

        let visible_cols: Vec<usize> = (0..cols).filter(|c| !col_hidden.get(c).copied().unwrap_or(false)).collect();
        let default_width = model.float(17).filter(|w| *w > 0.0).unwrap_or(98.0);
        let mut widths: Vec<f64> =
            visible_cols.iter().map(|c| col_sizes.get(c).copied().unwrap_or(default_width)).collect();
        let total: f64 = widths.iter().sum();
        if total > ctx.max_width {
            for w in &mut widths {
                *w *= ctx.max_width / total;
            }
        }

        let table_style = self.styles.merged(model.reference(3), CHAR_PROPS);
        let band = (table_style.uint(1) == Some(1))
            .then(|| table_style.msg(2).and_then(|f| f.msg(1)).as_ref().and_then(color))
            .flatten();
        let border = table_style
            .msg(32)
            .and_then(|strokes| strokes.msgs(3).into_iter().next())
            .and_then(|layer| layer.msg(1))
            .map(|stroke| Border {
                width: stroke.float(2).unwrap_or(0.5),
                color: stroke.msg(1).as_ref().and_then(color).unwrap_or(Rgb(0, 0, 0)),
            });
        let cell_style = |reader: &mut Self, field: u32| reader.styles.merged(model.reference(field), CHAR_PROPS);
        let body_cell = cell_style(self, 18);
        let fill_of = |m: &Msg| m.msg(1).and_then(|f| f.msg(1)).as_ref().and_then(color);
        let (body_fill, header_row_fill, header_col_fill) =
            (fill_of(&body_cell), fill_of(&cell_style(self, 19)), fill_of(&cell_style(self, 20)));
        let padding = body_cell.msg(9).map_or([4.0; 4], |p| {
            [p.float(1), p.float(2), p.float(3), p.float(4)].map(|v| v.unwrap_or(4.0))
        });

        let mut table = Table { columns: widths.clone(), rows: Vec::new(), border, cell_padding: padding };
        for r in (0..rows).filter(|r| !row_hidden.get(r).copied().unwrap_or(false)) {
            let is_header = r < header_rows;
            let is_footer = !is_header && r + footer_rows >= rows;
            let text_style = model.reference(if is_header { 25 } else if is_footer { 27 } else { 24 });
            let body_index = r.saturating_sub(header_rows);
            let mut row = Row {
                height: (!is_header).then(|| row_sizes.get(&r).copied()).flatten(),
                header: is_header,
                cells: Vec::new(),
            };
            for (&c, &width) in visible_cols.iter().zip(&widths) {
                let in_header_col = c < header_cols && !is_header;
                let fill = if is_header {
                    header_row_fill
                } else if in_header_col {
                    header_col_fill
                } else if band.is_some() && body_index % 2 == 1 {
                    band
                } else {
                    body_fill
                };
                let style = if in_header_col { model.reference(26) } else { text_style };
                let cell_ctx = Ctx { part: Part::Cell, max_width: width };
                let blocks = self.cell_blocks(cells.get(&(r, c)), strings, rich, style, cell_ctx);
                row.cells.push(Cell { blocks, fill });
            }
            table.rows.push(row);
        }
        table
    }

    /// Sizes and hidden flags of row or column headers (`TST.HeaderStorageBucket`).
    fn header_sizes(&self, buckets: Vec<u64>) -> (HashMap<usize, f64>, HashMap<usize, bool>) {
        let (mut sizes, mut hidden) = (HashMap::new(), HashMap::new());
        for bucket in buckets.into_iter().filter_map(|b| self.pkg.object(b)) {
            for header in bucket.msgs(2) {
                let Some(index) = header.uint(1).and_then(|i| usize::try_from(i).ok()) else { continue };
                if let Some(size) = header.float(2).filter(|s| *s > 0.0) {
                    sizes.insert(index, size);
                }
                hidden.insert(index, header.uint(3).unwrap_or(0) != 0);
            }
        }
        (sizes, hidden)
    }

    /// All cells of the table's tiles, keyed by (row, column).
    fn cells(&self, store: &Msg) -> HashMap<(usize, usize), CellData> {
        let mut out = HashMap::new();
        let tiles = store.msg(3).unwrap_or_default();
        let tile_rows = tiles.uint(2).filter(|s| *s > 0).unwrap_or(256);
        for entry in tiles.msgs(1) {
            let Some(tile) = entry.reference(2).and_then(|id| self.pkg.object(id)) else { continue };
            let base = entry.uint(1).unwrap_or(0) * tile_rows;
            for row_info in tile.msgs(5) {
                let Ok(row) = usize::try_from(base + row_info.uint(1).unwrap_or(0)) else { continue };
                // Fields 6 and 7 hold the current format; 3 and 4 an older one this does not read.
                let (Some(storage), Some(offsets)) = (row_info.bytes(6), row_info.bytes(7)) else { continue };
                let scale = if row_info.bool(8).unwrap_or(false) { 4 } else { 1 };
                for (col, pair) in offsets.chunks_exact(2).enumerate() {
                    let offset = u16::from_le_bytes([pair[0], pair[1]]);
                    if offset == u16::MAX {
                        continue;
                    }
                    if let Some(cell) = parse_cell(storage, usize::from(offset) * scale) {
                        out.insert((row, col), cell);
                    }
                }
            }
        }
        out
    }

    fn cell_blocks(
        &mut self,
        cell: Option<&CellData>,
        strings: &HashMap<u64, Msg>,
        rich: &HashMap<u64, Msg>,
        style: Option<u64>,
        ctx: Ctx,
    ) -> Vec<Block> {
        let props = self.styles.para_props(style);
        let mark = self.styles.char_props(style);
        let mut blocks = Vec::new();
        if let Some(cell) = cell {
            if cell.kind == 9 {
                // Rich text: list entry → TST.RichTextPayloadArchive → text storage.
                let storage = cell
                    .rich_id
                    .and_then(|id| rich.get(&id))
                    .and_then(|entry| entry.field_numbers().flat_map(|f| single_refs(entry, f)).next())
                    .and_then(|payload| self.pkg.object(payload))
                    .and_then(|payload| payload.reference(1))
                    .filter(|s| self.pkg.kind(*s) == Some(kinds::STORAGE));
                if let Some(storage) = storage {
                    blocks = self.read_storage(storage, ctx);
                }
            } else {
                let string = cell.string_id.and_then(|id| strings.get(&id)).and_then(|e| e.string(3));
                let text = self.cell_text(cell, string);
                for line in text.split('\n').filter(|_| !text.is_empty()) {
                    let inlines = if line.is_empty() {
                        Vec::new()
                    } else {
                        vec![crate::model::Inline {
                            kind: crate::model::InlineKind::Text(line.to_owned()),
                            props: mark.clone(),
                            link: None,
                        }]
                    };
                    blocks.push(Block::Paragraph(Paragraph {
                        props: props.clone(),
                        mark: mark.clone(),
                        inlines,
                        ..Paragraph::default()
                    }));
                }
            }
        }
        if blocks.is_empty() {
            blocks.push(Block::Paragraph(Paragraph { props, mark, ..Paragraph::default() }));
        }
        blocks
    }

    fn cell_text(&self, cell: &CellData, string: Option<String>) -> String {
        let number = || -> Option<String> {
            let s = match cell.number? {
                Value::Number(v) => format_float(v),
                Value::Decimal { mantissa, exponent, negative } => format_decimal(mantissa, exponent, negative),
            };
            Some(if self.decimal_comma { s.replace('.', ",") } else { s })
        };
        match cell.kind {
            3 => string.unwrap_or_default(),
            2 | 10 => number().unwrap_or_default(),
            5 => cell.seconds.map(format_date).unwrap_or_default(),
            6 => match cell.number {
                Some(Value::Number(v)) if v != 0.0 => crate::messages::CELL_TRUE.to_owned(),
                Some(_) => crate::messages::CELL_FALSE.to_owned(),
                None => String::new(),
            },
            7 => match cell.number {
                #[allow(clippy::cast_possible_truncation)]
                Some(Value::Number(v)) => {
                    let total = v.round() as i64;
                    format!("{}:{:02}:{:02}", total / 3600, total % 3600 / 60, total % 60)
                }
                _ => String::new(),
            },
            8 => crate::messages::CELL_ERROR.to_owned(),
            _ => String::new(),
        }
    }
}

/// Decodes one cell: a 12-byte header (version 5, cell type, flags at offset 8), then the
/// values the flags announce, in flag order.
fn parse_cell(buf: &[u8], offset: usize) -> Option<CellData> {
    let header = buf.get(offset..offset + 12)?;
    if header[0] != 5 {
        return None;
    }
    let flags = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let mut pos = offset + 12;
    let mut take = |n: usize| -> Option<&[u8]> {
        let bytes = buf.get(pos..pos + n)?;
        pos += n;
        Some(bytes)
    };
    let mut cell = CellData { kind: header[1], ..CellData::default() };
    if flags & 0x1 != 0 {
        cell.number = Some(decimal128(take(16)?));
    }
    if flags & 0x2 != 0 {
        let b = take(8)?;
        cell.number = Some(Value::Number(f64::from_le_bytes(b.try_into().ok()?)));
    }
    if flags & 0x4 != 0 {
        let b = take(8)?;
        cell.seconds = Some(f64::from_le_bytes(b.try_into().ok()?));
    }
    let id = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    if flags & 0x8 != 0 {
        cell.string_id = Some(u64::from(id(take(4)?)));
    }
    if flags & 0x10 != 0 {
        cell.rich_id = Some(u64::from(id(take(4)?)));
    }
    Some(cell)
}

/// IEEE 754-2008 decimal128 with binary integer significand, as Numbers stores values.
fn decimal128(b: &[u8]) -> Value {
    let exponent = ((i32::from(b[15] & 0x7F) << 7) | (i32::from(b[14]) >> 1)) - 0x1820;
    let mut mantissa = u128::from(b[14] & 1);
    for &byte in b[..14].iter().rev() {
        mantissa = mantissa * 256 + u128::from(byte);
    }
    Value::Decimal { mantissa, exponent, negative: b[15] & 0x80 != 0 }
}

fn format_decimal(mantissa: u128, exponent: i32, negative: bool) -> String {
    let digits = mantissa.to_string();
    let mut s = if exponent >= 0 {
        let zeros = usize::try_from(exponent).unwrap_or(0).min(64);
        format!("{digits}{}", "0".repeat(zeros))
    } else {
        let frac = usize::try_from(-exponent).unwrap_or(0);
        let padded = if digits.len() <= frac { format!("{}{digits}", "0".repeat(frac - digits.len() + 1)) } else { digits };
        let (int, dec) = padded.split_at(padded.len() - frac);
        let dec = dec.trim_end_matches('0');
        if dec.is_empty() { int.to_owned() } else { format!("{int}.{dec}") }
    };
    if negative && s != "0" {
        s.insert(0, '-');
    }
    s
}

fn format_float(v: f64) -> String {
    // Rust prints the shortest representation that round-trips, never in exponent form.
    let s = format!("{v}");
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

/// Seconds since 2001-01-01 UTC as `DD.MM.YYYY`, with the time when there is one.
fn format_date(seconds: f64) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let unix = seconds.round() as i64 + APPLE_EPOCH_OFFSET;
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (y, m, d) = crate::civil_from_days(days);
    if secs == 0 { format!("{d:02}.{m:02}.{y}") } else { format!("{d:02}.{m:02}.{y} {:02}:{:02}", secs / 3600, secs % 3600 / 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_cell() {
        // From a real table: version 5, type 3 (text), flags 0x21008, string id 1.
        let buf = [5, 3, 0, 0, 0, 0, 0, 0, 0x08, 0x10, 0x02, 0x00, 1, 0, 0, 0, 5, 0, 0, 0, 1, 0, 0, 0];
        let cell = parse_cell(&buf, 0).unwrap();
        assert_eq!((cell.kind, cell.string_id), (3, Some(1)));
    }

    #[test]
    fn formats_decimals() {
        assert_eq!(format_decimal(12345, -2, false), "123.45");
        assert_eq!(format_decimal(5, -3, true), "-0.005");
        assert_eq!(format_decimal(1200, -2, false), "12");
        assert_eq!(format_decimal(7, 2, false), "700");
    }

    #[test]
    fn formats_dates() {
        assert_eq!(format_date(0.0), "01.01.2001");
        assert_eq!(format_date(86_400.0 * 31.0 + 3600.0 * 13.5), "01.02.2001 13:30");
    }
}
