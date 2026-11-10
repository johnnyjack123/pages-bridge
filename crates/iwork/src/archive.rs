use std::collections::HashMap;

use crate::package::{Error, Object};
use crate::proto::{Msg, read_varint};

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

/// Adds the objects of one decompressed archive stream to `objects`.
///
/// The stream is a sequence of `ArchiveInfo` messages (field 1: object id, field 2: message
/// infos with type in field 1 and payload length in field 3), each followed by its payloads.
/// Objects already present are kept, so the caller decides precedence by reading order.
pub(crate) fn read_objects(stream: &[u8], objects: &mut HashMap<u64, Object>) -> Result<(), Error> {
    let corrupt = |e: crate::DecodeError| Error::Corrupt(e.to_string());
    let mut pos = 0;
    while pos < stream.len() {
        let len = usize::try_from(read_varint(stream, &mut pos).map_err(corrupt)?)
            .map_err(|_| Error::Corrupt("archive info too large".into()))?;
        let info_bytes = stream
            .get(pos..pos + len)
            .ok_or_else(|| Error::Corrupt("truncated archive info".into()))?;
        pos += len;
        let info = Msg::parse(info_bytes).map_err(corrupt)?;
        let id = info.uint(1);
        for (i, message) in info.msgs(2).iter().enumerate() {
            let len = usize::try_from(message.uint(3).unwrap_or(0))
                .map_err(|_| Error::Corrupt("message too large".into()))?;
            let payload = stream
                .get(pos..pos + len)
                .ok_or_else(|| Error::Corrupt("truncated message".into()))?;
            pos += len;
            // Further message infos of one archive repeat the object for older readers.
            if i == 0 {
                if let Some(id) = id {
                    let kind = u32::try_from(message.uint(1).unwrap_or(0)).unwrap_or(0);
                    objects.entry(id).or_insert_with(|| Object { kind, data: payload.to_vec() });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal_snappy(data: &[u8]) -> Vec<u8> {
        snap::raw::Encoder::new().compress_vec(data).unwrap()
    }

    #[test]
    fn reads_objects_from_chunked_stream() {
        // ArchiveInfo {1: 42, 2: {1: 2001, 3: 2}} followed by the payload {1: 7}.
        let info = [0x08, 42, 0x12, 0x05, 0x08, 0xD1, 0x0F, 0x18, 0x02];
        let mut stream = vec![u8::try_from(info.len()).unwrap()];
        stream.extend_from_slice(&info);
        stream.extend_from_slice(&[0x08, 0x07]);
        let compressed = literal_snappy(&stream);
        let mut iwa = vec![0u8];
        iwa.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes()[..3]);
        iwa.extend_from_slice(&compressed);

        let mut objects = HashMap::new();
        read_objects(&decompress_iwa(&iwa).unwrap(), &mut objects).unwrap();
        let obj = &objects[&42];
        assert_eq!(obj.kind, 2001);
        assert_eq!(Msg::parse(&obj.data).unwrap().uint(1), Some(7));
    }
}
