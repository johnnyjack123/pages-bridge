use std::collections::BTreeMap;
use std::fmt;

/// A protobuf message that is not well-formed on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid protobuf data: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

/// One field value as it appears on the wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Raw<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

/// Reads a base-128 varint at `*pos` and advances past it.
pub fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64, DecodeError> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *buf.get(*pos).ok_or(DecodeError("truncated varint"))?;
        *pos += 1;
        if shift < 64 {
            result |= u64::from(byte & 0x7F) << shift;
        }
        if byte < 0x80 {
            return Ok(result);
        }
        shift += 7;
        if shift > 63 + 7 {
            return Err(DecodeError("varint longer than 10 bytes"));
        }
    }
}

/// A protobuf message decoded without a schema.
///
/// The wire format cannot tell a string from a nested message or a packed array, so values are
/// kept raw and interpreted by the typed accessors. Singular accessors return the last occurrence,
/// as protobuf specifies.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Msg<'a> {
    fields: BTreeMap<u32, Vec<Raw<'a>>>,
}

impl<'a> Msg<'a> {
    pub fn parse(buf: &'a [u8]) -> Result<Self, DecodeError> {
        let mut fields: BTreeMap<u32, Vec<Raw<'a>>> = BTreeMap::new();
        let mut pos = 0;
        while pos < buf.len() {
            let key = read_varint(buf, &mut pos)?;
            let number = u32::try_from(key >> 3).map_err(|_| DecodeError("field number too large"))?;
            let raw = match key & 7 {
                0 => Raw::Varint(read_varint(buf, &mut pos)?),
                1 => Raw::Fixed64(u64::from_le_bytes(take::<8>(buf, &mut pos)?)),
                2 => {
                    let len = usize::try_from(read_varint(buf, &mut pos)?)
                        .map_err(|_| DecodeError("length too large"))?;
                    let end = pos.checked_add(len).ok_or(DecodeError("length too large"))?;
                    let bytes = buf.get(pos..end).ok_or(DecodeError("truncated field"))?;
                    pos = end;
                    Raw::Bytes(bytes)
                }
                5 => Raw::Fixed32(u32::from_le_bytes(take::<4>(buf, &mut pos)?)),
                _ => return Err(DecodeError("unsupported wire type")),
            };
            fields.entry(number).or_default().push(raw);
        }
        Ok(Self { fields })
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn has(&self, n: u32) -> bool {
        self.fields.contains_key(&n)
    }

    pub fn field_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.fields.keys().copied()
    }

    pub fn raw(&self, n: u32) -> &[Raw<'a>] {
        self.fields.get(&n).map_or(&[], Vec::as_slice)
    }

    pub fn uint(&self, n: u32) -> Option<u64> {
        self.raw(n).iter().rev().find_map(|r| match *r {
            Raw::Varint(v) | Raw::Fixed64(v) => Some(v),
            Raw::Fixed32(v) => Some(u64::from(v)),
            Raw::Bytes(_) => None,
        })
    }

    pub fn bool(&self, n: u32) -> Option<bool> {
        self.uint(n).map(|v| v != 0)
    }

    pub fn float(&self, n: u32) -> Option<f64> {
        self.raw(n).iter().rev().find_map(|r| match *r {
            Raw::Fixed32(v) => Some(f64::from(f32::from_bits(v))),
            Raw::Fixed64(v) => Some(f64::from_bits(v)),
            #[allow(clippy::cast_precision_loss)]
            Raw::Varint(v) => Some(v as f64),
            Raw::Bytes(_) => None,
        })
    }

