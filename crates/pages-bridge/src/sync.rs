//! Keeps a `.pages` file in step with the `.docx` edited from it: every save in the word
//! processor is written back, until the document is closed.
//!
//! The end of editing shows in one of two ways. A program started for the document ends when it
//! closes (WordCraft runs one process per document). Word instead hands the document to its
//! running instance, so the started process exits at once; Word then locks the file while the
//! document is open.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant, SystemTime};

use crate::messages;
use crate::system;
use crate::writeback::{self, Options};

const TICK: Duration = Duration::from_millis(700);
/// A saved file must look unchanged this long before it is read: editors may write in steps.
const SETTLE: Duration = Duration::from_millis(1200);
/// A started program that ends this soon has handed the document to another instance; Word
/// does so within a second or two.
const HANDOFF: Duration = Duration::from_secs(6);
/// A lock seen before that stays away this long means the document was closed. Word releases
/// and takes the file again while saving.
const UNLOCKED: Duration = Duration::from_secs(4);
/// Without a process and without ever seeing a lock, the end cannot be told; stop watching then.
const NEVER_LOCKED: Duration = Duration::from_secs(90);
const MAX_SESSION: Duration = Duration::from_secs(24 * 3600);
/// Reading a docx that is half written fails; that many tries before giving up.
const READ_RETRIES: u32 = 3;
const KEEP_BACKUPS: usize = 30;

#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub(crate) saves_written: usize,
    /// What the `.pages` could not take, as of the last save.
    pub(crate) not_written: BTreeMap<String, usize>,
    /// Where edits went because the `.pages` was changed elsewhere during the session.
    pub(crate) conflict_copy: Option<PathBuf>,
    pub(crate) backup: Option<PathBuf>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        std::fs::metadata(path).ok().map(|m| Self { modified: m.modified().ok(), len: m.len() })
    }
}

enum Watch {
    Process(Child),
    Lock { seen_locked: bool, unlocked_since: Option<Instant> },
}

impl Watch {
    fn editor_open(&mut self, docx: &Path, started: Instant) -> bool {
        match self {
            Watch::Process(child) => match child.try_wait() {
                Ok(None) => true,
                _ if started.elapsed() < HANDOFF => {
                    system::log("editor process ended early, watching the file lock instead");
                    *self = Watch::Lock { seen_locked: false, unlocked_since: None };
                    true
                }
                _ => false,
            },
            Watch::Lock { seen_locked, unlocked_since } => {
                if system::is_locked(docx) {
                    *seen_locked = true;
                    *unlocked_since = None;
                    return true;
                }
                if !*seen_locked {
                    return started.elapsed() < NEVER_LOCKED;
                }
                unlocked_since.get_or_insert_with(Instant::now).elapsed() < UNLOCKED
            }
        }
    }
}

struct Session<'a> {
    pages: &'a Path,
    docx: &'a Path,
    /// The `.pages` as last written, which edits are compared against.
    base: PathBuf,
    pages_stamp: Option<Stamp>,
    /// The `.pages` itself, or the conflict copy once it was changed elsewhere.
    target: PathBuf,
    read_failures: u32,
    outcome: Outcome,
}

/// Watches `docx` while `editor` (or whatever opened it) has it open, writing every save back
/// into `pages`.
pub(crate) fn run(pages: &Path, docx: &Path, editor: Option<Child>) -> Outcome {
    let started = Instant::now();
    let dir = system::data_dir().join(format!("session-{}", std::process::id()));
    let base = dir.join("base.pages");
    let mut session = Session {
        pages,
        docx,
        base: base.clone(),
        pages_stamp: Stamp::of(pages),
        target: pages.to_path_buf(),
        read_failures: 0,
        outcome: Outcome::default(),
    };
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(pages, &base)) {
        session.outcome.error = Some(e.to_string());
        return session.outcome;
    }
    system::log(&format!("session start: {} <- {}", pages.display(), docx.display()));

    let mut watch = match editor {
        Some(child) => Watch::Process(child),
        None => Watch::Lock { seen_locked: false, unlocked_since: None },
    };
    let mut written = Stamp::of(docx);
    let mut pending: Option<(Option<Stamp>, Instant)> = None;
    loop {
        std::thread::sleep(TICK);
        let now = Stamp::of(docx);
        if now == written {
            pending = None;
        } else {
            match pending {
                Some((stamp, since)) if stamp == now => {
                    if since.elapsed() >= SETTLE {
                        if session.write() {
                            written = now;
                        }
                        pending = None;
                    }
                }
                _ => pending = Some((now, Instant::now())),
            }
        }
        if !watch.editor_open(docx, started) || started.elapsed() > MAX_SESSION {
            break;
        }
    }
    // A save just before closing.
    if Stamp::of(docx) != written {
        session.write();
    }
    let _ = std::fs::remove_dir_all(&dir);
    let o = &session.outcome;
    system::log(&format!(
        "session end: {} saves written, not written {:?}, conflict {:?}, error {:?}",
        o.saves_written, o.not_written, o.conflict_copy, o.error
    ));
    session.outcome
}

