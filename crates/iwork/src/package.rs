use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use crate::archive::{decompress_iwa, read_objects, rewrite_iwa};
use crate::container::{Container, read_nested_index};
use crate::proto::Msg;

/// Message type of `TSP.PackageMetadata`, which maps data ids to files under `Data/`.
const PACKAGE_METADATA: u32 = 11006;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// Not a ZIP file or bundle with IWA archives.
    NotIwork,
    /// An iWork '09 document (XML based), which this crate does not read.
    LegacyFormat,
    Corrupt(String),
    /// Readable, but this crate cannot write it back.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NotIwork => f.write_str("not an iWork document"),
            Error::LegacyFormat => f.write_str("iWork '09 document (not supported)"),
            Error::Corrupt(why) => write!(f, "damaged iWork document: {why}"),
            Error::Unsupported(what) => write!(f, "cannot write {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::Io(e) => Error::Io(e),
            other => Error::Corrupt(other.to_string()),
        }
    }
}

/// One archived object: its message type, protobuf payload, the `.iwa` file it lives in and
/// the ids of the objects it references (from its archive header).
#[derive(Debug, Clone)]
pub struct Object {
    pub kind: u32,
    pub data: Vec<u8>,
    pub file: String,
    pub references: Vec<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectEdit {
    pub(crate) payload: Vec<u8>,
    pub(crate) references: Vec<u64>,
}

/// Changes to write with [`Package::save_as`].
#[derive(Debug, Clone, Default)]
pub struct Edits {
    objects: BTreeMap<u64, ObjectEdit>,
    files: BTreeMap<String, Vec<u8>>,
}

impl Edits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the payload of an existing object. `references` must list every object id the
    /// new payload refers to: iWork loads and keeps objects by these lists.
    pub fn replace_object(&mut self, id: u64, payload: Vec<u8>, references: Vec<u64>) {
        self.objects.insert(id, ObjectEdit { payload, references });
    }

    /// Replaces a file of the package other than an `.iwa` archive, e.g. a preview image.
    pub fn replace_file(&mut self, name: impl Into<String>, data: Vec<u8>) {
        self.files.insert(name.into(), data);
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.files.is_empty()
    }
}

/// The decoded objects and embedded files of one iWork document.
pub struct Package {
    container: Container,
    objects: HashMap<u64, Object>,
    data_map: HashMap<u64, String>,
    /// Pages 5.0–5.2 bundles nest the archives in `Index.zip`; those are read-only here.
    nested_index: bool,
}

