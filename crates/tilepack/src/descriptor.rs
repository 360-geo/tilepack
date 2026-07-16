//! Per-group descriptors: what a band group's blobs mean and how they tile.

use crate::error::{ParseError, WriteError};

/// Descriptor length in bytes.
pub const DESCRIPTOR_LEN: usize = 48;

/// What a band group represents. Unknown values are preserved as
/// [`Semantic::Other`] so readers can skip them without losing round-trip
/// fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantic {
    Rgb,
    Nir,
    Tir,
    Depth,
    Other(u8),
}

impl Semantic {
    pub fn from_u8(v: u8) -> Semantic {
        match v {
            0 => Semantic::Rgb,
            1 => Semantic::Nir,
            2 => Semantic::Tir,
            3 => Semantic::Depth,
            other => Semantic::Other(other),
        }
    }
    pub fn to_u8(self) -> u8 {
        match self {
            Semantic::Rgb => 0,
            Semantic::Nir => 1,
            Semantic::Tir => 2,
            Semantic::Depth => 3,
            Semantic::Other(v) => v,
        }
    }
}

/// Tile blob codec. Unknown values preserved as [`Codec::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Jpeg,
    Webp,
    /// Lossless WebP carrying a u16 count per pixel as `R*256 + G`.
    WebpSplit16,
    Depthpack,
    JpegXl,
    Other(u8),
}

impl Codec {
    pub fn from_u8(v: u8) -> Codec {
        match v {
            0 => Codec::Jpeg,
            1 => Codec::Webp,
            2 => Codec::WebpSplit16,
            3 => Codec::Depthpack,
            4 => Codec::JpegXl,
            other => Codec::Other(other),
        }
    }
    pub fn to_u8(self) -> u8 {
        match self {
            Codec::Jpeg => 0,
            Codec::Webp => 1,
            Codec::WebpSplit16 => 2,
            Codec::Depthpack => 3,
            Codec::JpegXl => 4,
            Codec::Other(v) => v,
        }
    }
}

/// Pixel sample layout of a decoded tile. Unknown values preserved as
/// [`SampleType::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    Rgb8,
    Gray8,
    U16,
    Other(u8),
}

impl SampleType {
    pub fn from_u8(v: u8) -> SampleType {
        match v {
            0 => SampleType::Rgb8,
            1 => SampleType::Gray8,
            2 => SampleType::U16,
            other => SampleType::Other(other),
        }
    }
    pub fn to_u8(self) -> u8 {
        match self {
            SampleType::Rgb8 => 0,
            SampleType::Gray8 => 1,
            SampleType::U16 => 2,
            SampleType::Other(v) => v,
        }
    }
}

/// Group layout flags. Raw bits are preserved; unknown bits are ignored by
/// accessors but kept for round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupFlags(pub u8);

impl GroupFlags {
    pub const UNTILED: u8 = 0b0000_0001;
    pub const NEAREST_DOWNSAMPLE: u8 = 0b0000_0010;

    pub fn new(untiled: bool, nearest_downsample: bool) -> GroupFlags {
        let mut bits = 0;
        if untiled {
            bits |= Self::UNTILED;
        }
        if nearest_downsample {
            bits |= Self::NEAREST_DOWNSAMPLE;
        }
        GroupFlags(bits)
    }

    /// One blob per face per level (no tile grid).
    pub fn untiled(self) -> bool {
        self.0 & Self::UNTILED != 0
    }

    /// Pyramid built by nearest decimation rather than averaging.
    pub fn nearest_downsample(self) -> bool {
        self.0 & Self::NEAREST_DOWNSAMPLE != 0
    }
}

/// Physical mapping for raw-value groups: `value = count * scale + offset`,
/// labelled by an opaque `unit`. All zero for display imagery.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radiometry {
    pub scale: f64,
    pub offset: f64,
    pub nodata: u16,
    /// Default display window low, in counts.
    pub min: u16,
    /// Default display window high, in counts.
    pub max: u16,
    /// ASCII unit label, null-padded; never interpreted by the container.
    pub unit: [u8; 8],
}

impl Default for Radiometry {
    fn default() -> Radiometry {
        Radiometry {
            scale: 0.0,
            offset: 0.0,
            nodata: 0,
            min: 0,
            max: 0,
            unit: [0; 8],
        }
    }
}

impl Radiometry {
    /// The unit label as a string, trimming trailing NULs. Returns `None` if
    /// the bytes are not valid UTF-8.
    pub fn unit_str(&self) -> Option<&str> {
        let end = self.unit.iter().position(|&b| b == 0).unwrap_or(self.unit.len());
        std::str::from_utf8(&self.unit[..end]).ok()
    }

