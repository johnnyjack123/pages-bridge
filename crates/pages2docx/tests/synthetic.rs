//! Converts a small Pages document built in memory, covering what the reference document lacks:
//! character styles, a picture, a page break, header and footer fields and text beyond the
//! Basic Multilingual Plane.

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

fn iwa(objects: &[(u64, u32, Vec<u8>)]) -> Vec<u8> {
    let mut stream = Vec::new();
    for (id, kind, payload) in objects {
        let info = [uint(1, *id), bytes(2, &[uint(1, u64::from(*kind)), uint(3, payload.len() as u64)].concat())].concat();
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

fn png_2x1() -> Vec<u8> {
    // A valid 2×1 PNG; only its header matters to the converter.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x7B, 0x40, 0xE8, 0xDD]);
    png
}

fn utf16_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

const BODY: &str = "Fett und kursiv \u{FFFC} Bild\u{0C}Nach Seitenumbruch 😀 Ende";

fn build_document(path: &Path) {
    let (style, bold, italic, picture_anchor, picture, body) = (51, 52, 53, 60, 61, 70);
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
        ),
        (bold, 2021, [bytes(1, &[]), bytes(11, &uint(1, 1))].concat()),
        (italic, 2021, [bytes(1, &[]), bytes(11, &[uint(2, 1), uint(11, 1)].concat())].concat()),
        (picture_anchor, 2003, reference(1, picture)),
        (
            picture,
            3005,
            [
                bytes(1, &bytes(1, &[bytes(1, &[float(1, 0.0), float(2, 0.0)].concat()), bytes(2, &[float(1, 100.0), float(2, 50.0)].concat())].concat())),
                reference(11, 999),
            ]
            .concat(),
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
        ),
        (80, 10011, reference(23, 81)),
        (81, 10143, [82, 83, 84].iter().map(|s| reference(1, *s)).chain([85, 86, 87].iter().map(|s| reference(2, *s))).collect::<Vec<_>>().concat()),
        (82, 2001, uint(1, 1)),
        (83, 2001, [uint(1, 1), string(3, "Kopfzeile"), bytes(5, &attr_table(&[(0, Some(style))]))].concat()),
        (84, 2001, uint(1, 1)),
        (85, 2001, uint(1, 1)),
        (86, 2001, [uint(1, 1), string(3, "Seite \u{FFFC}"), bytes(5, &attr_table(&[(0, Some(style))])), bytes(9, &attr_table(&[(6, Some(88))]))].concat()),
        (87, 2001, uint(1, 1)),
        (88, 2043, bytes(1, &[])),
        (1, 10000, [reference(4, body), float(30, 595.28), float(31, 841.89)].concat()),
    ];
    let metadata = vec![(2, 11006, bytes(4, &[uint(1, 999), string(3, "pic.png")].concat()))];

    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in [("Index/Document.iwa", iwa(&objects)), ("Index/Metadata.iwa", iwa(&metadata)), ("Data/pic.png", png_2x1())] {
        zip.start_file(name, options).unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.finish().unwrap();
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pages2docx-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read_part(docx: &Path, name: &str) -> Option<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(docx).unwrap()).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    Some(text)
}

/// The run properties of the run that shows `text`.
fn run_props<'a>(xml: &'a str, text: &str) -> &'a str {
    let at = xml.find(&format!(">{text}</w:t>")).unwrap_or_else(|| panic!("{text} not found"));
    let start = xml[..at].rfind("<w:r>").unwrap();
    &xml[start..at]
}

#[test]
fn converts_formatting_pictures_breaks_and_fields() {
    let dir = scratch_dir("synthetic");
    let (pages, docx) = (dir.join("test.pages"), dir.join("test.docx"));
    build_document(&pages);

    let report = pages2docx::convert(&pages, &docx).unwrap();
    assert!(report.skipped.is_empty(), "skipped: {:?}", report.skipped);

    let doc = read_part(&docx, "word/document.xml").unwrap();
    assert!(run_props(&doc, "Fett").contains("<w:b/>"));
    let kursiv = run_props(&doc, "kursiv");
    assert!(kursiv.contains("<w:i/>") && kursiv.contains("<w:u w:val=\"single\"/>") && !kursiv.contains("<w:b/>"));
    // The character style starts after the emoji, counted in UTF-16 code units as Pages does.
    assert!(run_props(&doc, "Ende").contains("<w:b/>"));
    assert!(!run_props(&doc, "Nach Seitenumbruch 😀 ").contains("<w:b/>"));
    assert!(doc.contains("<w:br w:type=\"page\"/>"));
    assert!(doc.contains("<wp:extent cx=\"1270000\" cy=\"635000\"/>"), "picture keeps its 100 × 50 pt size");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&docx).unwrap()).unwrap();
    assert_eq!(archive.by_name("word/media/image1.png").unwrap().size(), png_2x1().len() as u64);

    let header = read_part(&docx, "word/header1.xml").unwrap();
    assert!(header.contains(">Kopfzeile<") && header.contains("<w:jc w:val=\"center\"/>"));
    let footer = read_part(&docx, "word/footer1.xml").unwrap();
    assert!(footer.contains("w:instr=\" PAGE \""));
    assert!(read_part(&docx, "[Content_Types].xml").unwrap().contains("/word/footer1.xml"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rejects_files_that_are_not_pages_documents() {
    let dir = scratch_dir("invalid");
    let path = dir.join("broken.pages");
    std::fs::write(&path, b"not a zip").unwrap();
    assert!(matches!(pages2docx::convert(&path, &dir.join("out.docx")), Err(pages2docx::Error::Package(iwork::Error::NotIwork))));
    let _ = std::fs::remove_dir_all(dir);
}
