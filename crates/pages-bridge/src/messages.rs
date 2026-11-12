//! Everything the user reads, in one place so the tool can be translated.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;

use crate::Error;

pub const ABOUT: &str = "Öffnet Apple-Pages-Dokumente (.pages) in Word oder WordCraft und schreibt Änderungen \
                         in die .pages-Datei zurück. Wandelt auch einfach zwischen .pages und .docx um.";
pub const HELP_AFTER: &str = "Beispiele:\n  \
    pages-bridge Bericht.pages                  Bericht.docx daneben anlegen\n  \
    pages-bridge to-pages Bericht.docx          Änderungen in „Bericht (bearbeitet).pages“ schreiben\n  \
    pages-bridge to-pages Bericht.docx -i       Änderungen direkt in Bericht.pages schreiben\n  \
    pages-bridge config --open-with word        .pages-Dateien künftig in Word öffnen";
pub const HELP_TO_DOCX: &str = ".pages in .docx umwandeln (auch ohne Befehl: pages-bridge Datei.pages)";
pub const HELP_FILES: &str = ".pages-Dateien, die umgewandelt werden sollen";
pub const HELP_OUTPUT: &str = "Zieldatei (nur bei genau einer Eingabedatei)";
pub const HELP_OVERWRITE: &str = "Eine vorhandene .docx gleichen Namens ersetzen";
pub const HELP_TO_PAGES: &str = "Änderungen aus einer .docx in die .pages-Datei schreiben, aus der sie umgewandelt wurde";
pub const HELP_TO_PAGES_DOCX: &str = "Die bearbeitete .docx-Datei";
pub const HELP_TO_PAGES_ORIGINAL: &str = "Die ursprüngliche .pages-Datei (Standard: gleicher Name wie die .docx)";
pub const HELP_OUTPUT_PAGES: &str = "Zieldatei (Standard: „Name (bearbeitet).pages“ neben dem Original)";
pub const HELP_IN_PLACE: &str = "Direkt in die ursprüngliche .pages-Datei schreiben (vorher wird eine Sicherung angelegt)";
pub const HELP_OPEN: &str = "Umwandeln, im eingestellten Programm öffnen und Änderungen zurückschreiben (das macht der Doppelklick)";
pub const HELP_OPEN_FILE: &str = "Die .pages-Datei";
pub const HELP_SETTINGS: &str = "Das Einstellungsfenster öffnen (oder das Installationsfenster, falls noch nicht installiert)";
pub const HELP_CONFIG: &str = "Einstellungen anzeigen oder ändern";
pub const HELP_OPEN_WITH: &str =
    "Womit .pages-Dateien geöffnet werden: wordcraft, word, default (Windows-Standard für .docx) oder Pfad zu einer .exe";
pub const HELP_WRITE_BACK: &str = "Jedes Speichern in die .pages-Datei zurückschreiben: on oder off";
pub const HELP_DOCX_LOCATION: &str = "Wo die .docx beim Bearbeiten liegt: unsichtbar (temp) oder neben der .pages-Datei";
pub const HELP_INSTALL: &str = "Installieren und Doppelklick auf .pages-Dateien einrichten (nur für den aktuellen Benutzer)";
pub const HELP_NO_ASSOCIATE: &str = ".pages-Dateien nicht zuordnen, nur installieren";
pub const HELP_UNINSTALL: &str = "pages-bridge wieder entfernen (Einstellungen und Sicherungen bleiben)";

pub const WINDOW_TITLE: &str = "Pages öffnen";
pub const FILE_TYPE_NAME: &str = "Apple Pages-Dokument";

// The setup and settings window.
pub const SETUP_TITLE: &str = "pages-bridge";
pub const SETUP_INSTRUCTION: &str = "pages-bridge installieren";
pub const SETTINGS_INSTRUCTION: &str = "Einstellungen";
pub const SETUP_CONTENT: &str = "Öffnet Apple-Pages-Dokumente in Word oder WordCraft und schreibt jedes Speichern in \
                                 die .pages-Datei zurück.\n\nPages-Dokumente öffnen mit:";
pub const CHOICE_WORDCRAFT: &str = "WordCraft";
pub const CHOICE_WORD: &str = "Microsoft Word";
pub const CHOICE_DEFAULT: &str = "dem Standardprogramm für Word-Dateien";
pub const SETUP_ASSOCIATE: &str = ".pages-Dateien per Doppelklick so öffnen";
pub const SETUP_FOOTER: &str = "Wird nur für dich installiert, ohne Administratorrechte.\n\
                                Ändern oder entfernen: Startmenü › pages-bridge Einstellungen";
