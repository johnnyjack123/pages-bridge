//! The JSON settings file: which program opens converted documents, and what to do when a
//! `.docx` of the same name already exists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const WORDCRAFT_DEFAULT_PATH: &str = r"C:\Program Files\ArtCraft\WordCraft\wordcraft.exe";

/// What happens when `X.docx` already exists next to `X.pages`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExistingDocx {
    /// Open the existing file if it is newer than the `.pages` (it may hold the user's edits),
    /// otherwise write a numbered copy.
    #[default]
    OpenIfNewer,
    /// Always write a numbered copy (`X (2).docx`).
    NewCopy,
    /// Replace it.
    Overwrite,
}

/// Where the `.docx` for editing goes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocxLocation {
    /// Next to the `.pages`, where the user sees it.
    NextToPages,
    /// A private folder; removed after the editor closes when everything was written back,
    /// so the user only ever sees the `.pages`.
    #[default]
    Temp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    /// A key of `programs`, `default` (Windows' program for .docx) or the path of an `.exe`.
    pub open_with: String,
    /// Name → program path, or a bare `.exe` name looked up in Windows' App Paths and PATH.
    pub programs: BTreeMap<String, String>,
    pub existing_docx: ExistingDocx,
    /// Write every save in the word processor back into the `.pages`.
    pub write_back: bool,
    pub docx_location: DocxLocation,
}

impl Default for Config {
    fn default() -> Self {
        let programs = BTreeMap::from([
            ("wordcraft".to_owned(), WORDCRAFT_DEFAULT_PATH.to_owned()),
            ("word".to_owned(), "winword.exe".to_owned()),
        ]);
        let open_with = if Path::new(WORDCRAFT_DEFAULT_PATH).is_file() { "wordcraft" } else { "default" };
        Self {
            open_with: open_with.to_owned(),
            programs,
            existing_docx: ExistingDocx::default(),
            write_back: true,
            docx_location: DocxLocation::default(),
        }
    }
}

/// `%APPDATA%\pages2docx\config.json`, or next to the program where there is no `%APPDATA%`.
pub fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf))
    });
    base.unwrap_or_default().join("pages2docx").join("config.json")
}

/// Loads the settings, writing the defaults first if there is no file yet.
///
/// A file that does not parse is left untouched and reported, so a typo never silently
/// discards the user's settings.
pub fn load_or_create(path: &Path) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{FEFF}')).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let config = Config::default();
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, default_file(&config)).map_err(|e| e.to_string())?;
            Ok(config)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The defaults plus an explanation; JSON has no comments, and unknown keys are ignored.
fn default_file(config: &Config) -> String {
    let mut value = serde_json::to_value(config).unwrap_or_default();
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "_hinweis".to_owned(),
            serde_json::Value::String(
                "open_with: \"wordcraft\", \"word\", \"default\" (Windows-Standardprogramm für .docx) oder Pfad zu einer \
                 .exe. existing_docx: \"open_if_newer\", \"new_copy\" oder \"overwrite\". write_back: true schreibt jedes \
                 Speichern in die .pages zurück. docx_location: \"temp\" (die .docx bleibt unsichtbar und wird nach \
                 dem Schließen gelöscht) oder \"next_to_pages\" (die .docx liegt sichtbar neben der .pages)."
                    .to_owned(),
            ),
        );
    }
    serde_json::to_string_pretty(&value).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_keys() {
        let config: Config = serde_json::from_str(r#"{"open_with": "word", "_hinweis": "x"}"#).unwrap();
        assert_eq!(config.open_with, "word");
        assert_eq!(config.existing_docx, ExistingDocx::OpenIfNewer);
        assert!(config.programs.contains_key("wordcraft"));
        // Settings files written before write-back existed get it switched on.
        assert!(config.write_back);
        assert_eq!(config.docx_location, DocxLocation::Temp);
        let text = default_file(&config);
        assert_eq!(serde_json::from_str::<Config>(&text).unwrap(), config);
    }
}
