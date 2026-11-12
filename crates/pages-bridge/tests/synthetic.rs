//! Converts a small Pages document built in memory, covering what the reference document lacks:
//! character styles, a picture, a page break, header and footer fields and text beyond the
//! Basic Multilingual Plane.

mod common;

use common::{build_document, png_2x1, read_part, run_props, scratch_dir};

#[test]
fn converts_formatting_pictures_breaks_and_fields() {
    let dir = scratch_dir("synthetic");
    let (pages, docx) = (dir.join("test.pages"), dir.join("test.docx"));
    build_document(&pages);

    let report = pages_bridge::convert(&pages, &docx).unwrap();
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
    assert!(matches!(pages_bridge::convert(&path, &dir.join("out.docx")), Err(pages_bridge::Error::Package(iwork::Error::NotIwork))));
    let _ = std::fs::remove_dir_all(dir);
}