pub const BUTTON_INSTALL: &str = "Installieren";
pub const BUTTON_SAVE: &str = "Speichern";
pub const BUTTON_UNINSTALL: &str = "Deinstallieren";
pub const UNINSTALL_CONFIRM: &str =
    "pages-bridge entfernen?\n\nDeine Pages-Dateien, die Einstellungen und die Sicherungskopien bleiben erhalten.";
/// Start menu entry that opens the settings.
pub const SHORTCUT_NAME: &str = "pages-bridge Einstellungen";
pub const SHORTCUT_DESCRIPTION: &str = "Einstellen, womit Pages-Dokumente geöffnet werden";

pub fn not_found(program: &str) -> String {
    format!("{program} (nicht gefunden)")
}

pub fn current_program(program: &str) -> String {
    format!("{program} (bisherige Einstellung)")
}

pub fn setup_done(program: &str, associated: bool) -> String {
    if associated {
        format!(
            "Fertig. Ein Doppelklick auf eine .pages-Datei öffnet sie jetzt mit {program}, und jedes Speichern landet \
             in der .pages-Datei.\n\nÄndern oder entfernen: Startmenü › {SHORTCUT_NAME}"
        )
    } else {
        format!("Fertig. pages-bridge ist installiert, .pages-Dateien sind ihm aber nicht zugeordnet.\n\n\
                 Ändern oder entfernen: Startmenü › {SHORTCUT_NAME}")
    }
}

pub fn uninstalled(data: &Path) -> String {
    format!(
        "pages-bridge wurde entfernt.\n\nEinstellungen, Sicherungskopien und Protokoll liegen weiter hier und können \
         gelöscht werden:\n{}",
        data.display()
    )
}

pub fn uninstall_failed(why: &str) -> String {
    format!("pages-bridge konnte nicht vollständig entfernt werden:\n{why}")
}

pub fn backed_up(path: &Path) -> String {
    format!("Sicherung der vorherigen Fassung: {}", path.display())
}

pub const NOTE_NO_BODY: &str = "Hinweis: Layout-Dokument ohne Fließtext, nur die Textfelder wurden übernommen.";
pub const SKIP_OBJECT: &str = "Objekt";
pub const SKIP_NESTED_TABLE: &str = "Tabelle in Tabelle oder Kopfzeile";
pub const SKIP_HEADER_IMAGE: &str = "Bild in Kopf-/Fußzeile";
pub const SKIP_IMAGE: &str = "Bild";
pub const SKIP_IMAGE_FORMAT: &str = "Bild in unbekanntem Format";

/// Name of the Word style for a Pages style changed on some paragraphs ("Text*" in Pages).
pub fn style_variation(base: &str, n: usize) -> String {
    if n <= 1 { format!("{base} (geändert)") } else { format!("{base} (geändert {n})") }
}

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
        Error::Package(iwork::Error::Unsupported(what)) => {
            format!("Diese Pages-Datei kann nicht geschrieben werden ({what}):\n{path}")
        }
        Error::Docx(why) => format!("Die Word-Datei konnte nicht gelesen werden:\n{why}"),
        Error::Unsupported(crate::writeback::Unsupported::LayoutDocument) => {
            "Layout-Dokumente ohne Fließtext können noch nicht zurückgeschrieben werden.".to_owned()
        }
        Error::Unsupported(crate::writeback::Unsupported::Content(what)) => format!(
            "Das Pages-Dokument enthält {what}. Das Zurückschreiben würde das zerstören und ist deshalb noch nicht möglich:\n{path}"
        ),
        Error::Verification(why) => {
            format!("Die geschriebene Pages-Datei hat die Kontrolle nicht bestanden und wurde verworfen:\n{why}")
        }
    }
}

pub const NO_CHANGES: &str = "Keine Änderungen gefunden, es wurde nichts geschrieben.";

/// Name part of the copy write-back creates: „Name (bearbeitet).pages“.
pub const CONFLICT_COPY_LABEL: &str = "bearbeitet";
/// Name part of a Word file kept because not everything could be written back.
pub const KEPT_DOCX_LABEL: &str = "Word-Fassung";
/// Folder in the program's data folder that keeps copies of overwritten .pages files.
pub const BACKUP_FOLDER: &str = "Sicherungen";

