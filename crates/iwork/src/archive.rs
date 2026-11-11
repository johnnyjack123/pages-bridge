use std::collections::{BTreeMap, HashMap};

use crate::package::{Error, Object, ObjectEdit};
use crate::proto::{Encoder, Msg, read_varint, write_varint};

/// Largest uncompressed chunk; Pages' own files never exceed it.
const MAX_CHUNK: usize = 65_536;

/// Joins the Snappy chunks of an `.iwa` file into the archive stream.
///
/// Each chunk is a zero byte, a 24-bit little-endian length and a raw Snappy block. Unlike the
/// Snappy framing format there are no stream identifier and no checksums.
pub fn decompress_iwa(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut decoder = snap::raw::Decoder::new();
    let mut pos = 0;
    while pos < data.len() {
        let header = data
            .get(pos..pos + 4)
            .ok_or_else(|| Error::Corrupt("truncated IWA chunk header".into()))?;
        if header[0] != 0 {
            return Err(Error::Corrupt(format!("unexpected IWA chunk type {}", header[0])));
        }
        let len = usize::from(header[1]) | usize::from(header[2]) << 8 | usize::from(header[3]) << 16;
        let chunk = data
            .get(pos + 4..pos + 4 + len)
            .ok_or_else(|| Error::Corrupt("truncated IWA chunk".into()))?;
        out.extend(decoder.decompress_vec(chunk).map_err(|e| Error::Corrupt(e.to_string()))?);
        pos += 4 + len;
    }
    Ok(out)
}

/// One archive of the stream: where it starts and ends, and the object it holds.
struct Segment<'a> {
    start: usize,
    end: usize,
    info: Msg<'a>,
    payload_start: usize,
}

fn segments(stream: &[u8]) -> Result<Vec<Segment<'_>>, Error> {
    let corrupt = |e: crate::DecodeError| Error::Corrupt(e.to_string());
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < stream.len() {
        let start = pos;
        let len = usize::try_from(read_varint(stream, &mut pos).map_err(corrupt)?)
            .map_err(|_| Error::Corrupt("archive info too large".into()))?;
        let info_bytes = stream
            .get(pos..pos + len)
            .ok_or_else(|| Error::Corrupt("truncated archive info".into()))?;
        pos += len;
        let info = Msg::parse(info_bytes).map_err(corrupt)?;
        let payload_start = pos;
        for message in info.msgs(2) {
            pos += usize::try_from(message.uint(3).unwrap_or(0)).map_err(|_| Error::Corrupt("message too large".into()))?;
        }
        if pos > stream.len() {
            return Err(Error::Corrupt("truncated message".into()));
        }
        out.push(Segment { start, end: pos, info, payload_start });
    }
    Ok(out)
}

/// Adds the objects of one decompressed archive stream to `objects`.
///
/// The stream is a sequence of `ArchiveInfo` messages (field 1: object id, field 2: message
/// infos with type in field 1, payload length in field 3 and the ids of all referenced objects
/// in field 5), each followed by its payloads. Objects already present are kept, so the caller
/// decides precedence by reading order.
pub(crate) fn read_objects(stream: &[u8], file: &str, objects: &mut HashMap<u64, Object>) -> Result<(), Error> {
    for segment in segments(stream)? {
        let Some(id) = segment.info.uint(1) else { continue };
        // Further message infos of one archive repeat the object for older readers.
        let Some(message) = segment.info.msgs(2).into_iter().next() else { continue };
        let len = usize::try_from(message.uint(3).unwrap_or(0)).unwrap_or(0);
        let payload = &stream[segment.payload_start..segment.payload_start + len];
        let kind = u32::try_from(message.uint(1).unwrap_or(0)).unwrap_or(0);
        objects.entry(id).or_insert_with(|| Object {
            kind,
            data: payload.to_vec(),
            file: file.to_owned(),
            references: message.uints(5),
        });
    }
    Ok(())
}

/// Re-encodes an `.iwa` file with some objects replaced; every other archive keeps its bytes.
pub(crate) fn rewrite_iwa(data: &[u8], edits: &BTreeMap<u64, &ObjectEdit>) -> Result<Vec<u8>, Error> {
    let stream = decompress_iwa(data)?;
    let mut out = Vec::with_capacity(stream.len() + 1024);
    let mut boundaries = Vec::new();
    for segment in segments(&stream)? {
        match segment.info.uint(1).and_then(|id| edits.get(&id)) {
            Some(edit) => {
                let info = rebuild_info(&segment.info, edit);
                write_varint(&mut out, info.len() as u64);
                out.extend_from_slice(&info);
                out.extend_from_slice(&edit.payload);
            }
            None => out.extend_from_slice(&stream[segment.start..segment.end]),
        }
        boundaries.push(out.len());
    }
    Ok(compress_iwa(&out, &boundaries))
}

