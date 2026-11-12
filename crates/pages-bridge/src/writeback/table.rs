//! Writing edited cell text back into Pages tables.
//!
//! Text cells point into the table's string list (`TST.TableDataList`, list type 1), which keeps a
//! reference count per string. Cells live in tiles of up to 256 rows: per stored row a buffer of
//! cell records plus a 16-bit offset per column (`0xFFFF` = no cell), once in the current format
//! (fields 6/7) and once in a pre-2013 format (fields 3/4) that Pages fills with empty placeholder
//! records. Rows without any cell are not stored at all.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use iwork::{Edits, Encoder, Msg, Package};

use super::Report;
use super::docx_in::Para as DocxPara;
use crate::messages;
use crate::model::{Block, Table};
use crate::reader::attach::single_refs;
use crate::reader::kinds;

const LIST_STRINGS: u64 = 1;
const CELL_TEXT: u8 = 3;
const NO_CELL: u16 = u16::MAX;
/// An empty record of the pre-2013 cell format, as Pages writes for every cell.
const LEGACY_RECORD: [u8; 12] = [4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
/// Offset slots per stored row in Pages' own files.
const DEFAULT_SLOTS: usize = 255;
/// The cell storage version Pages writes in field 5 of each stored row.
const ROW_STORAGE_VERSION: u64 = 5;

/// Cell text as the reader shows it: paragraphs joined by line feeds.
fn model_cell_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => Some(p.plain_text()),
            Block::Table(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn docx_cell_text(paras: &[DocxPara]) -> String {
    paras.iter().map(DocxPara::text).collect::<Vec<_>>().join("\n")
}

/// Offset of the string id inside a version-5 cell record: after the 12-byte header and the
/// number and date values its flags announce.
fn string_id_offset(cell: &[u8]) -> Option<usize> {
    let flags = u32::from_le_bytes(cell.get(8..12)?.try_into().ok()?);
    if flags & 0x8 == 0 {
        return None;
    }
    let decimal = if flags & 0x1 != 0 { 16 } else { 0 };
    let double = if flags & 0x2 != 0 { 8 } else { 0 };
    let date = if flags & 0x4 != 0 { 8 } else { 0 };
    let offset = 12 + decimal + double + date;
    (cell.len() >= offset + 4).then_some(offset)
}

fn string_id(cell: &[u8]) -> Option<u64> {
    let at = string_id_offset(cell)?;
    Some(u64::from(u32::from_le_bytes(cell[at..at + 4].try_into().ok()?)))
}

fn with_string_id(cell: &[u8], id: u64) -> Vec<u8> {
    let mut out = cell.to_vec();
    if let Some(at) = string_id_offset(cell) {
        out[at..at + 4].copy_from_slice(&u32::try_from(id).unwrap_or(u32::MAX).to_le_bytes());
    }
    out
}

/// A text cell holding only a string id, for tables without a text cell to copy.
fn minimal_text_cell(id: u64) -> Vec<u8> {
    let mut cell = vec![5, CELL_TEXT, 0, 0, 0, 0, 0, 0, 0x08, 0, 0, 0];
    cell.extend_from_slice(&u32::try_from(id).unwrap_or(u32::MAX).to_le_bytes());
    cell
}

/// One stored row of a tile, split per column.
struct TileRow {
    /// Position of the row's message in the tile; `None` for a row added here.
    message: Option<usize>,
    /// Row number within the tile (field 1).
    row_in_tile: u64,
    cells: Vec<Option<Vec<u8>>>,
    legacy: Vec<Option<Vec<u8>>>,
    offset_slots: usize,
    legacy_slots: usize,
    wide: bool,
}

struct Tile<'p> {
    id: u64,
    /// Table row of the tile's first row.
    base: u64,
    msg: Msg<'p>,
    rows: Vec<TileRow>,
}

impl<'p> Tile<'p> {
    fn load(pkg: &'p Package, id: u64, base: u64) -> Option<Self> {
        let msg = pkg.object(id)?;
        let rows = msg
            .msgs(5)
            .iter()
            .enumerate()
            .map(|(index, info)| {
                let wide = info.bool(8).unwrap_or(false);
                let (storage, offsets) = (info.bytes(6).unwrap_or_default(), info.bytes(7).unwrap_or_default());
                let (legacy, legacy_offsets) = (info.bytes(3).unwrap_or_default(), info.bytes(4).unwrap_or_default());
                TileRow {
                    message: Some(index),
                    row_in_tile: info.uint(1).unwrap_or(0),
                    cells: split_cells(storage, offsets, if wide { 4 } else { 1 }),
                    legacy: split_cells(legacy, legacy_offsets, 1),
                    offset_slots: offsets.len() / 2,
                    legacy_slots: legacy_offsets.len() / 2,
                    wide,
                }
            })
            .collect();
        Some(Self { id, base, msg, rows })
    }

    /// The stored row for table row `row`, created empty if the tile has none for it yet.
    fn row_mut(&mut self, row: u64) -> &mut TileRow {
        let row_in_tile = row - self.base;
        if let Some(i) = self.rows.iter().position(|r| r.row_in_tile == row_in_tile) {
            return &mut self.rows[i];
        }
        let (offset_slots, legacy_slots) =
            self.rows.first().map_or((DEFAULT_SLOTS, DEFAULT_SLOTS), |r| (r.offset_slots, r.legacy_slots));
        let at = self.rows.partition_point(|r| r.row_in_tile < row_in_tile);
        self.rows.insert(
            at,
            TileRow { message: None, row_in_tile, cells: Vec::new(), legacy: Vec::new(), offset_slots, legacy_slots, wide: false },
        );
        &mut self.rows[at]
    }

    fn encode(&self) -> Vec<u8> {
        let infos = self.msg.msgs(5);
        let mut e = Encoder::new();
        e.copy_except(&self.msg, &[4, 5]);
        // Field 4 counts the rows the tile holds; Pages' own files store them without gaps.
        e.uint(4, self.rows.iter().map(|r| r.row_in_tile + 1).max().unwrap_or(0));
        for row in &self.rows {
            let (storage, offsets) = join_cells(&row.cells, row.offset_slots, row.wide);
            let (legacy, legacy_offsets) = join_cells(&row.legacy, row.legacy_slots, false);
            let mut r = Encoder::new();
            match row.message.and_then(|i| infos.get(i)) {
                Some(info) => {
                    r.copy_except(info, &[1, 2, 3, 4, 6, 7]);
                }
                None => {
                    r.uint(5, ROW_STORAGE_VERSION);
                }
            }
            r.uint(1, row.row_in_tile).uint(2, row.cells.iter().flatten().count() as u64);
            r.bytes(3, &legacy).bytes(4, &legacy_offsets).bytes(6, &storage).bytes(7, &offsets);
            e.bytes(5, &r.finish());
        }
        e.finish()
    }
}

fn split_cells(storage: &[u8], offsets: &[u8], scale: usize) -> Vec<Option<Vec<u8>>> {
    let starts: Vec<Option<usize>> = offsets
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .map(|o| (o != NO_CELL).then(|| usize::from(o) * scale))
        .collect();
    let mut sorted: Vec<usize> = starts.iter().flatten().copied().collect();
    sorted.sort_unstable();
    starts
        .iter()
        .map(|start| {
            let start = (*start)?;
            let end = sorted.iter().copied().find(|s| *s > start).unwrap_or(storage.len());
            storage.get(start..end).map(<[u8]>::to_vec)
        })
        .collect()
}

fn join_cells(cells: &[Option<Vec<u8>>], slots: usize, wide: bool) -> (Vec<u8>, Vec<u8>) {
    let mut storage = Vec::new();
    let mut offsets = Vec::with_capacity(slots * 2);
    for i in 0..slots.max(cells.len()) {
        match cells.get(i).and_then(Option::as_ref) {
            Some(cell) => {
                let at = if wide { storage.len() / 4 } else { storage.len() };
                offsets.extend_from_slice(&u16::try_from(at).unwrap_or(NO_CELL - 1).to_le_bytes());
                storage.extend_from_slice(cell);
            }
            None => offsets.extend_from_slice(&NO_CELL.to_le_bytes()),
        }
    }
    (storage, offsets)
}

/// Visible indices among `count` row or column headers, skipping hidden ones as the reader does.
fn visible(pkg: &Package, buckets: &[u64], count: usize) -> Vec<usize> {
    let mut hidden = HashMap::new();
    for bucket in buckets.iter().filter_map(|b| pkg.object(*b)) {
        for h in bucket.msgs(2) {
            if let Some(i) = h.uint(1).and_then(|i| usize::try_from(i).ok()) {
                hidden.insert(i, h.uint(3).unwrap_or(0) != 0);
            }
        }
    }
    (0..count).filter(|i| !hidden.get(i).copied().unwrap_or(false)).collect()
}

/// Applies the cell edits of one table. Objects are only replaced when a cell really changed.
pub(crate) fn apply(pkg: &Package, original: &Table, edited: &[Vec<Vec<DocxPara>>], edits: &mut Edits, report: &mut Report) {
    let Some(info_id) = original.source else { return };
    let Some(model) = pkg.object(info_id).and_then(|i| i.reference(2)).and_then(|id| pkg.object(id)) else { return };
    let store = model.msg(4).unwrap_or_default();

    let changes: Vec<(usize, usize, String)> = original
        .rows
        .iter()
        .zip(edited)
        .enumerate()
        .flat_map(|(r, (row0, row1))| {
            row0.cells.iter().zip(row1).enumerate().filter_map(move |(c, (cell0, cell1))| {
                let new = docx_cell_text(cell1);
                (model_cell_text(&cell0.blocks) != new).then_some((r, c, new))
            })
        })
        .collect();
    let shape_changed =
        original.rows.len() != edited.len() || original.rows.iter().zip(edited).any(|(r0, r1)| r0.cells.len() != r1.len());
    if shape_changed {
        report.skip(messages::NOT_WRITTEN_TABLE_SHAPE);
    }
    if changes.is_empty() {
        return;
    }

    let to_usize = |v: Option<u64>| usize::try_from(v.unwrap_or(0)).unwrap_or(0);
    let row_buckets = store.msg(1).map(|m| m.references(2)).unwrap_or_default();
    let col_bucket = store.reference(2);
    let rows = visible(pkg, &row_buckets, to_usize(model.uint(6)));
    let cols = visible(pkg, &col_bucket.into_iter().collect::<Vec<_>>(), to_usize(model.uint(7)));

    let Some(strings_id) = store
        .field_numbers()
        .flat_map(|f| single_refs(&store, f))
        .find(|id| pkg.kind(*id) == Some(kinds::DATA_LIST) && pkg.object(*id).and_then(|l| l.uint(1)) == Some(LIST_STRINGS))
    else {
        report.skip(messages::NOT_WRITTEN_CELL);
        return;
    };
    let strings_msg = pkg.object(strings_id).unwrap_or_default();
    // key → (reference count, text)
    let mut strings: BTreeMap<u64, (u64, String)> = strings_msg
        .msgs(3)
        .iter()
        .filter_map(|e| Some((e.uint(1)?, (e.uint(2).unwrap_or(1), e.string(3).unwrap_or_default()))))
        .collect();
    let mut next_key = strings_msg.uint(2).unwrap_or(1).max(strings.keys().max().map_or(1, |k| k + 1));

    let tiles_msg = store.msg(3).unwrap_or_default();
    let tile_rows = tiles_msg.uint(2).filter(|s| *s > 0).unwrap_or(256);
    let mut tiles: Vec<Tile> = tiles_msg
        .msgs(1)
        .iter()
        .filter_map(|entry| Tile::load(pkg, entry.reference(2)?, entry.uint(1).unwrap_or(0) * tile_rows))
        .collect();
    let template = tiles
        .iter()
        .flat_map(|t| t.rows.iter().flat_map(|r| r.cells.iter().flatten()))
        .find(|c| c.get(1) == Some(&CELL_TEXT) && string_id(c).is_some())
        .cloned();

    let mut touched: BTreeSet<usize> = BTreeSet::new();
    for (r, c, text) in changes {
        let (Some(&row), Some(&col)) = (rows.get(r), cols.get(c)) else { continue };
        let row = row as u64;
        let Some(t) = tiles.iter().position(|tile| tile.base <= row && row < tile.base + tile_rows) else {
            report.skip(messages::NOT_WRITTEN_CELL_AREA);
            continue;
        };
        let stored = tiles[t].row_mut(row);
        if stored.cells.len() <= col {
            stored.cells.resize(col + 1, None);
        }
        if stored.legacy.len() <= col {
            stored.legacy.resize(col + 1, None);
        }
        match stored.cells[col].clone() {
            Some(cell) if cell.get(1) != Some(&CELL_TEXT) => {
                report.skip(messages::NOT_WRITTEN_CELL);
                continue;
            }
            Some(cell) => {
                let old_key = string_id(&cell);
                if let Some(old) = old_key.and_then(|k| strings.get_mut(&k)) {
                    old.0 = old.0.saturating_sub(1);
                }
                if text.is_empty() {
                    stored.cells[col] = None;
                    stored.legacy[col] = None;
                } else if let Some(key) = old_key.filter(|k| strings.get(k).is_some_and(|(n, _)| *n == 0)) {
                    // Only this cell used the string: change it in place.
                    strings.insert(key, (1, text));
                } else {
                    strings.insert(next_key, (1, text));
                    stored.cells[col] = Some(with_string_id(&cell, next_key));
                    next_key += 1;
                }
            }
            None if text.is_empty() => continue,
            None => {
                strings.insert(next_key, (1, text));
                let cell = template.as_ref().map_or_else(|| minimal_text_cell(next_key), |t| with_string_id(t, next_key));
                stored.cells[col] = Some(cell);
                stored.legacy[col] = Some(LEGACY_RECORD.to_vec());
                next_key += 1;
            }
        }
        touched.insert(t);
        report.cells_changed += 1;
    }
    if touched.is_empty() {
        return;
    }
    strings.retain(|_, (count, _)| *count > 0);

    for &t in &touched {
        edits.replace_object(tiles[t].id, tiles[t].encode(), pkg.references(tiles[t].id).to_vec());
    }
    let mut list = Encoder::new();
    list.copy_except(&strings_msg, &[2, 3]);
    list.uint(2, next_key);
    for (key, (count, text)) in &strings {
        let mut entry = Encoder::new();
        entry.uint(1, *key).uint(2, *count).string(3, text);
        list.bytes(3, &entry.finish());
    }
    edits.replace_object(strings_id, list.finish(), pkg.references(strings_id).to_vec());
    update_cell_counts(pkg, &tiles, &row_buckets, col_bucket, edits);
}

/// Keeps the per-row and per-column cell counts of the header buckets in step with the cells.
fn update_cell_counts(pkg: &Package, tiles: &[Tile], row_buckets: &[u64], col_bucket: Option<u64>, edits: &mut Edits) {
    let mut per_row: HashMap<usize, u64> = HashMap::new();
    let mut per_col: HashMap<usize, u64> = HashMap::new();
    for tile in tiles {
        for row in &tile.rows {
            let Ok(r) = usize::try_from(tile.base + row.row_in_tile) else { continue };
            for (c, cell) in row.cells.iter().enumerate() {
                if cell.is_some() {
                    *per_row.entry(r).or_default() += 1;
                    *per_col.entry(c).or_default() += 1;
                }
            }
        }
    }
    for (bucket_id, counts) in row_buckets.iter().map(|b| (*b, &per_row)).chain(col_bucket.map(|b| (b, &per_col))) {
        let Some(bucket) = pkg.object(bucket_id) else { continue };
        let headers = bucket.msgs(2);
        let count_of = |h: &Msg| {
            let i = h.uint(1).and_then(|i| usize::try_from(i).ok()).unwrap_or(usize::MAX);
            counts.get(&i).copied().unwrap_or(0)
        };
        if headers.iter().all(|h| h.uint(4).unwrap_or(0) == count_of(h)) {
            continue;
        }
        let mut e = Encoder::new();
        e.copy_except(&bucket, &[2]);
        for h in &headers {
            let mut header = Encoder::new();
            header.copy_except(h, &[4]).uint(4, count_of(h));
            e.bytes(2, &header.finish());
        }
        edits.replace_object(bucket_id, e.finish(), pkg.references(bucket_id).to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_joins_cell_buffers() {
        let a = [5, 3, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0];
        let b = [5, 3, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 2, 0, 0, 0];
        let storage = [a, b].concat();
        let offsets = [0u16, 0xFFFF, 16].iter().flat_map(|o| o.to_le_bytes()).collect::<Vec<_>>();
        let cells = split_cells(&storage, &offsets, 1);
        assert_eq!(cells, vec![Some(a.to_vec()), None, Some(b.to_vec())]);
        assert_eq!(string_id(cells[2].as_ref().unwrap()), Some(2));
        assert_eq!(join_cells(&cells, 3, false), (storage, offsets));
        assert_eq!(string_id(&with_string_id(&a, 7)), Some(7));
    }
}
