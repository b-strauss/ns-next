//! The artifact's byte primitives: fixed-width little-endian numbers and
//! length-prefixed strings. The reader panics on bytes that do not decode:
//! they are the binary's own, so a short read is a defect of the binary.

pub(crate) struct ByteWriter {
    buffer: Vec<u8>,
}

impl ByteWriter {
    pub(crate) fn new() -> ByteWriter {
        ByteWriter { buffer: Vec::new() }
    }
    pub(crate) fn u8(&mut self, value: u8) {
        self.buffer.push(value);
    }
    pub(crate) fn boolean(&mut self, value: bool) {
        self.u8(u8::from(value));
    }
    pub(crate) fn u16(&mut self, value: u16) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    pub(crate) fn u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }
    pub(crate) fn length(&mut self, value: usize) {
        self.u32(u32::try_from(value).expect("internal: a shipped length fits a u32"));
    }
    pub(crate) fn str(&mut self, text: &str) {
        self.length(text.len());
        self.buffer.extend_from_slice(text.as_bytes());
    }
    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }
}

// Read by the tests now, and by the boot from the next step on.
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) struct ByteReader<'bytes> {
    bytes: &'bytes [u8],
    position: usize,
}

#[cfg_attr(not(test), expect(dead_code))]
impl<'bytes> ByteReader<'bytes> {
    pub(crate) fn new(bytes: &'bytes [u8]) -> ByteReader<'bytes> {
        ByteReader { bytes, position: 0 }
    }
    pub(crate) fn raw(&mut self, length: usize) -> &'bytes [u8] {
        let end = self.position + length;
        let slice = self
            .bytes
            .get(self.position..end)
            .expect("internal: the embedded shipped code ends early");
        self.position = end;
        slice
    }
    pub(crate) fn u8(&mut self) -> u8 {
        self.raw(1)[0]
    }
    pub(crate) fn boolean(&mut self) -> bool {
        match self.u8() {
            0 => false,
            1 => true,
            other => panic!("internal: the embedded shipped code has a boolean {other}"),
        }
    }
    pub(crate) fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.raw(2).try_into().expect("two bytes"))
    }
    pub(crate) fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.raw(4).try_into().expect("four bytes"))
    }
    pub(crate) fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.raw(8).try_into().expect("eight bytes"))
    }
    pub(crate) fn length(&mut self) -> usize {
        self.u32() as usize
    }
    pub(crate) fn str(&mut self) -> &'bytes str {
        let length = self.length();
        std::str::from_utf8(self.raw(length))
            .expect("internal: the embedded shipped code holds UTF-8")
    }
}