/// The archive header for a replaced payload: new length and references, everything else as
/// before. Compatibility copies (further message infos) are dropped, since they would no longer
/// match the new payload.
fn rebuild_info(info: &Msg, edit: &ObjectEdit) -> Vec<u8> {
    let mut e = Encoder::new();
    e.copy_except(info, &[2]);
    if let Some(first) = info.msgs(2).first() {
        let mut m = Encoder::new();
        m.copy_except(first, &[3, 5]);
        m.uint(3, edit.payload.len() as u64);
        if !edit.references.is_empty() {
            m.packed_uints(5, &edit.references);
        }
        e.bytes(2, &m.finish());
    }
    e.finish()
}

/// Splits the stream into Snappy chunks of at most 64 KiB, cutting at archive boundaries
/// where one is in reach.
pub(crate) fn compress_iwa(stream: &[u8], boundaries: &[usize]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stream.len() / 2 + 16);
    let mut encoder = snap::raw::Encoder::new();
    let mut start = 0;
    while start < stream.len() {
        let limit = (start + MAX_CHUNK).min(stream.len());
        // Boundaries ascend, so the last one in reach is the first one found from the back.
        let end = boundaries.iter().copied().rev().find(|&b| b > start && b <= limit).unwrap_or(limit);
        let block = encoder.compress_vec(&stream[start..end]).expect("chunks are far below Snappy's size limit");
        out.push(0);
        out.extend_from_slice(&u32::try_from(block.len()).expect("chunk below 16 MiB").to_le_bytes()[..3]);
        out.extend_from_slice(&block);
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(id: u64, kind: u64, payload: &[u8], refs: &[u64]) -> Vec<u8> {
        let mut info = Encoder::new();
        let mut message = Encoder::new();
        message.uint(1, kind).packed_uints(2, &[1, 0, 5]).uint(3, payload.len() as u64);
        if !refs.is_empty() {
            message.packed_uints(5, refs);
        }
        info.uint(1, id).bytes(2, &message.finish());
        let info = info.finish();
        let mut out = Vec::new();
        write_varint(&mut out, info.len() as u64);
        out.extend(info);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn reads_objects_and_references() {
        let stream = archive(42, 2001, &[0x08, 0x07], &[5, 6]);
        let iwa = compress_iwa(&stream, &[stream.len()]);
        let mut objects = HashMap::new();
        read_objects(&decompress_iwa(&iwa).unwrap(), "Index/Document.iwa", &mut objects).unwrap();
        let obj = &objects[&42];
        assert_eq!((obj.kind, obj.references.as_slice(), obj.file.as_str()), (2001, &[5, 6][..], "Index/Document.iwa"));
        assert_eq!(Msg::parse(&obj.data).unwrap().uint(1), Some(7));
    }

    #[test]
    fn rewrite_replaces_one_object_and_keeps_the_rest() {
        let mut stream = archive(1, 10, &[0x08, 0x01], &[]);
        stream.extend(archive(2, 20, &[0x08, 0x02], &[1]));
        let iwa = compress_iwa(&stream, &[]);
        let edit = ObjectEdit { payload: vec![0x08, 0x63, 0x10, 0x01], references: vec![1, 9] };
        let rewritten = rewrite_iwa(&iwa, &BTreeMap::from([(2, &edit)])).unwrap();
        let mut objects = HashMap::new();
        read_objects(&decompress_iwa(&rewritten).unwrap(), "x", &mut objects).unwrap();
        assert_eq!(objects[&1].data, vec![0x08, 0x01]);
        assert_eq!((objects[&2].kind, objects[&2].references.as_slice()), (20, &[1, 9][..]));
        assert_eq!(Msg::parse(&objects[&2].data).unwrap().uint(1), Some(99));
    }

    #[test]
    fn large_streams_are_chunked_at_archive_boundaries() {
        let big = vec![0x0A, 0x80, 0x80, 0x02]; // field 1, 32 KiB of bytes follow
        let mut payload = big.clone();
        payload.extend(vec![b'x'; 32 * 1024]);
        let mut stream = Vec::new();
        let mut bounds = Vec::new();
        for id in 0..3 {
            stream.extend(archive(id, 1, &payload, &[]));
            bounds.push(stream.len());
        }
        let iwa = compress_iwa(&stream, &bounds);
        // Two archives of just over 32 KiB exceed 64 KiB, so each chunk ends after one archive.
        let first_len = usize::from(iwa[1]) | usize::from(iwa[2]) << 8 | usize::from(iwa[3]) << 16;
        let first = snap::raw::Decoder::new().decompress_vec(&iwa[4..4 + first_len]).unwrap();
        assert_eq!(first.len(), bounds[0]);
        assert_eq!(decompress_iwa(&iwa).unwrap(), stream);
    }
}
