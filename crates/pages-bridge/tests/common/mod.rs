//! Shared test helpers: a small Pages document built in memory, and docx inspection.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = u8::try_from(v & 0x7F).unwrap();
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn uint(n: u64, v: u64) -> Vec<u8> {
    [varint(n << 3), varint(v)].concat()
}

fn bytes(n: u64, b: &[u8]) -> Vec<u8> {
    [varint((n << 3) | 2), varint(b.len() as u64), b.to_vec()].concat()
}

fn float(n: u64, f: f32) -> Vec<u8> {
    [varint((n << 3) | 5), f.to_le_bytes().to_vec()].concat()
}

fn string(n: u64, s: &str) -> Vec<u8> {
    bytes(n, s.as_bytes())
}

fn reference(n: u64, id: u64) -> Vec<u8> {
    bytes(n, &uint(1, id))
}

/// A `TSWP.ObjectAttributeTable`: (UTF-16 index, object) entries.
fn attr_table(entries: &[(u64, Option<u64>)]) -> Vec<u8> {
    entries
        .iter()
        .flat_map(|(index, object)| {
            let entry = [uint(1, *index), object.map(|o| reference(2, o)).unwrap_or_default()].concat();
            bytes(1, &entry)
        })
        .collect()
}

/// An archive stream of (id, type, payload, referenced ids), compressed as one IWA chunk.
fn iwa(objects: &[(u64, u32, Vec<u8>, Vec<u64>)]) -> Vec<u8> {
    let mut stream = Vec::new();
    for (id, kind, payload, refs) in objects {
        let mut message = [uint(1, u64::from(*kind)), uint(3, payload.len() as u64)].concat();
        if !refs.is_empty() {
            message.extend(bytes(5, &refs.iter().flat_map(|r| varint(*r)).collect::<Vec<u8>>()));
        }
        let info = [uint(1, *id), bytes(2, &message)].concat();
        stream.extend(varint(info.len() as u64));
        stream.extend(info);
        stream.extend(payload);
    }
    let compressed = snap::raw::Encoder::new().compress_vec(&stream).unwrap();
    let mut out = vec![0u8];
    out.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes()[..3]);
    out.extend(compressed);
    out
}

pub fn png_2x1() -> Vec<u8> {
    // A valid 2×1 PNG; only its header matters to the converter.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x7B, 0x40, 0xE8, 0xDD]);
    png
}

fn utf16_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

pub const BODY: &str = "Fett und kursiv \u{FFFC} Bild\u{0C}Nach Seitenumbruch 😀 Ende";
pub const BODY_STORAGE: u64 = 70;

/// Writes a Pages document with character styles, a picture, a page break, a header, a footer
/// with a page number, and an emoji before a styled word.
pub fn build_document(path: &Path) {
    let (style, bold, italic, picture_anchor, picture, body) = (51, 52, 53, 60, 61, BODY_STORAGE);
    let ende = utf16_len(BODY.split("Ende").next().unwrap());
    let objects = vec![
        (
            style,
            2022,
            [
                bytes(1, &[string(1, "Text"), string(2, "text-20-paragraphstyle-Body 1")].concat()),
                bytes(11, &[string(5, "HelveticaNeue"), float(3, 11.0)].concat()),
                bytes(12, &uint(1, 4)),
            ]
            .concat(),
            vec![],
        ),
        (bold, 2021, [bytes(1, &[]), bytes(11, &uint(1, 1))].concat(), vec![]),
        (italic, 2021, [bytes(1, &[]), bytes(11, &[uint(2, 1), uint(11, 1)].concat())].concat(), vec![]),
        (picture_anchor, 2003, reference(1, picture), vec![picture]),
        (
            picture,
            3005,
            [
                bytes(
                    1,
                    &bytes(
                        1,
                        &[bytes(1, &[float(1, 0.0), float(2, 0.0)].concat()), bytes(2, &[float(1, 100.0), float(2, 50.0)].concat())]
                            .concat(),
                    ),
                ),
                reference(11, 999),
            ]
            .concat(),
            vec![],
        ),
        (
            body,
            2001,
            [
                uint(1, 0),
                string(3, BODY),
                bytes(5, &attr_table(&[(0, Some(style))])),
                bytes(8, &attr_table(&[(0, Some(bold)), (4, None), (9, Some(italic)), (15, None), (ende, Some(bold))])),
                bytes(9, &attr_table(&[(16, Some(picture_anchor))])),
                bytes(17, &attr_table(&[(0, Some(80))])),
            ]
            .concat(),
            vec![style, bold, italic, picture_anchor, 80],
        ),
        (80, 10011, reference(23, 81), vec![81]),
        (
            81,
            10143,
            [82, 83, 84].iter().map(|s| reference(1, *s)).chain([85, 86, 87].iter().map(|s| reference(2, *s))).collect::<Vec<_>>().concat(),
            vec![82, 83, 84, 85, 86, 87],
        ),
        (82, 2001, uint(1, 1), vec![]),
        (83, 2001, [uint(1, 1), string(3, "Kopfzeile"), bytes(5, &attr_table(&[(0, Some(style))]))].concat(), vec![style]),
        (84, 2001, uint(1, 1), vec![]),
        (85, 2001, uint(1, 1), vec![]),
        (
            86,
            2001,
            [uint(1, 1), string(3, "Seite \u{FFFC}"), bytes(5, &attr_table(&[(0, Some(style))])), bytes(9, &attr_table(&[(6, Some(88))]))]
                .concat(),
            vec![style, 88],
        ),
        (87, 2001, uint(1, 1), vec![]),
        (88, 2043, bytes(1, &[]), vec![]),
        (1, 10000, [reference(4, body), float(30, 595.28), float(31, 841.89)].concat(), vec![body]),
    ];
    let metadata = vec![(2, 11006, bytes(4, &[uint(1, 999), string(3, "pic.png")].concat()), vec![])];

    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in [("Index/Document.iwa", iwa(&objects)), ("Index/Metadata.iwa", iwa(&metadata)), ("Data/pic.png", png_2x1())] {
        zip.start_file(name, options).unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.finish().unwrap();
}

pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pages-bridge-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn read_part(docx: &Path, name: &str) -> Option<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(docx).unwrap()).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    Some(text)
}

/// The run properties of the run that shows `text`.
pub fn run_props<'a>(xml: &'a str, text: &str) -> &'a str {
    let at = xml.find(&format!(">{text}</w:t>")).unwrap_or_else(|| panic!("{text} not found"));
    let start = xml[..at].rfind("<w:r>").unwrap();
    &xml[start..at]
}

/// Rewrites `word/document.xml` of a docx in place, as an editor saving the file would.
pub fn edit_docx(path: &Path, edit: impl Fn(&str) -> String) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut parts = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if entry.name() == "word/document.xml" {
            data = edit(&String::from_utf8(data).unwrap()).into_bytes();
        }
        parts.push((entry.name().to_owned(), data));
    }
    drop(archive);
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.finish().unwrap();
}

/// The text of the body storage of a Pages file.
pub fn storage_text(pages: &Path, storage: u64) -> String {
    let pkg = iwork::Package::open(pages).unwrap();
    pkg.object(storage).unwrap().strings(3).concat()
}
