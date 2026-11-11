//! Writing edits made to a converted `.docx` back into the Pages document.

mod common;

use std::path::Path;

use common::{BODY, BODY_STORAGE, build_document, edit_docx, read_part, run_props, scratch_dir, storage_text};
use pages2docx::writeback::{Options, write_back};

#[test]
fn an_unchanged_document_writes_nothing() {
    let dir = scratch_dir("wb-unchanged");
    let (pages, docx, out) = (dir.join("a.pages"), dir.join("a.docx"), dir.join("out.pages"));
    build_document(&pages);
    pages2docx::convert(&pages, &docx).unwrap();
    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert!(report.is_unchanged() && report.not_written.is_empty(), "{report:?}");
    assert!(!out.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn edited_text_and_new_paragraphs_keep_styles_and_objects() {
    let dir = scratch_dir("wb-edit");
    let (pages, docx, out) = (dir.join("a.pages"), dir.join("a.docx"), dir.join("out.pages"));
    build_document(&pages);
    pages2docx::convert(&pages, &docx).unwrap();
    edit_docx(&docx, |xml| {
        xml.replace("Nach Seitenumbruch", "Nach dem Umbruch")
            .replace("<w:sectPr>", "<w:p><w:r><w:t>Neuer Absatz</w:t></w:r></w:p><w:sectPr>")
    });

    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert_eq!((report.paragraphs_changed, report.cells_changed), (2, 0), "{report:?}");
    assert!(report.not_written.is_empty(), "{report:?}");
    let expected = format!("{}\nNeuer Absatz", BODY.replace("Nach Seitenumbruch", "Nach dem Umbruch"));
    assert_eq!(storage_text(&out, BODY_STORAGE), expected);

    // The written document converts again, with its character styles, picture and footer.
    let again = dir.join("again.docx");
    pages2docx::convert(&out, &again).unwrap();
    let xml = read_part(&again, "word/document.xml").unwrap();
    assert!(run_props(&xml, "Fett").contains("<w:b/>"));
    assert!(run_props(&xml, "Ende").contains("<w:b/>"), "style after the edit still sits on the right characters");
    assert!(!run_props(&xml, "Nach dem Umbruch 😀 ").contains("<w:b/>"));
    assert!(xml.contains(">Neuer Absatz<") && xml.contains("r:embed="));
    assert!(read_part(&again, "word/footer1.xml").unwrap().contains(" PAGE "));
    let _ = std::fs::remove_dir_all(dir);
}

/// Adds run properties to the run showing `text`.
fn format_run(xml: &str, text: &str, props: &str) -> String {
    let at = xml.find(&format!(">{text}</w:t>")).unwrap();
    let start = xml[..at].rfind("<w:rPr>").unwrap() + "<w:rPr>".len();
    format!("{}{props}{}", &xml[..start], &xml[start..])
}

#[test]
fn formatting_uses_existing_styles_or_is_reported() {
    let dir = scratch_dir("wb-format");
    let (pages, docx, out) = (dir.join("a.pages"), dir.join("a.docx"), dir.join("out.pages"));
    build_document(&pages);
    pages2docx::convert(&pages, &docx).unwrap();
    // " Bild" is plain. The document has a bold style, so bold can be carried over.
    edit_docx(&docx, |xml| format_run(xml, " Bild", "<w:b/>"));
    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert!(report.not_written.is_empty(), "{report:?}");
    let again = dir.join("again.docx");
    pages2docx::convert(&out, &again).unwrap();
    let xml = read_part(&again, "word/document.xml").unwrap();
    assert!(run_props(&xml, " Bild").contains("<w:b/>"));
    assert!(!run_props(&xml, " und ").contains("<w:b/>"));

    // Bold and italic together: no style of the document gives both.
    pages2docx::convert(&pages, &docx).unwrap();
    edit_docx(&docx, |xml| format_run(xml, " Bild", "<w:b/><w:i/>"));
    std::fs::remove_file(&out).unwrap();
    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert_eq!(report.not_written.values().sum::<usize>(), 1, "{report:?}");
    let _ = std::fs::remove_dir_all(dir);
}

fn reference(name: &str) -> Option<std::path::PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference").join(name);
    path.is_file().then_some(path)
}

#[test]
fn reference_document_round_trip() {
    let Some(original) = reference("Cuba hoy.pages") else {
        eprintln!("reference/Cuba hoy.pages missing, skipped");
        return;
    };
    let dir = scratch_dir("wb-reference");
    let (pages, docx, out) = (dir.join("Cuba.pages"), dir.join("Cuba.docx"), dir.join("out.pages"));
    std::fs::copy(original, &pages).unwrap();
    pages2docx::convert(&pages, &docx).unwrap();
    edit_docx(&docx, |xml| xml.replace("acrual", "actual"));

    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert_eq!(report.paragraphs_changed, 1, "{report:?}");
    let again = dir.join("again.docx");
    pages2docx::convert(&out, &again).unwrap();
    let xml = read_part(&again, "word/document.xml").unwrap();
    assert!(xml.contains("situación económica actual de Cuba"));
    assert_eq!(xml.matches("<w:hyperlink ").count(), 7, "links survive");
    assert_eq!(xml.matches("<w:tbl>").count(), 1, "the table survives");
    assert_eq!(xml.matches("<w:numPr>").count(), 24, "lists survive");
    let _ = std::fs::remove_dir_all(dir);
}
