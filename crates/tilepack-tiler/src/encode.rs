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

/// Encode an 8-bit single-channel plane as WebP. libwebp has no luma input, so
/// the value is written to all three channels; under lossless the two
/// redundant planes cost almost nothing. `quality` `None` is lossless (exact
/// values, for analysis bands); `Some(q)` is lossy (far smaller, for display
/// bands where radiometric exactness is not required).
pub fn encode_webp_gray8(gray: &[u8], w: u32, h: u32, quality: Option<f32>) -> Result<Vec<u8>, TilerError> {
    let mut rgb = vec![0u8; gray.len() * 3];
    for (v, px) in gray.iter().zip(rgb.chunks_exact_mut(3)) {
        px[0] = *v;
        px[1] = *v;
        px[2] = *v;
    }
    let encoder = webp::Encoder::from_rgb(&rgb, w, h);
    let mem = match quality {
        None => encoder.encode_lossless(),
        Some(q) => encoder.encode(q),
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
