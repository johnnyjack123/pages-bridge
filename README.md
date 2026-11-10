# pages2docx

Wandelt Apple-Pages-Dokumente (`.pages`, ab Pages 5 / 2013) in Word-Dateien (`.docx`) um und
öffnet sie im eingestellten Programm, etwa WordCraft oder Word. Läuft als reiner Konverter auf der
Kommandozeile oder als Doppelklick-Programm für `.pages`-Dateien.

## Benutzen

```text
pages2docx Datei.pages [weitere.pages …]   # umwandeln, .docx landet daneben
pages2docx Datei.pages -o Ziel.docx        # umwandeln an einen bestimmten Ort
pages2docx Datei.pages --overwrite         # vorhandene Datei.docx ersetzen
pages2docx open Datei.pages                # umwandeln und im eingestellten Programm öffnen
pages2docx install                         # Doppelklick auf .pages einrichten (nur aktueller Benutzer)
pages2docx uninstall                       # Doppelklick-Zuordnung wieder entfernen
pages2docx config                          # Einstellungsdatei anzeigen
```

`install` kopiert `pages2docx.exe` und `pages2docxw.exe` nach
`%LOCALAPPDATA%\Programs\pages2docx` und meldet `.pages` dort an. `pages2docxw.exe` ist dasselbe
Programm ohne Konsolenfenster: Fehler erscheinen als Meldungsfenster, und auf das Programm
gezogene Dateien werden wie beim Doppelklick geöffnet.

Ohne vorherige Zuordnung reicht auch: `.pages`-Datei auf `pages2docxw.exe` ziehen.

## Einstellungen

`%APPDATA%\pages2docx\config.json`, wird beim ersten Start angelegt:

```json
{
  "open_with": "wordcraft",
  "programs": {
    "word": "winword.exe",
    "wordcraft": "C:\\Program Files\\ArtCraft\\WordCraft\\wordcraft.exe"
  },
  "existing_docx": "open_if_newer"
}
```

- `open_with`: ein Name aus `programs`, `default` (das Windows-Standardprogramm für `.docx`) oder
  der Pfad zu einer `.exe`.
- `programs`: Name → Pfad. Ein bloßer Programmname wie `winword.exe` wird wie im Windows-Explorer
  gesucht (App Paths, dann `PATH`).
- `existing_docx`: was passiert, wenn `X.docx` neben `X.pages` schon existiert:
  - `open_if_newer`: Ist sie neuer als die `.pages`, wird sie geöffnet, damit eigene Änderungen
    nicht verloren gehen. Sonst entsteht `X (2).docx`.
  - `new_copy`: Es entsteht immer eine neue nummerierte Kopie.
  - `overwrite`: Die vorhandene Datei wird ersetzt.

## Was übernommen wird

- Text, Absatz- und Zeichenformate: Schrift, Größe, fett, kursiv, unterstrichen, durchgestrichen,
  Farbe, hoch- und tiefgestellt, Ausrichtung, Einzüge, Abstände, Zeilenabstand, Tabulatoren.
- Aufzählungen und Nummerierungen, Links, Seitenumbrüche.
- Tabellen mit Spaltenbreiten, Zellabstand, Linien und abwechselnden Zeilenfarben.
- Eingebettete Bilder. HEIC und PDF nur, wenn ImageMagick installiert ist.
- Seitenformat und Ränder, Kopf- und Fußzeilen mit Seitenzahl.
- Text in Textfeldern, die im Text verankert sind.

Nicht übernommen werden Diagramme, Formen ohne Text, frei auf der Seite platzierte Objekte,
Fußnoten, Kommentare und verbundene Zellen. Dateien aus iWork ’09 werden nicht unterstützt.

### Seitenumbrüche wie in Pages

Pages-Dokumente schieben Inhalte oft mit Leerzeilen auf die nächste Seite. Damit das in Word und
WordCraft genauso umbricht, bildet der Konverter Pages’ Zeilenhöhen nach. Die Regeln wurden an
einem PDF-Export aus Pages gemessen: Ober- und Unterlänge werden einzeln auf ganze Punkte gerundet,
der Zusatzabstand von 1,5-zeilig steht unter der Zeile, und die letzte Zeile einer Seite darf mit
ihrer Unterlänge in den Rand ragen. Am Referenzdokument liegt jede Zeile aller 5 Seiten auf
±3 px genau an der Stelle wie in Pages. Gemessen ist das bisher nur für Helvetica Neue, die
Standardschrift von Pages.

## Aufbau

| Pfad | Inhalt |
|---|---|
| `crates/iwork` | Container (ZIP oder Paketordner), Snappy-komprimierte IWA-Archive, Protobuf ohne Schema. Unabhängig von Pages, also auch für Numbers und Keynote nutzbar. |
| `crates/pages2docx/src/reader` | Pages → Dokumentmodell: Formatvorlagen, Text, Listen, Tabellen, Bilder |
| `crates/pages2docx/src/model.rs` | Formatneutrales Dokumentmodell, die Schnittstelle zwischen Lesen und Schreiben |
| `crates/pages2docx/src/docx` | Modell → `.docx`, Schriftzuordnung und Pages-Zeilenhöhen |
| `crates/pages2docx/src/cli.rs`, `config.rs`, `system.rs` | Kommandozeile, Einstellungen, Windows-Anbindung |
| `reference/` | Echte Pages-Dateien mit PDF aus Pages. Testgrundlage, nicht zum Weitergeben |
| `prototype/` | Der erste Python-Prototyp, ersetzt durch das Rust-Programm |

Die Bedeutung der Pages-Felder ist nirgends dokumentiert. Jede Feldnummer im Code wurde an
Beispieldateien ermittelt und ist dort kommentiert.

## Entwickeln

```text
cargo test                  # Unit-Tests, synthetisches Dokument, Referenzdokument (falls vorhanden)
cargo clippy --all-targets
cargo build --release       # target\release\pages2docx.exe und pages2docxw.exe
```

Danach `target\release\pages2docx.exe install`, um die installierte Version zu aktualisieren.
