//! Planar RGB conversion — the SZI replacement for oblique / nadir photos.

use rayon::prelude::*;
use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::layout::{Face, TileLoc};
use tilepack::{Writer, WriterParams};

use crate::decode::decode_rgb;
use crate::encode::{crop_tile, encode_webp};
use crate::pyramid::build_pyramid;
use crate::slab::RgbSlab;
use crate::{TilerError, levels_for};

/// Options for planar RGB conversion.
#[derive(Debug, Clone)]
pub struct PlanarOptions {
    pub tile_size: u16,
    /// WebP quality 0..=100.
    pub quality: f32,
    /// libwebp method 0..=6, fastest to smallest.
    pub method: u8,
}

impl Default for PlanarOptions {
    fn default() -> PlanarOptions {
        PlanarOptions {
            tile_size: 512,
            // Tuned on real obliques to match the size of method 4 at q80
            // (-0.12% over 12 photos) at ~2.7x the encode speed, for ~0.35 dB.
            quality: 79.7,
            method: 1,
        }
    }
}

/// Convert a decoded planar RGB image into a single-face tilepack pyramid.
pub fn convert_planar(img: &RgbSlab, opts: &PlanarOptions) -> Result<Vec<u8>, TilerError> {
    convert_owned(img.clone(), opts)
}

/// Decode a JPEG/PNG planar image and convert it.
pub fn convert_planar_bytes(bytes: &[u8], opts: &PlanarOptions) -> Result<Vec<u8>, TilerError> {
    convert_owned(decode_rgb(bytes)?, opts)
}

fn convert_owned(img: RgbSlab, opts: &PlanarOptions) -> Result<Vec<u8>, TilerError> {
    let tile_size = opts.tile_size;
    let levels = levels_for(img.w, img.h, tile_size);
    let group = GroupDescriptor {
        semantic: Semantic::Rgb,
        codec: Codec::Webp,
        sample: SampleType::Rgb8,
        flags: GroupFlags::default(),
        level_count: levels,
        level_skip: 0,
        radiometry: Radiometry::default(),
    };
    let params = WriterParams {
        face_count: 1,
        levels,
        tile_size,
        root_w: img.w,
        root_h: img.h,
    };
    let mut writer = Writer::new(params, vec![group])?;
    let layout = writer.layout().clone();

    let dims: Vec<(u32, u32)> = (0..levels).map(|l| layout.level_dims(l)).collect();
    let pyramid = build_pyramid(img, &dims)?;

    let mut work = Vec::new();
    for level in 0..levels {
        let (cols, rows) = layout.grid(level);
        for row in 0..rows {
            for col in 0..cols {
                let ordinal = layout.tile_ordinal(TileLoc::new(0, level, Face::Front, row, col)).unwrap();
                work.push((ordinal, level, col, row));
            }
        }
    }

    let encoded: Result<Vec<(usize, Vec<u8>)>, TilerError> = work
        .par_iter()
        .map(|&(ordinal, level, col, row)| {
            let tile = crop_tile(&pyramid[level as usize], col, row, tile_size as u32);
            let bytes = encode_webp(&tile, opts.quality, opts.method)?;
            Ok((ordinal, bytes))
        })
        .collect();
    for (ordinal, bytes) in encoded? {
        writer.set_ordinal(ordinal, bytes)?;
    }
    writer.finish().map_err(Into::into)
}
