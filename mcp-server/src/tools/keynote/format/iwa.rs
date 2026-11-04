//! IWA files: Snappy-compressed streams of archived objects.
//!
//! The file is a sequence of chunks, each a 0x00 byte, a 24-bit little-endian length and a raw
//! (unframed) Snappy block. Decompressed chunks concatenate into a stream of objects, each a
//! varint-prefixed TSP.ArchiveInfo header followed by the payloads its message infos describe.

use anyhow::{Result, anyhow, bail};

use super::proto::{Message, read_varint, write_varint};

////////////////////////////////////////////////////////////////////////////////
mod archive_info {
    pub const IDENTIFIER: u32 = 1;
    pub const MESSAGE_INFOS: u32 = 2;
}

////////////////////////////////////////////////////////////////////////////////
mod message_info {
    pub const VERSION: u32 = 2;
    pub const TYPE: u32 = 1;
    pub const LENGTH: u32 = 3;
    pub const FIELD_INFOS: u32 = 4;
    pub const OBJECT_REFERENCES: u32 = 5;
    pub const DATA_REFERENCES: u32 = 6;
}

////////////////////////////////////////////////////////////////////////////////
mod field_info {
    pub const OBJECT_REFERENCES: u32 = 4;
}

const CHUNK_SIZE: usize = 65536;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone)]
pub struct IwaObject {
    /// TSP.ArchiveInfo
    info: Message,
    /// TSP.MessageInfo of every payload
    message_infos: Vec<Message>,
    /// the object itself first, followed by diff messages when present
    payloads: Vec<Vec<u8>>,
}

