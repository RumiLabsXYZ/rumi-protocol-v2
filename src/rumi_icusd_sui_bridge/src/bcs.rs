//! The small subset of BCS (Binary Canonical Serialization) the foundation needs, written out
//! by hand so every byte of a signed transaction is visible in this crate: ULEB128 lengths and
//! enum tags, little-endian integers, raw 32-byte addresses, length-prefixed byte strings.
//!
//! The reader is strict: ULEB128 values must be canonical (no padding bytes) and fit `u32`,
//! booleans are 0 or 1, and [`Reader::finish`] refuses trailing bytes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BcsError {
    /// Ran out of input.
    Eof,
    /// Bytes left after a value that should have consumed the whole input.
    Trailing(usize),
    /// A ULEB128 value that is padded, longer than five bytes, or above `u32::MAX`.
    BadUleb,
    /// A boolean that is neither 0 nor 1.
    BadBool(u8),
    /// A length above the caller's bound.
    TooLong { len: usize, max: usize },
    /// A string that is not UTF-8.
    BadUtf8,
}

impl core::fmt::Display for BcsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Appends BCS values to a buffer.
#[derive(Default, Debug, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// ULEB128, used for sequence lengths and enum variant indexes.
    pub fn uleb(&mut self, mut n: u32) -> &mut Self {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                self.buf.push(byte);
                return self;
            }
            self.buf.push(byte | 0x80);
        }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.buf.push(v as u8);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u128(&mut self, v: u128) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Raw bytes with no length prefix (addresses, object ids, fixed arrays).
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// `vector<u8>` / `bytes` / `String`: ULEB128 length, then the bytes.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.uleb(u32::try_from(v.len()).expect("BCS sequence length fits u32"));
        self.buf.extend_from_slice(v);
        self
    }

    pub fn string(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
}

/// Reads BCS values from a slice.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], BcsError> {
        if self.remaining() < n {
            return Err(BcsError::Eof);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, BcsError> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool, BcsError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(BcsError::BadBool(other)),
        }
    }

    pub fn u16(&mut self) -> Result<u16, BcsError> {
        let mut b = [0u8; 2];
        b.copy_from_slice(self.take(2)?);
        Ok(u16::from_le_bytes(b))
    }

    pub fn u64(&mut self) -> Result<u64, BcsError> {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(b))
    }

    pub fn u128(&mut self) -> Result<u128, BcsError> {
        let mut b = [0u8; 16];
        b.copy_from_slice(self.take(16)?);
        Ok(u128::from_le_bytes(b))
    }

    /// Canonical ULEB128 into a `u32`.
    pub fn uleb(&mut self) -> Result<u32, BcsError> {
        let mut value: u64 = 0;
        for i in 0..5 {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                // A final zero byte after a continuation is padding, not canonical.
                if i > 0 && byte == 0 {
                    return Err(BcsError::BadUleb);
                }
                return u32::try_from(value).map_err(|_| BcsError::BadUleb);
            }
        }
        Err(BcsError::BadUleb)
    }

    pub fn raw<const N: usize>(&mut self) -> Result<[u8; N], BcsError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// A length-prefixed byte string no longer than `max`.
    pub fn bytes(&mut self, max: usize) -> Result<Vec<u8>, BcsError> {
        let len = self.uleb()? as usize;
        if len > max {
            return Err(BcsError::TooLong { len, max });
        }
        Ok(self.take(len)?.to_vec())
    }

    pub fn string(&mut self, max: usize) -> Result<String, BcsError> {
        String::from_utf8(self.bytes(max)?).map_err(|_| BcsError::BadUtf8)
    }

    /// Succeeds only when the whole input was consumed.
    pub fn finish(self) -> Result<(), BcsError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(BcsError::Trailing(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uleb_known_encodings() {
        let cases: [(u32, &[u8]); 6] = [
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (300, &[0xac, 0x02]),
            (u32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        ];
        for (n, enc) in cases {
            let mut w = Writer::new();
            w.uleb(n);
            assert_eq!(w.into_bytes(), enc, "encode {n}");
            let mut r = Reader::new(enc);
            assert_eq!(r.uleb().unwrap(), n, "decode {n}");
            r.finish().unwrap();
        }
    }

    #[test]
    fn uleb_rejects_padding_and_overflow() {
        assert_eq!(Reader::new(&[0x80, 0x00]).uleb(), Err(BcsError::BadUleb), "padded zero");
        assert_eq!(Reader::new(&[0x81, 0x80, 0x00]).uleb(), Err(BcsError::BadUleb), "padded one");
        assert_eq!(Reader::new(&[0xff, 0xff, 0xff, 0xff, 0x1f]).uleb(), Err(BcsError::BadUleb), "above u32");
        assert_eq!(Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).uleb(), Err(BcsError::BadUleb), "six bytes");
        assert_eq!(Reader::new(&[0x80]).uleb(), Err(BcsError::Eof));
    }

    #[test]
    fn integers_are_little_endian_and_bytes_are_length_prefixed() {
        let mut w = Writer::new();
        w.u16(0x0102).u64(0x0102030405060708).u128(1).bool(true).bytes(&[9, 8, 7]).string("ab");
        let bytes = w.into_bytes();
        assert_eq!(&bytes[..2], &[0x02, 0x01]);
        assert_eq!(&bytes[2..10], &[8, 7, 6, 5, 4, 3, 2, 1]);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u16().unwrap(), 0x0102);
        assert_eq!(r.u64().unwrap(), 0x0102030405060708);
        assert_eq!(r.u128().unwrap(), 1);
        assert!(r.bool().unwrap());
        assert_eq!(r.bytes(10).unwrap(), vec![9, 8, 7]);
        assert_eq!(r.string(10).unwrap(), "ab");
        r.finish().unwrap();
    }

    #[test]
    fn the_reader_is_strict() {
        assert_eq!(Reader::new(&[2]).bool(), Err(BcsError::BadBool(2)));
        assert_eq!(Reader::new(&[3, 1, 2]).bytes(10), Err(BcsError::Eof));
        assert_eq!(Reader::new(&[3, 1, 2, 3]).bytes(2), Err(BcsError::TooLong { len: 3, max: 2 }));
        assert_eq!(Reader::new(&[2, 0xff, 0xfe]).string(8), Err(BcsError::BadUtf8));
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert_eq!(r.finish(), Err(BcsError::Trailing(1)));
    }
}
