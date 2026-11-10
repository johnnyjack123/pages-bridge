//! Reading Apple iWork documents (Pages, Numbers and Keynote since 2013): the ZIP file or bundle
//! folder, the Snappy-compressed IWA archives inside it, and the protobuf objects they hold.
//!
//! Apple publishes no schema for these objects, so [`Msg`] decodes protobuf without one. What a
//! field number means is for the caller to establish from sample documents.

mod archive;
mod container;
mod package;
mod proto;

pub use archive::decompress_iwa;
pub use package::{Error, Object, Package};
pub use proto::{DecodeError, Msg, Raw, read_varint};