pub fn conflict(copy: &Path) -> String {
    format!(
        "Die Pages-Datei wurde während der Bearbeitung woanders geändert (z. B. über iCloud).\n\
         Damit nichts verloren geht, stehen deine Änderungen in einer eigenen Datei:\n{}",
        copy.display()
    )
}

pub fn session_not_written(kinds: &str) -> String {
    format!("Nicht in die Pages-Datei übernommen: {kinds}.\nDas steht nur in der Word-Datei.")
}

pub fn kept_docx(path: &Path) -> String {
    format!("Die Word-Datei mit allen Änderungen liegt hier:\n{}", path.display())
}

pub fn pages_not_found(path: &Path) -> String {
    format!(
        "Die zugehörige Pages-Datei wurde nicht gefunden:\n{}\n\npages-bridge schreibt Änderungen in die .pages-Datei, \
         aus der die .docx umgewandelt wurde; ganz neue .pages-Dateien kann es nicht anlegen. Liegt das Original woanders \
         oder heißt es anders, bitte mit --pages angeben.",
        path.display()
    )
}

pub fn written_back(path: &Path, paragraphs: usize, cells: usize) -> String {
    format!("Zurückgeschrieben: {} ({paragraphs} Absätze und {cells} Tabellenzellen geändert)", path.display())
}

pub fn not_written(count: usize, kinds: &str) -> String {
    format!("Nicht übernommen ({count}): {kinds}")
}

// Content that blocks write-back, as in "Das Pages-Dokument enthält …".
pub const CONTENT_COLUMNS: &str = "wechselnde Spaltenlayouts";
pub const CONTENT_PARAGRAPH_DATA: &str = "besondere Absatzdaten";
pub const CONTENT_BOOKMARKS: &str = "Lesezeichen";
pub const CONTENT_FOOTNOTES: &str = "Fußnoten";
pub const CONTENT_SECTIONS: &str = "mehrere Abschnitte";
pub const CONTENT_RUBY: &str = "Ruby-Text";
pub const CONTENT_DICTATION: &str = "Diktat-Daten";
pub const CONTENT_TRACKED_CHANGES: &str = "verfolgte Änderungen";
pub const CONTENT_COMMENTS: &str = "Kommentare oder Hervorhebungen";
pub const CONTENT_BIDI: &str = "Text mit wechselnder Schreibrichtung";
pub const CONTENT_ANNOTATIONS: &str = "Stift-Anmerkungen";
pub const CONTENT_TATECHUYOKO: &str = "Tate-Chu-Yoko-Text";
pub const CONTENT_DROP_CAPS: &str = "Initialen";

// Edits write-back reports instead of carrying over.
pub const NOT_WRITTEN_FORMAT: &str = "Formatierung ohne passende Vorlage im Dokument";
pub const NOT_WRITTEN_PARA_STYLE: &str = "Absatz-Formatvorlage, die es im Pages-Dokument nicht gibt";
pub const NOT_WRITTEN_LINK: &str = "neuer Link";
pub const NOT_WRITTEN_OBJECT: &str = "neues Bild oder Objekt";
pub const NOT_WRITTEN_TABLE: &str = "neue Tabelle";
pub const NOT_WRITTEN_TABLE_SHAPE: &str = "geänderte Zeilen- oder Spaltenanzahl";
pub const NOT_WRITTEN_LIST: &str = "Aufzählung ohne passenden Listenstil";
pub const NOT_WRITTEN_CELL: &str = "Zelle mit Zahl, Datum oder formatiertem Text";
pub const NOT_WRITTEN_CELL_AREA: &str = "Zelle außerhalb des von Pages gespeicherten Tabellenbereichs";
pub const NOT_WRITTEN_TEXTBOX: &str = "Änderung in einem Textfeld";

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

pub fn installed(dir: &Path, associated: bool) -> String {
    let first = if associated {
        "Fertig: Ein Doppelklick auf eine .pages-Datei öffnet sie jetzt umgewandelt."
    } else {
        "Fertig: pages-bridge ist installiert, .pages-Dateien sind ihm nicht zugeordnet."
    };
    format!("{first}\nProgramm installiert in: {}", dir.display())
}

pub fn install_failed(why: &str) -> String {
    format!("pages-bridge konnte nicht eingerichtet werden:\n{why}")
}

#[cfg_attr(windows, allow(dead_code))]
pub const NOT_ON_THIS_SYSTEM: &str = "Das geht nur unter Windows.";
