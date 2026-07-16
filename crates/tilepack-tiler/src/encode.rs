//! Tile encoding. WebP via libwebp; the tile-parallel hot stage of conversion.

use crate::TilerError;
use crate::slab::RgbSlab;

/// Encode an RGB slab as lossy WebP at `quality` (0..=100). Matches the
/// production DZP tile codec (WebP q80).
pub fn encode_webp(slab: &RgbSlab, quality: f32) -> Result<Vec<u8>, TilerError> {
    let encoder = webp::Encoder::from_rgb(&slab.data, slab.w, slab.h);
    let mem = encoder.encode(quality);
    Ok(mem.to_vec())
}

/// Encode an RGB slab as lossless WebP.
pub fn encode_webp_lossless(slab: &RgbSlab) -> Result<Vec<u8>, TilerError> {
    let encoder = webp::Encoder::from_rgb(&slab.data, slab.w, slab.h);
    let mem = encoder.encode_lossless();
    Ok(mem.to_vec())
}

/// How to encode an 8-bit raw raster (NIR / TIR) as WebP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gray8Encoding {
    /// Exact, bit-for-bit. Largest; use for calibrated analysis bands.
    Lossless,
    /// WebP near-lossless preprocessing, level `0..=100` (100 = exact,
    /// lower = smaller). Decodes as an ordinary lossless WebP — no special
    /// decoder — but bounds the per-pixel error, roughly halving size at a
    /// small controllable error. The recommended default for NIR / TIR.
    NearLossless(u8),
    /// Lossy WebP at quality `0..=100`. Smallest; display bands only.
    Lossy(f32),
}

/// Encode an 8-bit single-channel plane as WebP. libwebp has no luma input, so
/// the value is written to all three channels; under lossless the two
/// redundant planes cost almost nothing.
pub fn encode_webp_gray8(gray: &[u8], w: u32, h: u32, mode: Gray8Encoding) -> Result<Vec<u8>, TilerError> {
    let mut rgb = vec![0u8; gray.len() * 3];
    for (v, px) in gray.iter().zip(rgb.chunks_exact_mut(3)) {
        px[0] = *v;
        px[1] = *v;
        px[2] = *v;
    }
    let encoder = webp::Encoder::from_rgb(&rgb, w, h);
    let mem = match mode {
        Gray8Encoding::Lossless => encoder.encode_lossless(),
        Gray8Encoding::Lossy(q) => encoder.encode(q),
        Gray8Encoding::NearLossless(level) => {
            let mut config = webp::WebPConfig::new().map_err(|_| TilerError::Io("webp config init".into()))?;
            config.lossless = 1;
            config.quality = 100.0;
            config.near_lossless = level.min(100) as i32;
            encoder
                .encode_advanced(&config)
                .map_err(|e| TilerError::Io(format!("webp near-lossless: {e:?}")))?
        }
    };
    Ok(mem.to_vec())
}

/// Crop one tile out of a level slab. Edge tiles are smaller than `tile_size`.
pub fn crop_tile(level: &RgbSlab, col: u32, row: u32, tile_size: u32) -> RgbSlab {
    let x0 = col * tile_size;
    let y0 = row * tile_size;
    let tw = tile_size.min(level.w - x0);
    let th = tile_size.min(level.h - y0);
    let mut out = RgbSlab::new(tw, th);
    for r in 0..th {
        let src = (((y0 + r) as usize) * level.w as usize + x0 as usize) * 3;
        let dst = (r as usize) * tw as usize * 3;
        out.data[dst..dst + tw as usize * 3].copy_from_slice(&level.data[src..src + tw as usize * 3]);
    }
    out
}
