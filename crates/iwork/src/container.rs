use std::cell::RefCell;
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};

use crate::package::Error;

enum Source {
    Zip(RefCell<zip::ZipArchive<BufReader<File>>>),
    Dir(PathBuf),
}

/// Read access to a zipped iWork file or a bundle folder, with paths relative to the document.
pub(crate) struct Container {
    source: Source,
    /// Some zips carry the bundle folder as a top-level directory (`Doc.pages/Index/...`).
    prefix: String,
    names: Vec<String>,
}

impl Container {
    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        let (source, all) = if path.is_dir() {
            let mut names = Vec::new();
            collect_files(path, path, &mut names)?;
            (Source::Dir(path.to_path_buf()), names)
        } else {
            let archive = zip::ZipArchive::new(BufReader::new(File::open(path)?))
                .map_err(|_| Error::NotIwork)?;
            let names = archive.file_names().filter(|n| !n.ends_with('/')).map(str::to_owned).collect();
            (Source::Zip(RefCell::new(archive)), names)
        };
        let prefix = all
            .iter()
            .find_map(|n| {
                ["Index/Document.iwa", "Index.zip", "index.xml", "index.xml.gz"]
                    .iter()
                    .find_map(|marker| n.strip_suffix(marker))
                    .filter(|p| p.is_empty() || p.trim_end_matches('/').split('/').count() == 1)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let names = all.iter().filter_map(|n| n.strip_prefix(&prefix).map(str::to_owned)).collect();
        Ok(Self { source, prefix, names })
    }

    pub(crate) fn names(&self) -> &[String] {
        &self.names
    }

    pub(crate) fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        let full = format!("{}{name}", self.prefix);
        match &self.source {
            Source::Zip(archive) => {
                let mut archive = archive.borrow_mut();
                let mut entry = archive.by_name(&full).map_err(|e| Error::Corrupt(e.to_string()))?;
                let mut out = Vec::new();
                entry.read_to_end(&mut out)?;
                Ok(out)
            }
            Source::Dir(dir) => Ok(std::fs::read(dir.join(full))?),
        }
    }
}

/// Reads every `.iwa` entry of a nested `Index.zip` (Pages 5.0–5.2 bundles).
pub(crate) fn read_nested_index(zip_bytes: Vec<u8>) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|_| Error::NotIwork)?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| Error::Corrupt(e.to_string()))?;
        if entry.name().ends_with(".iwa") {
            let name = entry.name().to_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            out.push((name, data));
        }
    }
    Ok(out)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), Error> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}