impl Package {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let container = Container::open(path)?;
        let names = container.names();
        let mut iwa: Vec<(String, Vec<u8>)> = Vec::new();
        for name in names.iter().filter(|n| n.starts_with("Index/") && n.ends_with(".iwa")) {
            iwa.push((name.clone(), container.read(name)?));
        }
        let mut nested_index = false;
        if iwa.is_empty() && names.iter().any(|n| n == "Index.zip") {
            iwa = read_nested_index(container.read("Index.zip")?)?;
            nested_index = true;
        }
        if iwa.is_empty() {
            let legacy =
                names.iter().any(|n| n.eq_ignore_ascii_case("index.xml") || n.eq_ignore_ascii_case("index.xml.gz"));
            return Err(if legacy { Error::LegacyFormat } else { Error::NotIwork });
        }
        // Document.iwa first, so an id that appears twice resolves to the main document's copy.
        iwa.sort_by_key(|(n, _)| (!n.ends_with("Document.iwa"), n.clone()));
        let mut objects = HashMap::new();
        for (name, data) in &iwa {
            read_objects(&decompress_iwa(data)?, name, &mut objects)?;
        }
        let mut package = Self { container, objects, data_map: HashMap::new(), nested_index };
        package.data_map = package.build_data_map();
        Ok(package)
    }

    fn build_data_map(&self) -> HashMap<u64, String> {
        let files: HashMap<&str, &String> =
            self.container.names().iter().filter_map(|n| n.strip_prefix("Data/").map(|f| (f, n))).collect();
        let mut map = HashMap::new();
        for id in self.ids_of_kind(PACKAGE_METADATA) {
            let Some(meta) = self.object(id) else { continue };
            for entry in meta.msgs(4) {
                let Some(data_id) = entry.uint(1) else { continue };
                // The file name sits in one of the string fields; which one varies by version.
                let file = entry
                    .field_numbers()
                    .find_map(|n| entry.string(n).and_then(|s| files.get(s.as_str()).copied()));
                if let Some(file) = file {
                    map.insert(data_id, file.clone());
                }
            }
        }
        map
    }

    pub fn kind(&self, id: u64) -> Option<u32> {
        self.objects.get(&id).map(|o| o.kind)
    }

    /// The object's payload decoded schema-less; `None` for unknown ids or broken payloads.
    pub fn object(&self, id: u64) -> Option<Msg<'_>> {
        Msg::parse(&self.objects.get(&id)?.data).ok()
    }

    /// Ids of the objects `id` references, as its archive header lists them.
    pub fn references(&self, id: u64) -> &[u64] {
        self.objects.get(&id).map_or(&[], |o| o.references.as_slice())
    }

    /// Whether `from` may reference `target` without the package metadata changing: both sit
    /// in the same archive, or the metadata already lists `target` among the external
    /// references of the component holding `from`. This crate never edits the metadata.
    pub fn can_reference(&self, from: u64, target: u64) -> bool {
        let (Some(a), Some(b)) = (self.objects.get(&from), self.objects.get(&target)) else { return false };
        if a.file == b.file {
            return true;
        }
        self.ids_of_kind(PACKAGE_METADATA).into_iter().filter_map(|id| self.object(id)).any(|meta| {
            meta.msgs(3).iter().any(|component| {
                // Field 3 is the component's locator, 2 the preferred one; both name the archive.
                let holds_from =
                    [component.string(3), component.string(2)].into_iter().flatten().any(|l| a.file == format!("Index/{l}.iwa"));
                holds_from && component.msgs(6).iter().any(|r| r.uint(2) == Some(target))
            })
        })
    }

    pub fn ids_of_kind(&self, kind: u32) -> Vec<u64> {
        let mut ids: Vec<u64> = self.objects.iter().filter(|(_, o)| o.kind == kind).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids
    }

    pub fn has_data(&self, data_id: u64) -> bool {
        self.data_map.contains_key(&data_id)
    }

    /// The bytes of an embedded file (an image, say) by its data id.
    pub fn read_data(&self, data_id: u64) -> Result<Vec<u8>, Error> {
        let name = self.data_map.get(&data_id).ok_or_else(|| Error::Corrupt(format!("no data file {data_id}")))?;
        self.container.read(name)
    }

    pub fn data_file_name(&self, data_id: u64) -> Option<&str> {
        self.data_map.get(&data_id).map(String::as_str)
    }

    /// Writes a copy of the document with `edits` applied to `out`, which must not be the file
    /// this package was opened from (it is still open). Every untouched entry is copied
    /// byte for byte; entries stay uncompressed, as iWork writes them.
    pub fn save_as(&self, out: &Path, edits: &Edits) -> Result<(), Error> {
        let source = self.container.zip_path().ok_or(Error::Unsupported("bundle folders"))?;
        if self.nested_index {
            return Err(Error::Unsupported("Pages 5.0 bundles"));
        }
        let mut by_file: HashMap<&str, BTreeMap<u64, &ObjectEdit>> = HashMap::new();
        for (id, edit) in &edits.objects {
            let object = self.objects.get(id).ok_or_else(|| Error::Corrupt(format!("no object {id} to replace")))?;
            by_file.entry(object.file.as_str()).or_default().insert(*id, edit);
        }

        let mut archive = zip::ZipArchive::new(BufReader::new(File::open(source)?))?;
        let tmp = out.with_extension("pages.tmp");
        let mut writer = zip::ZipWriter::new(File::create(&tmp)?);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let result = (|| -> Result<(), Error> {
            for i in 0..archive.len() {
                let full = archive.name_for_index(i).ok_or_else(|| Error::Corrupt("unnamed entry".into()))?.to_owned();
                let name = full.strip_prefix(&self.container.prefix).unwrap_or(&full);
                let replacement = if let Some(data) = edits.files.get(name) {
                    Some(data.clone())
                } else if let Some(objects) = by_file.get(name) {
                    let mut raw = Vec::new();
                    archive.by_index(i)?.read_to_end(&mut raw)?;
                    Some(rewrite_iwa(&raw, objects)?)
                } else {
                    None
                };
                match replacement {
                    Some(data) => {
                        writer.start_file(full.as_str(), stored)?;
                        writer.write_all(&data)?;
                    }
                    None => writer.raw_copy_file(archive.by_index_raw(i)?)?,
                }
            }
            writer.finish()?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, out)?;
        Ok(())
    }
}
