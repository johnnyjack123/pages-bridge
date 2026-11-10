//! Objects anchored in text through an attachment character (U+FFFC): tables, pictures, text
//! boxes and groups, and the page number fields of headers and footers.

use std::path::PathBuf;
use std::process::Command;

use iwork::{Msg, Raw};

use super::{Ctx, Part, Reader, kinds};
use crate::messages;
use crate::model::{Block, Image, InlineKind};

pub(super) enum Attached {
    Nothing,
    Inline(Vec<InlineKind>),
    Blocks(Vec<Block>),
}

/// Fields of a picture object that can hold its `TSP.DataReference`: the picture, the original
/// before cropping, a legacy field; thumbnails only as a last resort.
const IMAGE_DATA_FIELDS: [u32; 5] = [11, 13, 2, 15, 12];

impl Reader<'_> {
    pub(super) fn attachment(&mut self, id: Option<u64>, ctx: Ctx) -> Attached {
        let Some(id) = id else { return Attached::Nothing };
        match self.pkg.kind(id) {
            Some(kinds::DRAWABLE_ATTACHMENT) => {
                let drawable = self.pkg.object(id).and_then(|m| m.reference(1));
                self.drawable(drawable, ctx, 0)
            }
            // The other attachments in headers and footers are page number fields.
            _ if ctx.part == Part::Header => Attached::Inline(vec![InlineKind::PageNumber]),
            _ => {
                self.report.skip(messages::SKIP_OBJECT);
                Attached::Nothing
            }
        }
    }

    fn drawable(&mut self, id: Option<u64>, ctx: Ctx, depth: u32) -> Attached {
        let pkg = self.pkg;
        let Some(id) = id else { return Attached::Nothing };
        let Some(m) = pkg.object(id) else { return Attached::Nothing };
        if pkg.kind(id) == Some(kinds::TABLE_INFO) {
            if ctx.part == Part::Body {
                return Attached::Blocks(vec![Block::Table(self.read_table(id, ctx))]);
            }
            self.report.skip(messages::SKIP_NESTED_TABLE);
            return Attached::Nothing;
        }
        if let Some(image) = self.image(&m, ctx) {
            return image;
        }
        let storages: Vec<u64> = m
            .field_numbers()
            .flat_map(|f| single_refs(&m, f))
            .filter(|r| pkg.kind(*r) == Some(kinds::STORAGE))
            .collect();
        if !storages.is_empty() {
            let blocks = storages.into_iter().flat_map(|s| self.read_storage(s, ctx)).collect();
            return Attached::Blocks(blocks);
        }
        if depth < 5 {
            // Groups list their members in field 2.
            let (mut inline, mut blocks) = (Vec::new(), Vec::new());
            for child in single_refs(&m, 2) {
                match self.drawable(Some(child), ctx, depth + 1) {
                    Attached::Inline(k) => inline.extend(k),
                    Attached::Blocks(b) => blocks.extend(b),
                    Attached::Nothing => {}
                }
            }
            if !blocks.is_empty() {
                return Attached::Blocks(blocks);
            }
            if !inline.is_empty() {
                return Attached::Inline(inline);
            }
        }
        self.report.skip(messages::SKIP_OBJECT);
        Attached::Nothing
    }

    /// A picture: `Some` when `m` references image data, even if it cannot be used.
    fn image(&mut self, m: &Msg, ctx: Ctx) -> Option<Attached> {
        let pkg = self.pkg;
        let data_id = IMAGE_DATA_FIELDS
            .iter()
            .find_map(|&f| m.msg(f).and_then(|r| r.uint(1)).filter(|id| pkg.has_data(*id)))?;
        if ctx.part == Part::Header {
            self.report.skip(messages::SKIP_HEADER_IMAGE);
            return Some(Attached::Nothing);
        }
        let Ok(mut data) = pkg.read_data(data_id) else {
            self.report.skip(messages::SKIP_IMAGE);
            return Some(Attached::Nothing);
        };
        let mut info = image_info(&data);
        if info.is_none() {
            if let Some(png) = convert_with_imagemagick(&data) {
                info = image_info(&png);
                data = png;
            }
        }
        let Some((ext, pixels)) = info else {
            self.report.skip(messages::SKIP_IMAGE_FORMAT);
            return Some(Attached::Nothing);
        };
        let (mut width, mut height) = geometry_size(m).unwrap_or_else(|| {
            let (w, h) = pixels.unwrap_or((400, 300));
            // Pixels at 96 dpi.
            (f64::from(w) * 0.75, f64::from(h) * 0.75)
        });
        if width > ctx.max_width {
            height *= ctx.max_width / width;
            width = ctx.max_width;
        }
        let index = self.images.len();
        self.images.push(Image { data, ext, pixel_size: pixels });
        Some(Attached::Inline(vec![InlineKind::Image { index, width, height }]))
    }
}