    /// Build a unit label from a string, truncating to 8 bytes.
    pub fn unit_from_str(s: &str) -> [u8; 8] {
        let mut u = [0u8; 8];
        let bytes = s.as_bytes();
        let n = bytes.len().min(8);
        u[..n].copy_from_slice(&bytes[..n]);
        u
    }
}

/// One 48-byte group descriptor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupDescriptor {
    pub semantic: Semantic,
    pub codec: Codec,
    pub sample: SampleType,
    pub flags: GroupFlags,
    /// Number of finest levels this group covers, `1..=header.levels`.
    pub level_count: u8,
    pub radiometry: Radiometry,
}

impl GroupDescriptor {
    /// Parse a descriptor from exactly [`DESCRIPTOR_LEN`] bytes.
    pub fn parse(buf: &[u8]) -> Result<GroupDescriptor, ParseError> {
        if buf.len() < DESCRIPTOR_LEN {
            return Err(ParseError::Truncated { needed: DESCRIPTOR_LEN });
        }
        let semantic = Semantic::from_u8(buf[0]);
        let codec = Codec::from_u8(buf[1]);
        let sample = SampleType::from_u8(buf[2]);
        let flags = GroupFlags(buf[3]);
        let level_count = buf[4];
        // buf[5..8] reserved
        let scale = f64::from_le_bytes(buf[8..16].try_into().unwrap());
        let offset = f64::from_le_bytes(buf[16..24].try_into().unwrap());
        let nodata = u16::from_le_bytes([buf[24], buf[25]]);
        let min = u16::from_le_bytes([buf[26], buf[27]]);
        let max = u16::from_le_bytes([buf[28], buf[29]]);
        // buf[30..32] reserved
        let mut unit = [0u8; 8];
        unit.copy_from_slice(&buf[32..40]);
        // buf[40..48] reserved

        Ok(GroupDescriptor {
            semantic,
            codec,
            sample,
            flags,
            level_count,
            radiometry: Radiometry {
                scale,
                offset,
                nodata,
                min,
                max,
                unit,
            },
        })
    }

    /// Serialize into a 48-byte array.
    pub fn to_bytes(&self, levels: u8) -> Result<[u8; DESCRIPTOR_LEN], WriteError> {
        if self.level_count < 1 || self.level_count > levels {
            return Err(WriteError::InvalidParams("level_count must be in 1..=levels"));
        }
        let mut b = [0u8; DESCRIPTOR_LEN];
        b[0] = self.semantic.to_u8();
        b[1] = self.codec.to_u8();
        b[2] = self.sample.to_u8();
        b[3] = self.flags.0;
        b[4] = self.level_count;
        b[8..16].copy_from_slice(&self.radiometry.scale.to_le_bytes());
        b[16..24].copy_from_slice(&self.radiometry.offset.to_le_bytes());
        b[24..26].copy_from_slice(&self.radiometry.nodata.to_le_bytes());
        b[26..28].copy_from_slice(&self.radiometry.min.to_le_bytes());
        b[28..30].copy_from_slice(&self.radiometry.max.to_le_bytes());
        b[32..40].copy_from_slice(&self.radiometry.unit);
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_unknown_values() {
        let d = GroupDescriptor {
            semantic: Semantic::Other(200),
            codec: Codec::Other(150),
            sample: SampleType::U16,
            flags: GroupFlags(0b0000_0011),
            level_count: 3,
            radiometry: Radiometry {
                scale: 0.001,
                offset: -1.5,
                nodata: 0,
                min: 100,
                max: 60000,
                unit: Radiometry::unit_from_str("K"),
            },
        };
        let bytes = d.to_bytes(4).unwrap();
        let parsed = GroupDescriptor::parse(&bytes).unwrap();
        assert_eq!(parsed, d);
        assert_eq!(parsed.radiometry.unit_str(), Some("K"));
    }

    #[test]
    fn level_count_bounds_checked() {
        let d = GroupDescriptor {
            semantic: Semantic::Rgb,
            codec: Codec::Webp,
            sample: SampleType::Rgb8,
            flags: GroupFlags::default(),
            level_count: 5,
            radiometry: Radiometry::default(),
        };
        assert!(d.to_bytes(4).is_err(), "level_count 5 > levels 4 rejected");
        assert!(d.to_bytes(5).is_ok());
    }
}
