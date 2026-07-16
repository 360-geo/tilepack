//! The 24-byte fixed header at offset 0.

use crate::error::{ParseError, WriteError};

/// Magic bytes at offset 0.
pub const MAGIC: [u8; 4] = *b"TPCK";
/// The format version this crate reads and writes.
pub const VERSION: u8 = 1;
/// Header length in bytes.
pub const HEADER_LEN: usize = 24;

/// The fixed header. Everything a reader needs to locate the descriptor
/// table; the tile count additionally needs the descriptors themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    /// 1 for planar rasters, 6 for cubemaps.
    pub face_count: u8,
    /// Pyramid level count. Level 0 is coarsest, `levels - 1` finest.
    pub levels: u8,
    pub group_count: u8,
    pub tile_size: u16,
    /// Finest-level width in pixels. For cubemaps this is the per-face width.
    pub root_w: u32,
    /// Finest-level height in pixels. For cubemaps this is the per-face height.
    pub root_h: u32,
}

impl Header {
    /// Byte offset where the descriptor table ends and the index begins.
    pub const fn descriptors_end(&self) -> usize {
        HEADER_LEN + crate::descriptor::DESCRIPTOR_LEN * self.group_count as usize
    }

    /// Parse and validate a header from the first [`HEADER_LEN`] bytes.
    pub fn parse(buf: &[u8]) -> Result<Header, ParseError> {
        if buf.len() < HEADER_LEN {
            return Err(ParseError::Truncated { needed: HEADER_LEN });
        }
        if buf[0..4] != MAGIC {
            return Err(ParseError::BadMagic);
        }
        let version = buf[4];
        if version != VERSION {
            return Err(ParseError::BadVersion {
                found: version,
                supported: VERSION,
            });
        }
        let face_count = buf[5];
        if face_count != 1 && face_count != 6 {
            return Err(ParseError::BadFaceCount(face_count));
        }
        let levels = buf[6];
        let group_count = buf[7];
        let tile_size = u16::from_le_bytes([buf[8], buf[9]]);
        // buf[10..12] reserved
        let root_w = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let root_h = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        // buf[20..24] reserved

        let header = Header {
            version,
            face_count,
            levels,
            group_count,
            tile_size,
            root_w,
            root_h,
        };
        header.validate()?;
        Ok(header)
    }

    fn validate(&self) -> Result<(), ParseError> {
        if self.levels < 1 {
            return Err(ParseError::Inconsistent("levels must be >= 1"));
        }
        if self.group_count < 1 {
            return Err(ParseError::Inconsistent("group_count must be >= 1"));
        }
        if self.tile_size < 1 {
            return Err(ParseError::Inconsistent("tile_size must be >= 1"));
        }
        if self.root_w < 1 || self.root_h < 1 {
            return Err(ParseError::Inconsistent("root dimensions must be >= 1"));
        }
        if self.face_count == 6 && self.root_w != self.root_h {
            return Err(ParseError::Inconsistent("cubemap requires root_w == root_h"));
        }
        Ok(())
    }

    /// Serialize into a 24-byte array.
    pub fn to_bytes(&self) -> Result<[u8; HEADER_LEN], WriteError> {
        if self.face_count != 1 && self.face_count != 6 {
            return Err(WriteError::InvalidParams("face_count must be 1 or 6"));
        }
        if self.levels < 1 || self.group_count < 1 || self.tile_size < 1 {
            return Err(WriteError::InvalidParams("levels, group_count, tile_size must be >= 1"));
        }
        if self.root_w < 1 || self.root_h < 1 {
            return Err(WriteError::InvalidParams("root dimensions must be >= 1"));
        }
        if self.face_count == 6 && self.root_w != self.root_h {
            return Err(WriteError::InvalidParams("cubemap requires root_w == root_h"));
        }
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4] = self.version;
        b[5] = self.face_count;
        b[6] = self.levels;
        b[7] = self.group_count;
        b[8..10].copy_from_slice(&self.tile_size.to_le_bytes());
        b[12..16].copy_from_slice(&self.root_w.to_le_bytes());
        b[16..20].copy_from_slice(&self.root_h.to_le_bytes());
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let h = Header {
            version: VERSION,
            face_count: 6,
            levels: 4,
            group_count: 2,
            tile_size: 512,
            root_w: 4096,
            root_h: 4096,
        };
        let bytes = h.to_bytes().unwrap();
        assert_eq!(Header::parse(&bytes).unwrap(), h);
    }

    #[test]
    fn cubemap_requires_square_root() {
        let h = Header {
            version: VERSION,
            face_count: 6,
            levels: 2,
            group_count: 1,
            tile_size: 512,
            root_w: 4096,
            root_h: 2048,
        };
        assert!(h.to_bytes().is_err());
    }
}
