# pages-bridge

[![Build](https://github.com/johnnyjack123/pages-bridge/actions/workflows/build.yml/badge.svg)](https://github.com/johnnyjack123/pages-bridge/actions/workflows/build.yml)

**English** · [Deutsch](README.de.md)

Edit Apple Pages documents on Windows in Word, WordCraft or any other word processor. pages-bridge
converts a `.pages` file to `.docx`, opens it, and writes every save back into the original
`.pages` file, so the document stays editable in Pages on iPhone, iPad and Mac. On the command
line it converts between `.pages` and `.docx`.

## Features

- Double-click a `.pages` file to edit it in WordCraft, Microsoft Word or your default `.docx` app.
- Edits are written back on every save; the original is backed up before it is first changed.
- Faithful conversion: styles, lists, tables, images, headers and footers, and line heights
  matched to Pages so that pages break in the same places.
- Pages paragraph styles become Word styles and map back to Pages when applied.
- Installs per user, without administrator rights.

The user interface is currently in German. Pages documents from Pages 5 (2013) onwards are
supported.

## Installation

Download the setup from the [latest release](https://github.com/johnnyjack123/pages-bridge/releases/latest):
`pages-bridge-setup-x64.exe` for most PCs, `pages-bridge-setup-arm64.exe` for Windows on ARM.
Run it, choose the word processor and click *Installieren*.

The setup is not code-signed, so Windows SmartScreen may warn on first launch
(*More info › Run anyway*).

To change the word processor or uninstall later, use the Start menu entry
*pages-bridge Einstellungen*, *Settings › Apps › Installed apps*, or the command line.

## Command line

```text
pages-bridge Report.pages                 convert to Report.docx (same as to-docx)
pages-bridge to-docx Report.pages -o Out.docx
pages-bridge to-pages Report.docx         write the edits to "Report (bearbeitet).pages"
pages-bridge to-pages Report.docx -i      write the edits into Report.pages, keeping a backup
pages-bridge open Report.pages            convert, open and write back, like a double-click
pages-bridge settings                     open the settings window
pages-bridge config --open-with word      wordcraft, word, default or the path to an .exe
pages-bridge install | uninstall
```

`to-pages` updates the `.pages` file the `.docx` was converted from (found by name, or given with
`--pages`). It cannot create a `.pages` file from an arbitrary `.docx`.

## Writing back

pages-bridge compares the edited `.docx` with the `.pages` file and changes only the body text,
edited table cells and Pages' layout cache; everything else is kept byte for byte. Every written
file is read back and checked before it replaces anything.

| Written back | Reported, not written back |
|---|---|
| Text and paragraphs | Formatting the document has no style for |
| Paragraph styles of the document | Styles that exist only in Word |
| Lists and list levels | New links, images and tables |
| Text in table cells | Added or removed rows and columns |
| Bold, italic, underline, strikethrough via existing styles | Edits in text boxes |
| Links to addresses the document already uses | |

Documents with footnotes, comments, tracked changes, bookmarks or several sections are not written
back, since that would lose content.

Not converted at all: charts, shapes without text, objects placed freely on the page, footnotes,
comments, merged cells, and iWork ’09 files.

## Configuration

Settings are stored in `%APPDATA%\pages-bridge\config.json` and can be changed with
`pages-bridge config`, the settings window, or by hand:

| Key | Default | Meaning |
|---|---|---|
| `open_with` | `wordcraft` if installed, else `default` | a key of `programs`, `default`, or the path to an `.exe` |
| `programs` | `word`, `wordcraft` | name → program path or `.exe` name |
| `write_back` | `true` | write every save back into the `.pages` file |
| `docx_location` | `temp` | `temp`: hidden and deleted after closing; `next_to_pages`: visible next to the `.pages` file |
| `existing_docx` | `open_if_newer` | with `next_to_pages`: `open_if_newer`, `new_copy` or `overwrite` |

Backups and the log are kept in `%LOCALAPPDATA%\pages-bridge`.

## Building

Requires Rust 1.85 or newer on Windows.

```text
cargo test
cargo build --release
```

The setup is `pages-bridgew.exe` with the console program embedded:

```powershell
cargo build --release --bin pages-bridge
$env:PAGES_BRIDGE_EMBED_CLI = "$PWD\target\release\pages-bridge.exe"
cargo build --release --bin pages-bridgew
```

[CI](.github/workflows/build.yml) builds x64 and ARM64 on every push and publishes a release for
each `v*` tag.

| Path | Contents |
|---|---|
| `crates/iwork` | reader and writer for iWork packages (IWA archives, schema-less protobuf) |
| `crates/pages-bridge` | Pages → document model → `.docx`, write-back, editing session, setup, CLI |

Apple does not document the Pages format; every field used here was determined from sample files.

## License

Licensed under either the [MIT License](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE),
at your option.

pages-bridge is not affiliated with Apple. Pages is a trademark of Apple Inc.
