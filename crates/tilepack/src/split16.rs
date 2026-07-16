//! Packing and unpacking u16 counts as `webp-split16` RGB bytes.
//!
//! A u16 count is stored as `R = count >> 8`, `G = count & 0xff`, `B = 0`.
//! Reconstruction `count = (R << 8) | G` is linear in R and G, so bilinear
//! filtering of the split channels interpolates the count correctly (away
//! from nodata). The loops are plain and byte-strided, which LLVM
//! autovectorizes; no explicit SIMD is warranted at ~1 GB/s scalar.

/// Pack `counts` into `rgb`, which must be exactly `counts.len() * 3` bytes.
///
/// # Panics
/// Panics if `rgb.len() != counts.len() * 3`.
pub fn split16_pack(counts: &[u16], rgb: &mut [u8]) {
    assert_eq!(rgb.len(), counts.len() * 3, "rgb buffer must be counts.len() * 3");
    for (c, px) in counts.iter().zip(rgb.chunks_exact_mut(3)) {
        px[0] = (c >> 8) as u8;
        px[1] = (c & 0xff) as u8;
        px[2] = 0;
    }
}

/// Unpack `rgb` (`counts.len() * 3` bytes) into `counts`.
///
/// # Panics
/// Panics if `rgb.len() != counts.len() * 3`.
pub fn split16_unpack(rgb: &[u8], counts: &mut [u16]) {
    assert_eq!(rgb.len(), counts.len() * 3, "rgb buffer must be counts.len() * 3");
    for (c, px) in counts.iter_mut().zip(rgb.chunks_exact(3)) {
        *c = ((px[0] as u16) << 8) | px[1] as u16;
    }
}

/// Convenience: allocate and pack.
pub fn split16_pack_vec(counts: &[u16]) -> Vec<u8> {
    let mut rgb = vec![0u8; counts.len() * 3];
    split16_pack(counts, &mut rgb);
    rgb
}

/// Convenience: allocate and unpack. `rgb.len()` must be a multiple of 3.
pub fn split16_unpack_vec(rgb: &[u8]) -> Vec<u16> {
    let mut counts = vec![0u16; rgb.len() / 3];
    split16_unpack(&rgb[..counts.len() * 3], &mut counts);
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhaustive_u16_roundtrip() {
        // Every u16 value packs and unpacks bit-exactly, and B stays 0.
        let counts: Vec<u16> = (0..=u16::MAX).collect();
        let rgb = split16_pack_vec(&counts);
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            assert_eq!(px[2], 0, "B channel must be zero at {i}");
        }
        let back = split16_unpack_vec(&rgb);
        assert_eq!(back, counts);
    }

    #[test]
    fn known_packing() {
        // 1000 mm -> R=0x03 G=0xE8; 65535 -> R=0xFF G=0xFF.
        let rgb = split16_pack_vec(&[1000, 65535, 0]);
        assert_eq!(&rgb[0..3], &[0x03, 0xE8, 0x00]);
        assert_eq!(&rgb[3..6], &[0xFF, 0xFF, 0x00]);
        assert_eq!(&rgb[6..9], &[0x00, 0x00, 0x00]);
    }
}
