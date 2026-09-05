//! Canonical decoding, the inverse of `encode.rs`. A malformed byte stream
//! is a `DecodeError`, never a panic.

use crate::types::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEof,
    BadMagic,
    VersionMismatch { found: u32, expected: u32 },
    InvalidTag { what: &'static str, tag: u8 },
    InvalidUtf8,
    BadCodeId(u32),
    BadObjectId(u32),
    BadFrameId(u32),
    UnknownSelector(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::UnexpectedEof => write!(f, "unexpected end of image"),
            DecodeError::BadMagic => write!(f, "not an ns-next image (bad magic)"),
            DecodeError::VersionMismatch { found, expected } => {
                write!(f, "image format version {found} is not supported (this build reads version {expected})")
            }
            DecodeError::InvalidTag { what, tag } => write!(f, "invalid {what} tag {tag}"),
            DecodeError::InvalidUtf8 => write!(f, "invalid UTF-8 in image string"),
            DecodeError::BadCodeId(id) => write!(f, "code id {id} out of range"),
            DecodeError::BadObjectId(id) => write!(f, "object id {id} out of range"),
            DecodeError::BadFrameId(id) => write!(f, "frame id {id} out of range"),
            DecodeError::UnknownSelector(s) => write!(
                f,
                "image refers to method `{s}`, which the reloaded source does not define"
            ),
        }
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    pub fn read_raw(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DecodeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read_raw(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, DecodeError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::InvalidTag { what: "bool", tag }),
        }
    }

    pub fn read_u16(&mut self) -> Result<u16, DecodeError> {
        let bytes: [u8; 2] = self
            .read_raw(2)?
            .try_into()
            .expect("read_raw(2) returns 2 bytes");
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let bytes: [u8; 4] = self
            .read_raw(4)?
            .try_into()
            .expect("read_raw(4) returns 4 bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read_i64(&mut self) -> Result<i64, DecodeError> {
        let bytes: [u8; 8] = self
            .read_raw(8)?
            .try_into()
            .expect("read_raw(8) returns 8 bytes");
        Ok(i64::from_le_bytes(bytes))
    }

    pub fn read_f64(&mut self) -> Result<f64, DecodeError> {
        let bytes: [u8; 8] = self
            .read_raw(8)?
            .try_into()
            .expect("read_raw(8) returns 8 bytes");
        Ok(f64::from_le_bytes(bytes))
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let len = self.read_u32()? as usize;
        Ok(self.read_raw(len)?.to_vec())
    }

    pub fn read_str(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.read_bytes()?).map_err(|_| DecodeError::InvalidUtf8)
    }
}

pub fn read_header(r: &mut Reader) -> Result<(), DecodeError> {
    let magic = r.read_raw(4)?;
    if magic != super::encode::MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = r.read_u32()?;
    if version != super::encode::FORMAT_VERSION {
        return Err(DecodeError::VersionMismatch {
            found: version,
            expected: super::encode::FORMAT_VERSION,
        });
    }
    Ok(())
}

pub fn decode_scalar_value(r: &mut Reader) -> Result<Value, DecodeError> {
    match r.read_u8()? {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::SmallInt(r.read_i64()?)),
        4 => Ok(Value::Float(r.read_f64()?)),
        tag => Err(DecodeError::InvalidTag { what: "value", tag }),
    }
}
