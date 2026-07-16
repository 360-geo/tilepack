//! Building a mip pyramid by successive halving with `fast_image_resize`
//! (SIMD-accelerated Lanczos), coarse levels resampled from the next finer
//! level rather than from the root.

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::TilerError;
use crate::slab::RgbSlab;

/// Resize an RGB slab to `dw x dh` with Lanczos3.
pub fn resize_rgb(src: &RgbSlab, dw: u32, dh: u32) -> Result<RgbSlab, TilerError> {
    if (dw, dh) == (src.w, src.h) {
        return Ok(src.clone());
    }
    let src_img =
        Image::from_vec_u8(src.w, src.h, src.data.clone(), PixelType::U8x3).map_err(|e| TilerError::Io(format!("resize src: {e}")))?;
    let mut dst = Image::new(dw, dh, PixelType::U8x3);
    let mut resizer = Resizer::new();
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    resizer
        .resize(&src_img, &mut dst, &opts)
        .map_err(|e| TilerError::Io(format!("resize: {e}")))?;
    Ok(RgbSlab::from_data(dw, dh, dst.into_vec()))
}

/// Build every pyramid level from the finest, given per-level target dims
/// (index = level, `dims[levels-1]` must equal the finest slab's size).
/// Returns one slab per level, coarse to fine. Coarser levels are resampled
/// from the next finer level, not the root.
pub fn build_pyramid(finest: RgbSlab, dims: &[(u32, u32)]) -> Result<Vec<RgbSlab>, TilerError> {
    let levels = dims.len();
    assert!(levels >= 1);
    assert_eq!(dims[levels - 1], (finest.w, finest.h), "finest dims must match dims.last()");

    let mut out: Vec<Option<RgbSlab>> = (0..levels).map(|_| None).collect();
    out[levels - 1] = Some(finest);
    for l in (0..levels - 1).rev() {
        let finer = out[l + 1].as_ref().unwrap();
        let (dw, dh) = dims[l];
        out[l] = Some(resize_rgb(finer, dw, dh)?);
    }
    Ok(out.into_iter().map(|s| s.unwrap()).collect())
}
