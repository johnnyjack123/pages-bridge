//! Checks the conversion of the reference documents in `reference/` at the workspace root.
//! These are real Pages files that are not committed, so the tests skip when they are absent.

use std::io::Read;
use std::path::{Path, PathBuf};

fn reference(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference").join(name);
    path.is_file().then_some(path)
}

fn document_xml(pages: &Path) -> String {
    let dir = std::env::temp_dir().join(format!("pages2docx-ref-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let docx = dir.join("out.docx");
    pages2docx::convert(pages, &docx).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&docx).unwrap()).unwrap();
    let mut xml = String::new();
    archive.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    xml
}

#[test]
fn cuba_hoy() {
    let Some(pages) = reference("Cuba hoy.pages") else {
        eprintln!("reference/Cuba hoy.pages missing, skipped");
        return;
    };
    let xml = document_xml(&pages);
    assert_eq!(xml.matches("<w:hyperlink ").count(), 7);
    assert_eq!(xml.matches("<w:tbl>").count(), 1);
    // Dashes: 5 on page 1 and 3 on page 5; numbers: 5, 5 and 6 in the three groups.
    assert_eq!(xml.matches("<w:numPr>").count(), 24, "bullet and numbered paragraphs");
    for text in ["¿Qué queda de la Revolución cubana", "Grupo III: Sociedad", "Política", "¿Qué futuro imaginas para Cuba?"] {
        assert!(xml.contains(text), "{text}");
    }
    // Body lines at 1.5 × 14 pt with the 7 pt of extra leading moved into the top margin.
    assert!(xml.contains("w:line=\"420\" w:lineRule=\"atLeast\""));
    assert!(xml.contains("<w:pgMar w:top=\"994\""));
    // Every second body row of the table is banded.
    assert!(xml.contains("w:fill=\"F5F5F5\""));
}
