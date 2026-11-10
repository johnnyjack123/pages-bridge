//! Everything the user reads, in one place so the tool can be translated.

use std::path::Path;

use crate::Error;

pub const ABOUT: &str = "Wandelt Apple-Pages-Dokumente (.pages) in Word-Dateien (.docx) um und öffnet sie \
                         auf Wunsch im eingestellten Programm.";
pub const HELP_FILES: &str = ".pages-Dateien, die umgewandelt werden sollen";
pub const HELP_OUTPUT: &str = "Zieldatei (nur bei genau einer Eingabedatei)";
pub const HELP_OVERWRITE: &str = "Eine vorhandene .docx gleichen Namens ersetzen";
pub const HELP_OPEN: &str = "Umwandeln und im eingestellten Programm öffnen (das macht der Doppelklick)";
pub const HELP_OPEN_FILE: &str = "Die .pages-Datei";
pub const HELP_INSTALL: &str = "Doppelklick auf .pages-Dateien einrichten (nur für den aktuellen Benutzer)";
pub const HELP_UNINSTALL: &str = "Die Doppelklick-Zuordnung wieder entfernen";
pub const HELP_CONFIG: &str = "Die Einstellungsdatei anzeigen (wird bei Bedarf angelegt)";

pub const WINDOW_TITLE: &str = "Pages öffnen";
pub const FILE_TYPE_NAME: &str = "Apple Pages-Dokument";

pub const NOTE_NO_BODY: &str = "Hinweis: Layout-Dokument ohne Fließtext, nur die Textfelder wurden übernommen.";
pub const SKIP_OBJECT: &str = "Objekt";
pub const SKIP_NESTED_TABLE: &str = "Tabelle in Tabelle oder Kopfzeile";
pub const SKIP_HEADER_IMAGE: &str = "Bild in Kopf-/Fußzeile";
pub const SKIP_IMAGE: &str = "Bild";
pub const SKIP_IMAGE_FORMAT: &str = "Bild in unbekanntem Format";

pub const CELL_TRUE: &str = "WAHR";
pub const CELL_FALSE: &str = "FALSCH";
pub const CELL_ERROR: &str = "#FEHLER";

pub fn saved(path: &Path) -> String {
    format!("Gespeichert: {}", path.display())
}

pub fn reused(path: &Path) -> String {
    format!("Schon umgewandelt, öffne die vorhandene Datei: {}", path.display())
}

pub fn skipped(count: usize, kinds: &str) -> String {
    format!("Hinweis: {count} Objekt(e) wurden nicht übernommen ({kinds}).")
}

pub fn output_needs_single_input() -> String {
    "--output geht nur mit genau einer Eingabedatei.".to_owned()
}

pub fn describe(error: &Error, path: &Path) -> String {
    let path = path.display();
    match error {
        Error::Package(iwork::Error::LegacyFormat) => {
            format!("Diese Datei stammt aus Pages 2009 oder älter (iWork ’09). Dieses alte Format wird nicht unterstützt:\n{path}")
        }
        Error::Package(iwork::Error::NotIwork) | Error::NoDocument => format!("Das ist keine lesbare Pages-Datei:\n{path}"),
        Error::Package(iwork::Error::Io(e)) | Error::Io(e) => {
            format!("Die Datei konnte nicht gelesen oder geschrieben werden:\n{path}\n\n{e}")
        }
        Error::Package(iwork::Error::Corrupt(why)) => {
            format!("Die Pages-Datei ist beschädigt oder hat ein unbekanntes Format:\n{path}\n\n{why}")
        }
    }
}

pub fn config_invalid(path: &Path, why: &str) -> String {
    format!("Die Einstellungsdatei ist fehlerhaft und wurde nicht verändert:\n{}\n\n{why}", path.display())
}

pub fn config_location(path: &Path) -> String {
    format!("Einstellungen: {}", path.display())
}

pub fn program_not_found(name: &str, config: &Path) -> String {
    format!(
        "Das Programm „{name}“ wurde nicht gefunden.\nBitte in der Einstellungsdatei „open_with“ oder „programs“ anpassen:\n{}",
        config.display()
    )
}

pub fn launch_failed(program: &Path, why: &str) -> String {
    format!("„{}“ konnte nicht gestartet werden:\n{why}", program.display())
}

pub fn installed(dir: &Path) -> String {
    format!(
        "Fertig: Ein Doppelklick auf eine .pages-Datei öffnet sie jetzt umgewandelt.\nProgramm installiert in: {}",
        dir.display()
    )
}

pub const UNINSTALLED: &str = "Die Doppelklick-Zuordnung für .pages wurde entfernt.";

pub fn install_failed(why: &str) -> String {
    format!("Die Dateizuordnung konnte nicht eingerichtet werden:\n{why}")
}

#[cfg_attr(windows, allow(dead_code))]
pub const NOT_ON_THIS_SYSTEM: &str = "Das geht nur unter Windows.";
