use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use crate::archive::{decompress_iwa, read_objects};
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NotIwork => f.write_str("not an iWork document"),
            Error::LegacyFormat => f.write_str("iWork '09 document (not supported)"),
            Error::Corrupt(why) => write!(f, "damaged iWork document: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// One archived object: its message type and protobuf payload.
#[derive(Debug, Clone)]
pub struct Object {
    pub kind: u32,
    pub data: Vec<u8>,
}

/// The decoded objects and embedded files of one iWork document.
pub struct Package {
    container: Container,
    objects: HashMap<u64, Object>,
    data_map: HashMap<u64, String>,
}

impl Package {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let container = Container::open(path)?;
        let names = container.names();
        let mut iwa: Vec<(String, Vec<u8>)> = Vec::new();
        for name in names.iter().filter(|n| n.starts_with("Index/") && n.ends_with(".iwa")) {
            iwa.push((name.clone(), container.read(name)?));
        }
        if iwa.is_empty() && names.iter().any(|n| n == "Index.zip") {
            iwa = read_nested_index(container.read("Index.zip")?)?;
        }
        if iwa.is_empty() {
            let legacy = names.iter().any(|n| n.eq_ignore_ascii_case("index.xml") || n.eq_ignore_ascii_case("index.xml.gz"));
            return Err(if legacy { Error::LegacyFormat } else { Error::NotIwork });
        }
        // Document.iwa first, so an id that appears twice resolves to the main document's copy.
        iwa.sort_by_key(|(n, _)| (!n.ends_with("Document.iwa"), n.clone()));
        let mut objects = HashMap::new();
        for (_, data) in &iwa {
            read_objects(&decompress_iwa(data)?, &mut objects)?;
        }
        let mut package = Self { container, objects, data_map: HashMap::new() };
        package.data_map = package.build_data_map();
        Ok(package)
    }

    fn build_data_map(&self) -> HashMap<u64, String> {
        let files: HashMap<&str, &String> = self
            .container
            .names()
            .iter()
            .filter_map(|n| n.strip_prefix("Data/").map(|f| (f, n)))
            .collect();
        let mut map = HashMap::new();
        for id in self.ids_of_kind(PACKAGE_METADATA) {
            let Some(meta) = self.object(id) else { continue };
            for entry in meta.msgs(4) {
                let Some(data_id) = entry.uint(1) else { continue };
                // The file name sits in one of the string fields; which one varies by version.
                let file = entry.field_numbers().find_map(|n| entry.string(n).and_then(|s| files.get(s.as_str()).copied()));
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
}
