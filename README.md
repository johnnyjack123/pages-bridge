# pages-bridge

[![Build](https://github.com/johnnyjack123/pages-bridge/actions/workflows/build.yml/badge.svg)](https://github.com/johnnyjack123/pages-bridge/actions/workflows/build.yml)

Öffnet Apple-Pages-Dokumente (`.pages`, ab Pages 5 / 2013) unter Windows in Word, WordCraft oder
einem anderen Textprogramm. Jedes Speichern wird in die `.pages`-Datei zurückgeschrieben, sodass du
scheinbar direkt mit der Pages-Datei arbeitest und sie danach auf dem iPhone, iPad oder Mac
weiterbearbeiten kannst. Auf der Kommandozeile wandelt pages-bridge außerdem zwischen `.pages` und
`.docx` um.

pages-bridge ist ein unabhängiges Projekt und hat nichts mit Apple zu tun. Pages ist eine Marke
von Apple Inc.

## Installieren

1. Unter [Releases](https://github.com/johnnyjack123/pages-bridge/releases) das Setup für deinen
   PC herunterladen:
   - `pages-bridge-setup-x64.exe` für die meisten PCs (Intel, AMD)
   - `pages-bridge-setup-arm64.exe` für Windows auf ARM (z. B. Snapdragon)

   Welches du brauchst, steht unter *Einstellungen › System › Info › Systemtyp*. Ohne Release
   findest du die Dateien beim neuesten erfolgreichen Lauf unter
   [Actions](https://github.com/johnnyjack123/pages-bridge/actions) (dafür ist eine Anmeldung bei
   GitHub nötig).
2. Das Setup starten. Weil das Programm nicht signiert ist, warnt Windows beim ersten Start
   („Der Computer wurde durch Windows geschützt“): *Weitere Informationen › Trotzdem ausführen*.
3. Auswählen, womit Pages-Dokumente geöffnet werden sollen (WordCraft, Word oder das
   Standardprogramm für Word-Dateien), und *Installieren* klicken.

pages-bridge wird nur für dich installiert und braucht keine Administratorrechte. Es landet in
`%LOCALAPPDATA%\Programs\pages-bridge`, und dieser Ordner wird in deinen `PATH` aufgenommen,
damit `pages-bridge` in jedem neu geöffneten Terminal funktioniert.

### Einstellungen ändern oder entfernen

- *Startmenü › pages-bridge Einstellungen*: Programm wechseln, Doppelklick-Zuordnung an- oder
  ausschalten, deinstallieren.
- *Einstellungen › Apps › Installierte Apps › pages-bridge*: *Ändern* öffnet dieselben
  Einstellungen, *Deinstallieren* entfernt das Programm.
- Im Terminal: `pages-bridge settings`, `pages-bridge config …` und `pages-bridge uninstall`
  (siehe unten).

Beim Entfernen bleiben deine Einstellungen, Sicherungskopien und das Protokoll in
`%APPDATA%\pages-bridge` und `%LOCALAPPDATA%\pages-bridge` erhalten.

## Was beim Doppelklick passiert

1. Die `.pages` wird in eine `.docx` umgewandelt und im eingestellten Programm geöffnet. Die
   `.docx` liegt dabei unsichtbar in `%LOCALAPPDATA%\pages-bridge\open`.
2. Bei jedem Speichern schreibt pages-bridge die Änderungen im Hintergrund in die `.pages` zurück.
   Vor dem ersten Überschreiben einer Sitzung kommt eine Sicherungskopie nach
   `%LOCALAPPDATA%\pages-bridge\Sicherungen` (die neuesten 30 bleiben).
3. Schließt du das Dokument, endet die Sitzung. Eine Meldung erscheint nur, wenn etwas nicht in die
   `.pages` passte (siehe unten) oder schiefging. Dann bleibt eine `Name (Word-Fassung).docx` mit
   allen Änderungen neben der `.pages` liegen.

Ändert sich die `.pages` während der Bearbeitung woanders (z. B. über iCloud), wird sie nicht
überschrieben: Deine Änderungen landen dann in `Name (bearbeitet).pages`.

Wie lange ein Dokument offen ist, erkennt pages-bridge am gestarteten Programm (WordCraft öffnet
jedes Dokument in einem eigenen Prozess) oder an der Dateisperre (Word gibt Dateien an ein schon
laufendes Word weiter und sperrt sie, solange sie offen sind). Was passiert ist, steht in
`%LOCALAPPDATA%\pages-bridge\log.txt`.

Pages’ Formatvorlagen kommen als Word-Formatvorlagen mit: Pages’ „Text“ wird „Standard“ (Normal),
Titel, Untertitel und Überschriften werden Words eingebaute Vorlagen. Wendest du in Word oder WordCraft
eine Formatvorlage an, bekommt der Absatz in der `.pages` die entsprechende Pages-Vorlage. Absätze,
deren Vorlage in Pages abgewandelt wurde (in Pages mit `*` markiert), heißen in Word
„Text (geändert)“.

## Kommandozeile

```text
pages-bridge Bericht.pages [weitere.pages …]     .pages in .docx umwandeln, die .docx landet daneben
pages-bridge to-docx Bericht.pages -o Ziel.docx  dasselbe, mit Zieldatei
pages-bridge to-docx Bericht.pages --overwrite   eine vorhandene Bericht.docx ersetzen

pages-bridge to-pages Bericht.docx               Änderungen in „Bericht (bearbeitet).pages“ schreiben
pages-bridge to-pages Bericht.docx -i            Änderungen direkt in Bericht.pages schreiben (mit Sicherung)
pages-bridge to-pages Bericht.docx --pages Original.pages -o Ziel.pages

pages-bridge open Bericht.pages                  umwandeln, öffnen, Änderungen zurückschreiben (wie Doppelklick)

pages-bridge settings                            Einstellungsfenster öffnen
pages-bridge config                              Einstellungen anzeigen
pages-bridge config --open-with word             künftig in Word öffnen (wordcraft, word, default oder Pfad zu einer .exe)
pages-bridge config --write-back off             nicht mehr zurückschreiben (on schaltet es wieder ein)
pages-bridge config --docx-location next-to-pages   die .docx sichtbar neben die .pages legen (temp: unsichtbar)

pages-bridge install [--open-with word] [--no-associate]   installieren, ohne Fenster
pages-bridge uninstall                           wieder entfernen
```

`to-pages` schreibt die Änderungen in die `.pages`-Datei, aus der die `.docx` umgewandelt wurde. Sie
wird unter demselben Namen im selben Ordner gesucht (`Bericht.docx` → `Bericht.pages`), sonst mit
`--pages` angeben. Eine ganz neue `.pages`-Datei aus einer beliebigen `.docx` kann pages-bridge
nicht erzeugen: Das Pages-Format ist nicht dokumentiert, und pages-bridge verändert nur Dateien,
die Pages selbst angelegt hat.

## Einstellungen

`%APPDATA%\pages-bridge\config.json`, wird beim ersten Start angelegt. Das Einstellungsfenster und
`pages-bridge config` ändern sie, von Hand geht es auch:

```json
{
  "open_with": "wordcraft",
  "programs": {
    "word": "winword.exe",
    "wordcraft": "C:\\Program Files\\ArtCraft\\WordCraft\\wordcraft.exe"
  },
  "existing_docx": "open_if_newer",
  "write_back": true,
  "docx_location": "temp"
}
```

- `open_with`: ein Name aus `programs`, `default` (das Windows-Standardprogramm für `.docx`) oder
  der Pfad zu einer `.exe`.
- `programs`: Name → Pfad. Ein bloßer Programmname wie `winword.exe` wird wie im Windows-Explorer
  gesucht (App Paths, dann `PATH`).
- `write_back` (Standard `true`): Jedes Speichern wird in die `.pages` zurückgeschrieben.
- `docx_location`: Mit `temp` (Standard) bleibt die `.docx` unsichtbar und wird nach dem
  Schließen gelöscht. `next_to_pages` legt sie sichtbar neben die `.pages`.
- `existing_docx` (nur mit `next_to_pages`): was passiert, wenn `X.docx` neben `X.pages` schon
  existiert:
  - `open_if_newer`: Ist sie neuer als die `.pages`, wird sie geöffnet, damit eigene Änderungen
    nicht verloren gehen. Ist sie älter und stammt von pages-bridge, wird sie neu erzeugt. Eine
    ältere fremde Datei bleibt stehen, dann entsteht `X (2).docx`.
  - `new_copy`: Es entsteht immer eine neue nummerierte Kopie.
  - `overwrite`: Die vorhandene Datei wird ersetzt.

## Zurückschreiben in die Pages-Datei

pages-bridge vergleicht die bearbeitete `.docx` mit der Pages-Datei. Dann ändert es nur den
Fließtext, geänderte Tabellenzellen und den gespeicherten Layout-Cache. Alles andere bleibt Byte
für Byte erhalten. Bevor eine neue Datei akzeptiert wird, liest pages-bridge sie selbst wieder ein
und prüft den Text. Pages auf dem iPhone hat die so geschriebenen Dateien geöffnet.

Übernommen werden:

- neuer, geänderter und gelöschter Text, neue und gelöschte Absätze
- Absatz-Formatvorlagen, die es im Pages-Dokument gibt (Titel, Überschriften, Text …)
- Aufzählungen ein und aus, die Listenebene
- Text in Tabellenzellen, auch in bisher leeren Zellen
- fett, kursiv, unterstrichen und durchgestrichen, soweit das Dokument dafür schon eine
  Zeichenvorlage benutzt
- Links, deren Adresse im Dokument schon vorkommt

Gemeldet und nicht übernommen werden Formatierungen ohne passende Vorlage, Formatvorlagen, die es
nur in Word gibt, neue Links, Bilder und Tabellen, Änderungen an Zeilen- und Spaltenanzahl und
Änderungen in Textfeldern. Dokumente mit Fußnoten, Kommentaren, verfolgten Änderungen, Lesezeichen
oder mehreren Abschnitten lehnt pages-bridge ab, weil das Zurückschreiben diese Inhalte zerstören
würde.

## Was umgewandelt wird

- Text, Absatz- und Zeichenformate: Schrift, Größe, fett, kursiv, unterstrichen, durchgestrichen,
  Farbe, hoch- und tiefgestellt, Ausrichtung, Einzüge, Abstände, Zeilenabstand, Tabulatoren,
  Linien über und unter Absätzen.
- Formatvorlagen, Aufzählungen und Nummerierungen, Links, Seitenumbrüche.
- Tabellen mit Spaltenbreiten, Zellabstand, Linien und abwechselnden Zeilenfarben.
- Bilder im Text und mit dem Text verankerte, frei stehende Bilder. HEIC und PDF nur, wenn
  ImageMagick installiert ist.
- Seitenformat und Ränder, Kopf- und Fußzeilen mit Seitenzahl.
- Text in Textfeldern, die im Text verankert sind.

Nicht übernommen werden Diagramme, Formen ohne Text, frei auf der Seite platzierte Objekte,
Fußnoten, Kommentare und verbundene Zellen. Dateien aus iWork ’09 werden nicht unterstützt.

### Seitenumbrüche wie in Pages

Pages-Dokumente schieben Inhalte oft mit Leerzeilen auf die nächste Seite. Damit das in Word und
WordCraft genauso umbricht, bildet pages-bridge Pages’ Zeilenhöhen nach. Die Regeln wurden an
einem PDF-Export aus Pages gemessen: Ober- und Unterlänge werden einzeln auf ganze Punkte gerundet,
der Zusatzabstand von 1,5-zeilig steht unter der Zeile, und die letzte Zeile einer Seite darf mit
ihrer Unterlänge in den Rand ragen. Am Referenzdokument liegt jede Zeile aller 5 Seiten auf
±3 px genau an der Stelle wie in Pages. Gemessen ist das bisher nur für Helvetica Neue, die
Standardschrift von Pages.

## Selbst bauen

Mit [Rust](https://rustup.rs) (ab 1.85) unter Windows:

```text
cargo test                     Unit-Tests, synthetisches Dokument, Referenzdokumente (falls vorhanden)
cargo clippy --all-targets
cargo build --release          target\release\pages-bridge.exe und pages-bridgew.exe
target\release\pages-bridge.exe install
```

Das Setup ist `pages-bridgew.exe` mit dem Kommandozeilenprogramm darin. Es entsteht, wenn
`PAGES_BRIDGE_EMBED_CLI` beim Bauen auf ein fertiges `pages-bridge.exe` zeigt:

```powershell
cargo build --release --bin pages-bridge
$env:PAGES_BRIDGE_EMBED_CLI = "$PWD\target\release\pages-bridge.exe"
cargo build --release --bin pages-bridgew
```

Die [GitHub Action](.github/workflows/build.yml) macht das bei jedem Push für x64 und ARM64 und
legt die Dateien als Artefakte ab. Ein Tag `v…` veröffentlicht sie als Release.

## Aufbau

| Pfad | Inhalt |
|---|---|
| `crates/iwork` | Container (ZIP oder Paketordner), Snappy-komprimierte IWA-Archive, Protobuf ohne Schema. Unabhängig von Pages, also auch für Numbers und Keynote nutzbar. |
| `crates/pages-bridge/src/reader` | Pages → Dokumentmodell: Formatvorlagen, Text, Listen, Tabellen, Bilder |
| `crates/pages-bridge/src/model.rs` | Formatneutrales Dokumentmodell, die Schnittstelle zwischen Lesen und Schreiben |
| `crates/pages-bridge/src/docx` | Modell → `.docx`: Formatvorlagen, Schriftzuordnung und Pages-Zeilenhöhen |
| `crates/pages-bridge/src/writeback` | bearbeitete `.docx` → Pages: docx lesen, Absätze abgleichen, Text und Zellen neu kodieren |
| `crates/pages-bridge/src/sync.rs` | die Bearbeitungssitzung: Speichern erkennen, zurückschreiben, Ende erkennen |
| `crates/pages-bridge/src/setup.rs` | das Installations- und Einstellungsfenster |
| `crates/pages-bridge/src/cli.rs`, `config.rs`, `system.rs` | Kommandozeile, Einstellungen, Windows-Anbindung und Installation |
| `reference/` | Echte Pages-Dateien mit PDF aus Pages. Testgrundlage, nicht im Repository |
| `prototype/` | Der erste Python-Prototyp, ersetzt durch das Rust-Programm |

Die Bedeutung der Pages-Felder ist nirgends dokumentiert. Jede Feldnummer im Code wurde an
Beispieldateien ermittelt und ist dort kommentiert.

## Lizenz

Nach deiner Wahl unter der [MIT-Lizenz](LICENSE-MIT) oder der
[Apache-Lizenz 2.0](LICENSE-APACHE).
