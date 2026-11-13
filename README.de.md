# pages-bridge

[![Build](https://github.com/johnnyjack123/pages-bridge/actions/workflows/build.yml/badge.svg)](https://github.com/johnnyjack123/pages-bridge/actions/workflows/build.yml)

[English](README.md) · **Deutsch**

Apple-Pages-Dokumente unter Windows in Word, WordCraft oder einem anderen Textprogramm bearbeiten.
pages-bridge wandelt eine `.pages`-Datei in `.docx` um, öffnet sie und schreibt jedes Speichern in
die ursprüngliche `.pages`-Datei zurück. So bleibt das Dokument in Pages auf iPhone, iPad und Mac
bearbeitbar. Auf der Kommandozeile wandelt es zwischen `.pages` und `.docx` um.

## Funktionen

- Doppelklick auf eine `.pages`-Datei öffnet sie in WordCraft, Microsoft Word oder dem
  Standardprogramm für `.docx`.
- Änderungen werden bei jedem Speichern zurückgeschrieben; vor der ersten Änderung wird das
  Original gesichert.
- Getreue Umwandlung: Formatvorlagen, Listen, Tabellen, Bilder, Kopf- und Fußzeilen sowie
  Zeilenhöhen wie in Pages, damit die Seiten an denselben Stellen umbrechen.
- Pages-Absatzvorlagen werden zu Word-Formatvorlagen und beim Anwenden wieder zu Pages-Vorlagen.
- Installation pro Benutzer, ohne Administratorrechte.

Unterstützt werden Dokumente ab Pages 5 (2013).

## Installation

Das Setup aus dem [neuesten Release](https://github.com/johnnyjack123/pages-bridge/releases/latest)
herunterladen: `pages-bridge-setup-x64.exe` für die meisten PCs, `pages-bridge-setup-arm64.exe`
für Windows auf ARM. Starten, Textprogramm wählen und *Installieren* klicken.

Das Setup ist nicht signiert, daher warnt Windows SmartScreen beim ersten Start eventuell
(*Weitere Informationen › Trotzdem ausführen*).

Textprogramm ändern oder deinstallieren: über den Startmenü-Eintrag *pages-bridge Einstellungen*,
*Einstellungen › Apps › Installierte Apps* oder die Kommandozeile.

## Kommandozeile

```text
pages-bridge Bericht.pages                in Bericht.docx umwandeln (wie to-docx)
pages-bridge to-docx Bericht.pages -o Ziel.docx
pages-bridge to-pages Bericht.docx        Änderungen in „Bericht (bearbeitet).pages“ schreiben
pages-bridge to-pages Bericht.docx -i     Änderungen in Bericht.pages schreiben, mit Sicherung
pages-bridge open Bericht.pages           umwandeln, öffnen, zurückschreiben, wie per Doppelklick
pages-bridge settings                     Einstellungsfenster öffnen
pages-bridge config --open-with word      wordcraft, word, default oder Pfad zu einer .exe
pages-bridge install | uninstall
```

`to-pages` aktualisiert die `.pages`-Datei, aus der die `.docx` umgewandelt wurde (gleicher Name,
oder mit `--pages` angegeben). Aus einer beliebigen `.docx` kann es keine `.pages`-Datei erzeugen.

## Zurückschreiben

pages-bridge vergleicht die bearbeitete `.docx` mit der `.pages`-Datei und ändert nur den
Fließtext, bearbeitete Tabellenzellen und Pages’ Layout-Cache; alles andere bleibt Byte für Byte
erhalten. Jede geschriebene Datei wird wieder eingelesen und geprüft, bevor sie etwas ersetzt.

| Wird zurückgeschrieben | Wird gemeldet, nicht zurückgeschrieben |
|---|---|
| Text und Absätze | Formatierung ohne passende Vorlage im Dokument |
| Absatzvorlagen des Dokuments | Formatvorlagen, die es nur in Word gibt |
| Listen und Listenebenen | neue Links, Bilder und Tabellen |
| Text in Tabellenzellen | hinzugefügte oder gelöschte Zeilen und Spalten |
| Fett, kursiv, unterstrichen, durchgestrichen über vorhandene Vorlagen | Änderungen in Textfeldern |
| Links auf Adressen, die das Dokument schon verwendet | |

Dokumente mit Fußnoten, Kommentaren, verfolgten Änderungen, Lesezeichen oder mehreren Abschnitten
werden nicht zurückgeschrieben, weil dabei Inhalte verloren gingen.

Gar nicht umgewandelt werden Diagramme, Formen ohne Text, frei auf der Seite platzierte Objekte,
Fußnoten, Kommentare, verbundene Zellen und Dateien aus iWork ’09.

## Einstellungen

Die Einstellungen liegen in `%APPDATA%\pages-bridge\config.json` und lassen sich mit
`pages-bridge config`, im Einstellungsfenster oder von Hand ändern:

| Schlüssel | Standard | Bedeutung |
|---|---|---|
| `open_with` | `wordcraft`, falls installiert, sonst `default` | ein Schlüssel aus `programs`, `default` oder Pfad zu einer `.exe` |
| `programs` | `word`, `wordcraft` | Name → Programmpfad oder `.exe`-Name |
| `write_back` | `true` | jedes Speichern in die `.pages`-Datei zurückschreiben |
| `docx_location` | `temp` | `temp`: unsichtbar, nach dem Schließen gelöscht; `next_to_pages`: sichtbar neben der `.pages`-Datei |
| `existing_docx` | `open_if_newer` | mit `next_to_pages`: `open_if_newer`, `new_copy` oder `overwrite` |

Sicherungskopien und Protokoll liegen in `%LOCALAPPDATA%\pages-bridge`.

## Selbst bauen

Benötigt Rust 1.85 oder neuer unter Windows.

```text
cargo test
cargo build --release
```

Das Setup ist `pages-bridgew.exe` mit eingebettetem Kommandozeilenprogramm:

```powershell
cargo build --release --bin pages-bridge
$env:PAGES_BRIDGE_EMBED_CLI = "$PWD\target\release\pages-bridge.exe"
cargo build --release --bin pages-bridgew
```

Die [CI](.github/workflows/build.yml) baut x64 und ARM64 bei jedem Push und veröffentlicht für
jedes `v*`-Tag ein Release.

| Pfad | Inhalt |
|---|---|
| `crates/iwork` | Lesen und Schreiben von iWork-Paketen (IWA-Archive, Protobuf ohne Schema) |
| `crates/pages-bridge` | Pages → Dokumentmodell → `.docx`, Zurückschreiben, Bearbeitungssitzung, Setup, CLI |

Apple dokumentiert das Pages-Format nicht; jedes verwendete Feld wurde an Beispieldateien ermittelt.

## Lizenz

Nach Wahl unter der [MIT-Lizenz](LICENSE-MIT) oder der [Apache-Lizenz 2.0](LICENSE-APACHE).

pages-bridge steht in keiner Verbindung zu Apple. Pages ist eine Marke von Apple Inc.