    pub fn bytes(&self, n: u32) -> Option<&'a [u8]> {
        self.raw(n).iter().rev().find_map(|r| match *r {
            Raw::Bytes(b) => Some(b),
            _ => None,
        })
    }

    pub fn string(&self, n: u32) -> Option<String> {
        self.bytes(n).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn strings(&self, n: u32) -> Vec<String> {
        self.raw(n)
            .iter()
            .filter_map(|r| match *r {
                Raw::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect()
    }

    /// The nested message in field `n`; repeated occurrences are merged as protobuf does.
    pub fn msg(&self, n: u32) -> Option<Msg<'a>> {
        let mut merged: Option<Msg<'a>> = None;
        for raw in self.raw(n) {
            if let Raw::Bytes(b) = *raw {
                let part = Msg::parse(b).ok()?;
                match merged.as_mut() {
                    None => merged = Some(part),
                    Some(m) => {
                        for (k, v) in part.fields {
                            m.fields.entry(k).or_default().extend(v);
                        }
                    }
                }
            }
        }
        merged
    }

    pub fn msgs(&self, n: u32) -> Vec<Msg<'a>> {
        self.raw(n)
            .iter()
            .filter_map(|r| match *r {
                Raw::Bytes(b) => Msg::parse(b).ok(),
                _ => None,
            })
            .collect()
    }

    /// Repeated integers, packed or not.
    pub fn uints(&self, n: u32) -> Vec<u64> {
        let mut out = Vec::new();
        for raw in self.raw(n) {
            match *raw {
                Raw::Varint(v) | Raw::Fixed64(v) => out.push(v),
                Raw::Fixed32(v) => out.push(u64::from(v)),
                Raw::Bytes(b) => {
                    let mut pos = 0;
                    while pos < b.len() {
                        match read_varint(b, &mut pos) {
                            Ok(v) => out.push(v),
                            Err(_) => break,
                        }
                    }
                }
            }
        }
        out
    }

    /// Repeated 32-bit floats, packed or not.
    pub fn floats(&self, n: u32) -> Vec<f64> {
        let mut out = Vec::new();
        for raw in self.raw(n) {
            match *raw {
                Raw::Fixed32(v) => out.push(f64::from(f32::from_bits(v))),
                Raw::Fixed64(v) => out.push(f64::from_bits(v)),
                Raw::Bytes(b) if b.len() % 4 == 0 => out.extend(
                    b.chunks_exact(4)
                        .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]]))),
                ),
                _ => {}
            }
        }
        out
    }

    /// Follows a `TSP.Reference`: a nested message whose field 1 is an object id.
    pub fn reference(&self, n: u32) -> Option<u64> {
        self.msg(n).and_then(|m| m.uint(1))
    }

    pub fn references(&self, n: u32) -> Vec<u64> {
        self.msgs(n).iter().filter_map(|m| m.uint(1)).collect()
    }

    /// Replaces every field `other` has, keeping the rest: how a style overrides its parent.
    pub fn overlay(&mut self, other: &Msg<'a>) {
        for (k, v) in &other.fields {
            self.fields.insert(*k, v.clone());
        }
    }
}

fn take<const N: usize>(buf: &[u8], pos: &mut usize) -> Result<[u8; N], DecodeError> {
    let end = pos.checked_add(N).ok_or(DecodeError("truncated field"))?;
    let bytes = buf.get(*pos..end).ok_or(DecodeError("truncated field"))?;
    *pos = end;
    let mut out = [0u8; N];
    out.copy_from_slice(bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_scalars_strings_and_nested_messages() {
        // field 1 = 150, field 2 = "hi", field 3 = { field 1 = 7 }, field 4 = 1.5f32
        let buf = [0x08, 0x96, 0x01, 0x12, 0x02, b'h', b'i', 0x1A, 0x02, 0x08, 0x07, 0x25, 0, 0, 0xC0, 0x3F];
        let m = Msg::parse(&buf).unwrap();
        assert_eq!(m.uint(1), Some(150));
        assert_eq!(m.string(2).as_deref(), Some("hi"));
        assert_eq!(m.reference(3), Some(7));
        assert_eq!(m.float(4), Some(1.5));
    }

    #[test]
    fn reads_packed_and_unpacked_repeated_ints() {
        let packed = [0x0A, 0x03, 0x01, 0x02, 0x03];
        assert_eq!(Msg::parse(&packed).unwrap().uints(1), vec![1, 2, 3]);
        let unpacked = [0x08, 0x01, 0x08, 0x02];
        assert_eq!(Msg::parse(&unpacked).unwrap().uints(1), vec![1, 2]);
    }

    #[test]
    fn merges_repeated_singular_messages() {
        // field 1 = {1: 5}, field 1 = {2: 6}
        let buf = [0x0A, 0x02, 0x08, 0x05, 0x0A, 0x02, 0x10, 0x06];
        let m = Msg::parse(&buf).unwrap().msg(1).unwrap();
        assert_eq!((m.uint(1), m.uint(2)), (Some(5), Some(6)));
    }

    #[test]
    fn rejects_truncated_data() {
        assert!(Msg::parse(&[0x12, 0x05, b'a']).is_err());
        assert!(Msg::parse(&[0x08, 0x80]).is_err());
    }

    #[test]
    fn overlay_replaces_whole_fields() {
        let base = [0x08, 0x01, 0x10, 0x02, 0x10, 0x03];
        let top = [0x10, 0x09];
        let mut m = Msg::parse(&base).unwrap();
        m.overlay(&Msg::parse(&top).unwrap());
        assert_eq!(m.uint(1), Some(1));
        assert_eq!(m.uints(2), vec![9]);
    }
}