impl Session<'_> {
    /// Writes the current docx back. Returns false when it should be tried again later.
    fn write(&mut self) -> bool {
        if self.outcome.error.is_some() {
            return true;
        }
        if self.target == self.pages && Stamp::of(self.pages) != self.pages_stamp {
            let copy = free_name(self.pages, messages::CONFLICT_COPY_LABEL);
            system::log(&format!("{} changed elsewhere, writing to {}", self.pages.display(), copy.display()));
            self.outcome.conflict_copy = Some(copy.clone());
            self.target = copy;
        }
        // Next to the target, so the final rename stays on one volume and is atomic.
        let tmp = self.target.with_extension("pages.tmp");
        match writeback::write_back(&self.base, self.docx, &tmp, Options::default()) {
            Ok(report) => {
                self.read_failures = 0;
                self.outcome.not_written = report.not_written;
                if !report.written {
                    return true;
                }
                if let Err(e) = self.replace_target(&tmp) {
                    let _ = std::fs::remove_file(&tmp);
                    self.outcome.error = Some(e.to_string());
                    system::log(&format!("write failed: {e}"));
                    return true;
                }
                self.outcome.saves_written += 1;
                system::log(&format!(
                    "written back: {} paragraphs, {} cells",
                    report.paragraphs_changed, report.cells_changed
                ));
                true
            }
            Err(crate::Error::Docx(why)) if self.read_failures < READ_RETRIES => {
                self.read_failures += 1;
                system::log(&format!("docx not readable yet ({why}), retrying"));
                false
            }
            Err(e) => {
                system::log(&format!("write-back failed: {e}"));
                self.outcome.error = Some(messages::describe(&e, self.pages));
                true
            }
        }
    }

    fn replace_target(&mut self, tmp: &Path) -> std::io::Result<()> {
        if self.target == self.pages && self.outcome.backup.is_none() {
            self.outcome.backup = Some(backup(self.pages)?);
        }
        std::fs::rename(tmp, &self.target)?;
        std::fs::copy(&self.target, &self.base)?;
        if self.target == self.pages {
            self.pages_stamp = Stamp::of(self.pages);
        }
        Ok(())
    }
}

/// Copies `pages` into the backup folder before it is first overwritten, keeping the newest
/// [`KEEP_BACKUPS`] backups.
pub(crate) fn backup(pages: &Path) -> std::io::Result<PathBuf> {
    let dir = system::data_dir().join(messages::BACKUP_FOLDER);
    std::fs::create_dir_all(&dir)?;
    let stem = pages.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stamp = crate::utc_timestamp().replace(['T', ':'], "-").trim_end_matches('Z').to_owned();
    let path = dir.join(format!("{stem} {stamp}.pages"));
    std::fs::copy(pages, &path)?;
    // Copies keep the original's modification time, so their age is their creation time.
    let mut backups: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&dir)?
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.created().ok()?, e.path())))
        .collect();
    backups.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (_, old) in backups.into_iter().skip(KEEP_BACKUPS) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// `X (label).ext` next to `file`, numbered if that name is taken.
pub(crate) fn free_name(file: &Path, label: &str) -> PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    std::iter::once(format!("{stem} ({label}).{ext}"))
        .chain((2..).map(|n| format!("{stem} ({label} {n}).{ext}")))
        .map(|name| file.with_file_name(name))
        .find(|candidate| !candidate.exists())
        .expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_names_are_numbered() {
        let dir = std::env::temp_dir().join(format!("pages-bridge-free-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Doc.pages");
        assert_eq!(free_name(&file, "bearbeitet"), dir.join("Doc (bearbeitet).pages"));
        std::fs::write(dir.join("Doc (bearbeitet).pages"), b"").unwrap();
        assert_eq!(free_name(&file, "bearbeitet"), dir.join("Doc (bearbeitet 2).pages"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