/// References in field `f` that are bare `TSP.Reference`s, not larger messages that happen to
/// start with field 1.
pub(super) fn single_refs(m: &Msg, f: u32) -> Vec<u64> {
    m.msgs(f)
        .iter()
        .filter(|sub| sub.field_numbers().count() == 1)
        .filter_map(|sub| match sub.raw(1) {
            [Raw::Varint(id)] => Some(*id),
            _ => None,
        })
        .collect()
}

/// Width and height of a drawable: `TSD.GeometryArchive` (position, size) sits at the end of
/// the chain of `super` fields 1.
fn geometry_size(m: &Msg) -> Option<(f64, f64)> {
    let mut current = m.clone();
    for _ in 0..4 {
        current = current.msg(1)?;
        let is_point = |p: &Msg| matches!(p.raw(1), [Raw::Fixed32(_)]) && matches!(p.raw(2), [Raw::Fixed32(_)]);
        if let (Some(position), Some(size)) = (current.msg(1), current.msg(2)) {
            if is_point(&position) && is_point(&size) {
                let (w, h) = (size.float(1)?, size.float(2)?);
                return (w > 0.0 && h > 0.0).then_some((w, h));
            }
        }
    }
    None
}

/// File extension and pixel size of a picture format Word can embed.
pub(crate) fn image_info(data: &[u8]) -> Option<(&'static str, Option<(u32, u32)>)> {
    let be32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le16 = |o: usize| data.get(o..o + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(("png", be32(16).zip(be32(20))));
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some(("gif", le16(6).zip(le16(8))));
    }
    if data.starts_with(b"BM") {
        let le32 = |o: usize| data.get(o..o + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]).unsigned_abs());
        return Some(("bmp", le32(18).zip(le32(22))));
    }
    if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        return Some(("tiff", None));
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        return Some(("jpeg", jpeg_size(data)));
    }
    None
}

fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    let mut pos = 2;
    while pos + 9 < data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = data[pos + 1];
        let is_frame = matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
        if is_frame {
            let h = u32::from(u16::from_be_bytes([data[pos + 5], data[pos + 6]]));
            let w = u32::from(u16::from_be_bytes([data[pos + 7], data[pos + 8]]));
            return Some((w, h));
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            pos += 2;
            continue;
        }
        pos += 2 + usize::from(u16::from_be_bytes([data[pos + 2], data[pos + 3]]));
    }
    None
}

/// HEIC, PDF and other pictures Word cannot embed become PNG when ImageMagick is installed.
fn convert_with_imagemagick(data: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("pages2docx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let (src, dst): (PathBuf, PathBuf) = (dir.join("in"), dir.join("out.png"));
    let result = (|| {
        std::fs::write(&src, data).ok()?;
        let mut cmd = Command::new("magick");
        cmd.arg(format!("{}[0]", src.display())).arg(&dst);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.status().ok().filter(|s| s.success())?;
        std::fs::read(&dst).ok()
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_png_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(image_info(&png), Some(("png", Some((2, 1)))));
        assert_eq!(image_info(b"%PDF-1.4"), None);
    }
}
