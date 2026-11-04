//! Schema-less protobuf wire format.
//!
//! iWork schemas are reverse-engineered and change between app versions, so archives are
//! handled as plain field lists: the few fields the tools care about are addressed by number
//! and everything else round-trips untouched.

use anyhow::{Result, anyhow, bail};

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Varint(u64),
    Fixed64(u64),
    Bytes(Vec<u8>),
    Fixed32(u32),
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub number: u32,
    pub value: Value,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Message {
    pub fields: Vec<Field>,
}

impl Message {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn decode(mut bytes: &[u8]) -> Result<Self> {
        let mut fields = Vec::new();
        while !bytes.is_empty() {
            let key = read_varint(&mut bytes)?;
            let number = u32::try_from(key >> 3).map_err(|_| anyhow!("field number is too large: {key}"))?;
            if number == 0 {
                bail!("field number 0 is invalid");
            }

            let value = match key & 7 {
                0 => Value::Varint(read_varint(&mut bytes)?),
                1 => Value::Fixed64(u64::from_le_bytes(take(&mut bytes, 8)?.try_into()?)),
                2 => {
                    let length = usize::try_from(read_varint(&mut bytes)?)?;
                    Value::Bytes(take(&mut bytes, length)?.to_vec())
                }
                5 => Value::Fixed32(u32::from_le_bytes(take(&mut bytes, 4)?.try_into()?)),
                wire => bail!("unsupported wire type {wire} of field {number}"),
            };
            fields.push(Field { number, value });
        }
        Ok(Self { fields })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for field in &self.fields {
            let number = u64::from(field.number) << 3;
            match &field.value {
                Value::Varint(e) => {
                    write_varint(&mut out, number);
                    write_varint(&mut out, *e);
                }
                Value::Fixed64(e) => {
                    write_varint(&mut out, number | 1);
                    out.extend_from_slice(&e.to_le_bytes());
                }
                Value::Bytes(e) => {
                    write_varint(&mut out, number | 2);
                    write_varint(&mut out, e.len() as u64);
                    out.extend_from_slice(e);
                }
                Value::Fixed32(e) => {
                    write_varint(&mut out, number | 5);
                    out.extend_from_slice(&e.to_le_bytes());
                }
            }
        }
        out
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn last(&self, number: u32) -> Option<&Value> {
        self.fields.iter().rev().find(|e| e.number == number).map(|e| &e.value)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn has(&self, number: u32) -> bool {
        self.fields.iter().any(|e| e.number == number)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn varint(&self, number: u32) -> Option<u64> {
        match self.last(number)? {
            Value::Varint(e) => Some(*e),
            _ => None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn bool(&self, number: u32) -> Option<bool> {
        self.varint(number).map(|e| e != 0)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn float(&self, number: u32) -> Option<f32> {
        match self.last(number)? {
            Value::Fixed32(e) => Some(f32::from_bits(*e)),
            _ => None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn double(&self, number: u32) -> Option<f64> {
        match self.last(number)? {
            Value::Fixed64(e) => Some(f64::from_bits(*e)),
            _ => None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn bytes(&self, number: u32) -> Option<&[u8]> {
        match self.last(number)? {
            Value::Bytes(e) => Some(e),
            _ => None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn string(&self, number: u32) -> Option<String> {
        self.bytes(number).map(|e| String::from_utf8_lossy(e).into_owned())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn strings(&self, number: u32) -> Vec<String> {
        self.fields
            .iter()
            .filter(|e| e.number == number)
            .filter_map(|e| match &e.value {
                Value::Bytes(e) => Some(String::from_utf8_lossy(e).into_owned()),
                _ => None,
            })
            .collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn message(&self, number: u32) -> Result<Option<Message>> {
        self.bytes(number).map(Message::decode).transpose()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn messages(&self, number: u32) -> Result<Vec<Message>> {
        self.fields
            .iter()
            .filter(|e| e.number == number)
            .map(|e| match &e.value {
                Value::Bytes(e) => Message::decode(e),
                _ => bail!("field {number} is not a message"),
            })
            .collect()
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Repeated varints, accepting both the packed and the unpacked encoding
    pub fn varints(&self, number: u32) -> Result<Vec<u64>> {
        let mut values = Vec::new();
        for field in self.fields.iter().filter(|e| e.number == number) {
            match &field.value {
                Value::Varint(e) => values.push(*e),
                Value::Bytes(e) => {
                    let mut bytes = e.as_slice();
                    while !bytes.is_empty() {
                        values.push(read_varint(&mut bytes)?);
                    }
                }
                _ => bail!("field {number} is not a varint"),
            }
        }
        Ok(values)
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Replaces the first occurrence in place (keeping field order) and drops the others
    pub fn set(&mut self, number: u32, value: Value) {
        match self.fields.iter().position(|e| e.number == number) {
            Some(index) => {
                self.fields[index].value = value;
                let mut seen = 0;
                self.fields.retain(|e| {
                    seen += usize::from(e.number == number);
                    e.number != number || seen == 1
                });
            }
            None => self.fields.push(Field { number, value }),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_varint(&mut self, number: u32, value: u64) {
        self.set(number, Value::Varint(value));
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_bool(&mut self, number: u32, value: bool) {
        self.set(number, Value::Varint(u64::from(value)));
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_bytes(&mut self, number: u32, value: Vec<u8>) {
        self.set(number, Value::Bytes(value));
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn set_message(&mut self, number: u32, value: &Message) {
        self.set(number, Value::Bytes(value.encode()));
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn push_message(&mut self, number: u32, value: &Message) {
        self.fields.push(Field {
            number,
            value: Value::Bytes(value.encode()),
        });
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Replaces every occurrence of a repeated message field, the new ones go where the first old one was
    pub fn set_messages(&mut self, number: u32, values: &[Message]) {
        let index = self.fields.iter().position(|e| e.number == number);
        self.remove(number);

        let index = index.unwrap_or(self.fields.len()).min(self.fields.len());
        let fields = values.iter().map(|e| Field {
            number,
            value: Value::Bytes(e.encode()),
        });
        self.fields.splice(index..index, fields);
    }

    ////////////////////////////////////////////////////////////////////////////////
    /// Replaces a repeated varint field with a single packed occurrence
    pub fn set_packed_varints(&mut self, number: u32, values: &[u64]) {
        if values.is_empty() {
            self.remove(number);
            return;
        }

        let mut packed = Vec::new();
        for value in values {
            write_varint(&mut packed, *value);
        }
        self.set(number, Value::Bytes(packed));
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn remove(&mut self, number: u32) {
        self.fields.retain(|e| e.number != number);
    }
}

////////////////////////////////////////////////////////////////////////////////
// TSP.Reference
////////////////////////////////////////////////////////////////////////////////

const REFERENCE_IDENTIFIER: u32 = 1;

////////////////////////////////////////////////////////////////////////////////
pub fn reference(message: &Message, number: u32) -> Result<Option<u64>> {
    Ok(message.message(number)?.and_then(|e| e.varint(REFERENCE_IDENTIFIER)))
}

////////////////////////////////////////////////////////////////////////////////
pub fn references(message: &Message, number: u32) -> Result<Vec<u64>> {
    let references = message.messages(number)?;
    Ok(references
        .iter()
        .filter_map(|e| e.varint(REFERENCE_IDENTIFIER))
        .collect())
}

////////////////////////////////////////////////////////////////////////////////
pub fn make_reference(identifier: u64) -> Message {
    let mut reference = Message::default();
    reference.set_varint(REFERENCE_IDENTIFIER, identifier);
    reference
}

////////////////////////////////////////////////////////////////////////////////
pub enum ReferenceAction {
    Keep,
    Replace(u64),
    Remove,
}

////////////////////////////////////////////////////////////////////////////////
/// Visits every nested message shaped like a TSP.Reference ({1: id} plus the deprecated 2 and 3).
///
/// Without a schema a reference can't be told apart from any other small message for sure,
/// so callers only act on ids the archive header lists as object references. Returns whether
/// the message changed.
pub fn rewrite_references(message: &mut Message, visit: &mut dyn FnMut(u64) -> ReferenceAction) -> bool {
    rewrite_references_at(message, visit, 0)
}

////////////////////////////////////////////////////////////////////////////////
fn rewrite_references_at(message: &mut Message, visit: &mut dyn FnMut(u64) -> ReferenceAction, depth: usize) -> bool {
    const MAX_DEPTH: usize = 64;
    if depth > MAX_DEPTH {
        return false;
    }

    let mut changed = false;
    let mut index = 0;
    while index < message.fields.len() {
        let Value::Bytes(bytes) = &message.fields[index].value else {
            index += 1;
            continue;
        };
        let Ok(mut nested) = Message::decode(bytes) else {
            index += 1;
            continue;
        };

        if let Some(identifier) = reference_shape(&nested) {
            match visit(identifier) {
                ReferenceAction::Keep => {}
                ReferenceAction::Replace(e) => {
                    nested.set_varint(REFERENCE_IDENTIFIER, e);
                    message.fields[index].value = Value::Bytes(nested.encode());
                    changed = true;
                }
                ReferenceAction::Remove => {
                    message.fields.remove(index);
                    changed = true;
                    continue;
                }
            }
        } else if !nested.fields.is_empty() && rewrite_references_at(&mut nested, visit, depth + 1) {
            message.fields[index].value = Value::Bytes(nested.encode());
            changed = true;
        }
        index += 1;
    }
    changed
}

////////////////////////////////////////////////////////////////////////////////
fn reference_shape(message: &Message) -> Option<u64> {
    let mut identifier = None;
    for field in &message.fields {
        match (field.number, &field.value) {
            (1, Value::Varint(e)) if identifier.is_none() => identifier = Some(*e),
            (2 | 3, Value::Varint(_)) => {}
            _ => return None,
        }
    }
    identifier
}

////////////////////////////////////////////////////////////////////////////////
// Varints
////////////////////////////////////////////////////////////////////////////////

pub fn read_varint(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or_else(|| anyhow!("truncated varint"))?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("varint is too long")
}

////////////////////////////////////////////////////////////////////////////////
pub fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

////////////////////////////////////////////////////////////////////////////////
fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    if bytes.len() < length {
        bail!("truncated field, expected {length} bytes, got {}", bytes.len());
    }
    let (head, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(head)
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_wire_type() {
        let mut message = Message::default();
        message.set_varint(1, 300);
        message.set(2, Value::Fixed64(u64::MAX));
        message.set_bytes(3, b"text".to_vec());
        message.set(4, Value::Fixed32(1.5f32.to_bits()));

        let decoded = Message::decode(&message.encode()).unwrap();
        assert_eq!(decoded, message);
        assert_eq!(decoded.float(4), Some(1.5));
    }

    #[test]
    fn reads_packed_and_unpacked_varints() {
        let mut message = Message::default();
        message.set_packed_varints(5, &[1, 200, 70000]);
        message.fields.push(Field {
            number: 5,
            value: Value::Varint(9),
        });
        assert_eq!(message.varints(5).unwrap(), vec![1, 200, 70000, 9]);
    }

    #[test]
    fn set_keeps_position_and_drops_duplicates() {
        let mut message = Message::default();
        message.set_varint(1, 1);
        message.fields.push(Field {
            number: 2,
            value: Value::Varint(2),
        });
        message.fields.push(Field {
            number: 1,
            value: Value::Varint(3),
        });
        message.set_varint(1, 4);
        assert_eq!(message.fields.len(), 2);
        assert_eq!(message.fields[0].number, 1);
        assert_eq!(message.varint(1), Some(4));
    }

    #[test]
    fn rewrites_nested_references() {
        let mut inner = Message::default();
        inner.push_message(7, &make_reference(10));
        inner.push_message(7, &make_reference(11));

        let mut outer = Message::default();
        outer.push_message(2, &make_reference(10));
        outer.set_message(3, &inner);

        let changed = rewrite_references(&mut outer, &mut |e| match e {
            10 => ReferenceAction::Replace(20),
            11 => ReferenceAction::Remove,
            _ => ReferenceAction::Keep,
        });

        assert!(changed);
        assert_eq!(reference(&outer, 2).unwrap(), Some(20));
        assert_eq!(references(&outer.message(3).unwrap().unwrap(), 7).unwrap(), vec![20]);
    }
}