impl IwaObject {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn identifier(&self) -> u64 {
        self.info.varint(archive_info::IDENTIFIER).unwrap_or_default()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn message_type(&self) -> u32 {
        let info = self.message_infos.first();
        info.and_then(|e| e.varint(message_info::TYPE)).unwrap_or_default() as u32
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Every object this one references, across all its payloads
    pub fn object_references(&self) -> Vec<u64> {
        let references = self.message_infos.iter();
        let references = references.flat_map(|e| e.varints(message_info::OBJECT_REFERENCES).unwrap_or_default());
        references.collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Objects the object message itself references, without its diff messages
    pub fn message_references(&self) -> Vec<u64> {
        let info = self.message_infos.first();
        info.and_then(|e| e.varints(message_info::OBJECT_REFERENCES).ok())
            .unwrap_or_default()
    }

    /// Raw payloads: the object message first, then diff messages when present
    #[cfg(test)]
    pub fn payloads(&self) -> &[Vec<u8>] {
        &self.payloads
    }

    ////////////////////////////////////////////////////////////////////////////////
    ////////////////////////////////////////////////////////////////////////////////
    pub fn message(&self) -> Result<Message> {
        let payload = self
            .payloads
            .first()
            .ok_or_else(|| anyhow!("object {} is empty", self.identifier()))?;
        Message::decode(payload)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Replaces the object message, `references` becomes its complete object reference list
    pub fn set_message(&mut self, message: &Message, references: &[u64]) -> Result<()> {
        if self.payloads.is_empty() {
            bail!("object {} is empty", self.identifier());
        }

        let payload = message.encode();
        let info = &mut self.message_infos[0];
        info.set_varint(message_info::LENGTH, payload.len() as u64);
        info.set_packed_varints(message_info::OBJECT_REFERENCES, references);
        self.payloads[0] = payload;
        self.store_message_infos();
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Copy of this object under a new identifier, with references remapped by `map`.
    ///
    /// Headers don't list every reference (a build's drawable or a build chunk's build are
    /// left out), so any reference to a mapped id is rewritten unless the header says the id
    /// is a data (media) reference, which lives in its own id space.
    pub fn remapped(&self, identifier: u64, map: &dyn Fn(u64) -> Option<u64>) -> Result<Self> {
        use super::proto::{ReferenceAction, rewrite_references};

        let mut copy = self.clone();
        copy.info.set_varint(archive_info::IDENTIFIER, identifier);

        for (info, payload) in copy.message_infos.iter_mut().zip(copy.payloads.iter_mut()) {
            let references = info.varints(message_info::OBJECT_REFERENCES)?;
            let data = info.varints(message_info::DATA_REFERENCES)?;
            let mut message = Message::decode(payload)?;
            let changed = rewrite_references(&mut message, &mut |e| match map(e) {
                Some(mapped) if references.contains(&e) || !data.contains(&e) => ReferenceAction::Replace(mapped),
                _ => ReferenceAction::Keep,
            });

            if changed {
                *payload = message.encode();
                info.set_varint(message_info::LENGTH, payload.len() as u64);
            }

            let references: Vec<_> = references.iter().map(|e| map(*e).unwrap_or(*e)).collect();
            info.set_packed_varints(message_info::OBJECT_REFERENCES, &references);

            let mut field_infos = info.messages(message_info::FIELD_INFOS)?;
            for field_info in &mut field_infos {
                let references = field_info.varints(field_info::OBJECT_REFERENCES)?;
                let references: Vec<_> = references.iter().map(|e| map(*e).unwrap_or(*e)).collect();
                field_info.set_packed_varints(field_info::OBJECT_REFERENCES, &references);
            }
            if !field_infos.is_empty() {
                info.set_messages(message_info::FIELD_INFOS, &field_infos);
            }
        }

        copy.store_message_infos();
        Ok(copy)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Drops references to `removed` ids from every payload, returns whether anything changed
    pub fn strip_references(&mut self, removed: &dyn Fn(u64) -> bool) -> Result<bool> {
        use super::proto::{ReferenceAction, rewrite_references};

        let mut changed = false;
        for (info, payload) in self.message_infos.iter_mut().zip(self.payloads.iter_mut()) {
            let references = info.varints(message_info::OBJECT_REFERENCES)?;
            if !references.iter().any(|e| removed(*e)) {
                continue;
            }

            let mut message = Message::decode(payload)?;
            rewrite_references(&mut message, &mut |e| match removed(e) {
                true => ReferenceAction::Remove,
                false => ReferenceAction::Keep,
            });
            *payload = message.encode();
            info.set_varint(message_info::LENGTH, payload.len() as u64);

            let references: Vec<_> = references.into_iter().filter(|e| !removed(*e)).collect();
            info.set_packed_varints(message_info::OBJECT_REFERENCES, &references);

            let mut field_infos = info.messages(message_info::FIELD_INFOS)?;
            for field_info in &mut field_infos {
                let references = field_info.varints(field_info::OBJECT_REFERENCES)?;
                let references: Vec<_> = references.into_iter().filter(|e| !removed(*e)).collect();
                field_info.set_packed_varints(field_info::OBJECT_REFERENCES, &references);
            }
            if !field_infos.is_empty() {
                info.set_messages(message_info::FIELD_INFOS, &field_infos);
            }
            changed = true;
        }

        self.store_message_infos();
        Ok(changed)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn store_message_infos(&mut self) {
        self.info.set_messages(archive_info::MESSAGE_INFOS, &self.message_infos);
    }
}

////////////////////////////////////////////////////////////////////////////////
pub fn decode_iwa(bytes: &[u8]) -> Result<Vec<IwaObject>> {
    let stream = decompress(bytes)?;
    let mut stream = stream.as_slice();

    let mut objects = Vec::new();
    while !stream.is_empty() {
        let length = usize::try_from(read_varint(&mut stream)?)?;
        let info = Message::decode(split(&mut stream, length)?)?;
        let message_infos = info.messages(archive_info::MESSAGE_INFOS)?;

        let mut payloads = Vec::new();
        for message_info in &message_infos {
            let length = message_info
                .varint(message_info::LENGTH)
                .ok_or_else(|| anyhow!("message length is missing"))?;
            payloads.push(split(&mut stream, usize::try_from(length)?)?.to_vec());
        }

        objects.push(IwaObject {
            info,
            message_infos,
            payloads,
        });
    }
    Ok(objects)
}

////////////////////////////////////////////////////////////////////////////////
pub fn encode_iwa(objects: &[IwaObject]) -> Result<Vec<u8>> {
    let mut stream = Vec::new();
    for object in objects {
        let info = object.info.encode();
        write_varint(&mut stream, info.len() as u64);
        stream.extend_from_slice(&info);
        for payload in &object.payloads {
            stream.extend_from_slice(payload);
        }
    }
    compress(&stream)
}

////////////////////////////////////////////////////////////////////////////////
/// The object stream of an IWA file
pub fn decompress(mut bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = snap::raw::Decoder::new();
    let mut stream = Vec::new();
    while !bytes.is_empty() {
        let header = split(&mut bytes, 4)?;
        if header[0] != 0 {
            bail!("IWA chunk does not start with 0x00 (found {:#x})", header[0]);
        }
        let length = u32::from_le_bytes([header[1], header[2], header[3], 0]) as usize;
        let chunk = split(&mut bytes, length)?;
        let chunk = decoder
            .decompress_vec(chunk)
            .map_err(|e| anyhow!("snappy decompression failed: {e}"))?;
        stream.extend_from_slice(&chunk);
    }
    Ok(stream)
}

////////////////////////////////////////////////////////////////////////////////
fn compress(stream: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = snap::raw::Encoder::new();
    let mut bytes = Vec::new();
    for chunk in stream.chunks(CHUNK_SIZE) {
        let chunk = encoder
            .compress_vec(chunk)
            .map_err(|e| anyhow!("snappy compression failed: {e}"))?;
        let length = u32::try_from(chunk.len())?;
        bytes.push(0);
        bytes.extend_from_slice(&length.to_le_bytes()[..3]);
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

////////////////////////////////////////////////////////////////////////////////
fn split<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    if bytes.len() < length {
        bail!("truncated IWA stream, expected {length} bytes, got {}", bytes.len());
    }
    let (head, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(head)
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
pub(super) fn test_object(identifier: u64, message_type: u32, message: &Message, references: &[u64]) -> IwaObject {
    IwaObject::new(identifier, message_type, &[], message, references)
}

impl IwaObject {
    ////////////////////////////////////////////////////////////////////////////////
    /// A new object; `version` is the archive version Keynote expects for its type
    pub fn new(identifier: u64, message_type: u32, version: &[u64], message: &Message, references: &[u64]) -> Self {
        let payload = message.encode();
        let mut info = Message::default();
        info.set_varint(message_info::TYPE, u64::from(message_type));
        info.set_packed_varints(message_info::VERSION, version);
        info.set_varint(message_info::LENGTH, payload.len() as u64);
        info.set_packed_varints(message_info::OBJECT_REFERENCES, references);

        let mut archive = Message::default();
        archive.set_varint(archive_info::IDENTIFIER, identifier);
        archive.push_message(archive_info::MESSAGE_INFOS, &info);

        Self {
            info: archive,
            message_infos: vec![info],
            payloads: vec![payload],
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Data (media) ids the object message uses, added to its header
    pub fn add_data_references(&mut self, data: &[u64]) -> Result<()> {
        let Some(info) = self.message_infos.first_mut() else {
            bail!("object {} is empty", self.identifier());
        };
        let mut references = info.varints(message_info::DATA_REFERENCES)?;
        for e in data {
            if !references.contains(e) {
                references.push(*e);
            }
        }
        info.set_packed_varints(message_info::DATA_REFERENCES, &references);
        self.store_message_infos();
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Data (media) ids the object uses
    #[cfg(test)]
    pub fn data_references(&self) -> Vec<u64> {
        let info = self.message_infos.first();
        info.and_then(|e| e.varints(message_info::DATA_REFERENCES).ok())
            .unwrap_or_default()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Archive version of the object message
    pub fn version(&self) -> Vec<u64> {
        let info = self.message_infos.first();
        info.and_then(|e| e.varints(message_info::VERSION).ok())
            .unwrap_or_default()
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::keynote::format::proto::{make_reference, reference};

    #[test]
    fn round_trips_objects_across_chunks() {
        let mut big = Message::default();
        big.set_bytes(1, vec![7; CHUNK_SIZE * 2 + 10]);

        let mut small = Message::default();
        small.push_message(2, &make_reference(1));

        let objects = vec![test_object(1, 2001, &big, &[]), test_object(2, 5, &small, &[1])];
        let decoded = decode_iwa(&encode_iwa(&objects).unwrap()).unwrap();

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].message().unwrap(), big);
        assert_eq!(decoded[1].identifier(), 2);
        assert_eq!(decoded[1].message_type(), 5);
        assert_eq!(decoded[1].object_references(), vec![1]);
    }

    #[test]
    fn remaps_references_in_payload_and_header() {
        let mut message = Message::default();
        message.push_message(2, &make_reference(1));
        let object = test_object(2, 5, &message, &[1]);

        let copy = object.remapped(12, &|e| (e == 1).then_some(11)).unwrap();
        assert_eq!(copy.identifier(), 12);
        assert_eq!(copy.object_references(), vec![11]);
        assert_eq!(reference(&copy.message().unwrap(), 2).unwrap(), Some(11));

        let decoded = decode_iwa(&encode_iwa(&[copy]).unwrap()).unwrap();
        assert_eq!(reference(&decoded[0].message().unwrap(), 2).unwrap(), Some(11));
    }
}
