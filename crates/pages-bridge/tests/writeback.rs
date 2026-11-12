//! Writing edits made to a converted `.docx` back into the Pages document.

mod common;

use std::path::Path;

use common::{BODY, BODY_STORAGE, build_document, edit_docx, read_part, run_props, scratch_dir, storage_text};
use pages_bridge::writeback::{Options, write_back};

#[test]
fn an_unchanged_document_writes_nothing() {
    let dir = scratch_dir("wb-unchanged");
    let (pages, docx, out) = (dir.join("a.pages"), dir.join("a.docx"), dir.join("out.pages"));
    build_document(&pages);
    pages_bridge::convert(&pages, &docx).unwrap();
    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert!(!report.written && report.is_unchanged() && report.not_written.is_empty(), "{report:?}");
    assert!(!out.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn edited_text_and_new_paragraphs_keep_styles_and_objects() {
    let dir = scratch_dir("wb-edit");
    let (pages, docx, out) = (dir.join("a.pages"), dir.join("a.docx"), dir.join("out.pages"));
    build_document(&pages);
    pages_bridge::convert(&pages, &docx).unwrap();
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
    pages_bridge::convert(&out, &again).unwrap();
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
    let run = xml[..at].rfind("<w:r>").unwrap() + "<w:r>".len();
    match xml[run..at].find("<w:rPr>") {
        Some(rpr) => {
            let start = run + rpr + "<w:rPr>".len();
            format!("{}{props}{}", &xml[..start], &xml[start..])
        }
        None => format!("{}<w:rPr>{props}</w:rPr>{}", &xml[..run], &xml[run..]),
    }
}

#[test]
fn formatting_uses_existing_styles_or_is_reported() {
    let dir = scratch_dir("wb-format");
    let (pages, docx, out) = (dir.join("a.pages"), dir.join("a.docx"), dir.join("out.pages"));
    build_document(&pages);
    pages_bridge::convert(&pages, &docx).unwrap();
    // " Bild" is plain. The document has a bold style, so bold can be carried over.
    edit_docx(&docx, |xml| format_run(xml, " Bild", "<w:b/>"));
    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert!(report.not_written.is_empty(), "{report:?}");
    let again = dir.join("again.docx");
    pages_bridge::convert(&out, &again).unwrap();
    let xml = read_part(&again, "word/document.xml").unwrap();
    assert!(run_props(&xml, " Bild").contains("<w:b/>"));
    assert!(!run_props(&xml, " und ").contains("<w:b/>"));

    // Bold and italic together: no style of the document gives both.
    pages_bridge::convert(&pages, &docx).unwrap();
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

/// Writes `text` into the cell at (`row`, `col`) of the first table, 0-based.
fn fill_cell(xml: &str, row: usize, col: usize, text: &str) -> String {
    let mut at = xml.find("<w:tbl>").unwrap();
    for _ in 0..=row {
        at += xml[at..].find("<w:tr>").unwrap() + 1;
    }
    for _ in 0..=col {
        at += xml[at..].find("<w:tc>").unwrap() + 1;
    }
    let end = at + xml[at..].find("</w:p>").unwrap();
    format!("{}<w:r><w:t>{text}</w:t></w:r>{}", &xml[..end], &xml[end..])
}

/// The texts of a converted table's cells, row by row.
fn table_texts(xml: &str) -> Vec<Vec<String>> {
    let table = &xml[xml.find("<w:tbl>").unwrap()..xml.find("</w:tbl>").unwrap()];
    table
        .split("<w:tr>")
        .skip(1)
        .map(|row| {
            row.split("<w:tc>")
                .skip(1)
                .map(|cell| cell.split("<w:t xml:space=\"preserve\">").skip(1).map(|t| &t[..t.find('<').unwrap()]).collect())
                .collect()
        })
        .collect()
}

#[test]
fn text_in_a_row_pages_did_not_store() {
    let Some(original) = reference("Showcase.pages") else {
        eprintln!("reference/Showcase.pages missing, skipped");
        return;
    };
    let dir = scratch_dir("wb-empty-row");
    let (pages, docx, out) = (dir.join("Showcase.pages"), dir.join("Showcase.docx"), dir.join("out.pages"));
    std::fs::copy(original, &pages).unwrap();
    pages_bridge::convert(&pages, &docx).unwrap();
    let before = table_texts(&read_part(&docx, "word/document.xml").unwrap());
    // The last row is completely empty, so Pages keeps no cells for it.
    assert!(before[4].iter().all(String::is_empty));
    edit_docx(&docx, |xml| fill_cell(xml, 4, 2, "test2"));

    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert_eq!(report.cells_changed, 1, "{report:?}");
    assert!(report.not_written.is_empty(), "{report:?}");
    let again = dir.join("again.docx");
    pages_bridge::convert(&out, &again).unwrap();
    let mut expected = before;
    expected[4][2] = "test2".to_owned();
    assert_eq!(table_texts(&read_part(&again, "word/document.xml").unwrap()), expected);
    let _ = std::fs::remove_dir_all(dir);
}

/// Gives the paragraph containing `text` the paragraph style `style`.
fn restyle(xml: &str, text: &str, style: &str) -> String {
    let at = xml.find(text).unwrap();
    let start = xml[..at].rfind("<w:pStyle w:val=\"").unwrap() + "<w:pStyle w:val=\"".len();
    let end = start + xml[start..].find('"').unwrap();
    format!("{}{style}{}", &xml[..start], &xml[end..])
}

#[test]
fn a_style_chosen_in_the_word_processor_is_written_back() {
    let Some(original) = reference("Cuba hoy.pages") else {
        eprintln!("reference/Cuba hoy.pages missing, skipped");
        return;
    };
    let dir = scratch_dir("wb-restyle");
    let (pages, docx, out) = (dir.join("Cuba.pages"), dir.join("Cuba.docx"), dir.join("out.pages"));
    std::fs::copy(original, &pages).unwrap();
    pages_bridge::convert(&pages, &docx).unwrap();
    // Pages' "Überschrift" is Word's "Heading 1"; "Quote" does not exist in the Pages document.
    edit_docx(&docx, |xml| restyle(&restyle(xml, "geografía, población", "Heading1"), "colonialismo", "Quote"));

    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert_eq!(report.paragraphs_changed, 1, "{report:?}");
    assert_eq!(report.not_written.len(), 1, "the unknown style is reported: {report:?}");
    let again = dir.join("again.docx");
    pages_bridge::convert(&out, &again).unwrap();
    let xml = read_part(&again, "word/document.xml").unwrap();
    let heading = &xml[..xml.find("geografía, población").unwrap()];
    assert!(heading[heading.rfind("<w:p>").unwrap()..].contains("<w:pStyle w:val=\"Heading1\"/>"));
    let _ = std::fs::remove_dir_all(dir);
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
    pages_bridge::convert(&pages, &docx).unwrap();
    edit_docx(&docx, |xml| xml.replace("acrual", "actual"));

    let report = write_back(&pages, &docx, &out, Options::default()).unwrap();
    assert_eq!(report.paragraphs_changed, 1, "{report:?}");
    let again = dir.join("again.docx");
    pages_bridge::convert(&out, &again).unwrap();
    let xml = read_part(&again, "word/document.xml").unwrap();
    assert!(xml.contains("situación económica actual de Cuba"));
    assert_eq!(xml.matches("<w:hyperlink ").count(), 7, "links survive");
    assert_eq!(xml.matches("<w:tbl>").count(), 1, "the table survives");
    assert_eq!(xml.matches("<w:numPr>").count(), 24, "lists survive");
    let _ = std::fs::remove_dir_all(dir);
}
