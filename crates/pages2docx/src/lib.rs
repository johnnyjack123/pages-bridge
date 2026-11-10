//! Converts Apple Pages documents (`.pages`, Pages 5 / 2013 and later) to Word `.docx`, and opens
//! the result in a word processor chosen in a JSON settings file.
//!
//! The work is split into a reader (Pages → [`model`]) and a writer (model → docx), so other
//! targets or a way back to Pages can reuse either side.

pub mod cli;
pub mod config;
mod docx;
mod messages;
pub mod model;
mod reader;
mod system;

use std::fmt;
use std::path::Path;

pub use model::{Document, Report};

/// Written into the docx properties, so a converted file can be recognised later.
pub(crate) const CONVERTED_MARKER: &str = "Converted from Apple Pages with pages2docx";

#[derive(Debug)]
pub enum Error {
    Package(iwork::Error),
    /// The package has no Pages document object (a Numbers or Keynote file, for instance).
    NoDocument,
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Package(e) => write!(f, "{e}"),
            Error::NoDocument => f.write_str("no Pages document in this package"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<iwork::Error> for Error {
    fn from(e: iwork::Error) -> Self {
        Error::Package(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Reads a `.pages` file or bundle into the document model.
pub fn read_document(input: &Path) -> Result<(Document, Report), Error> {
    let package = iwork::Package::open(input)?;
    let title = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    reader::read(&package, &title)
}

/// Converts `input` to a `.docx` at `output`, replacing an existing file there.
pub fn convert(input: &Path, output: &Path) -> Result<Report, Error> {
    let (document, report) = read_document(input)?;
    docx::write(&document, output)?;
    Ok(report)
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian calendar.
///
/// Howard Hinnant's `civil_from_days`, <https://howardhinnant.github.io/date_algorithms.html>.
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates() {
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        assert_eq!(super::civil_from_days(11_323), (2001, 1, 1));
        assert_eq!(super::civil_from_days(-1), (1969, 12, 31));
        assert_eq!(super::civil_from_days(20_513), (2026, 3, 1));
    }
}
