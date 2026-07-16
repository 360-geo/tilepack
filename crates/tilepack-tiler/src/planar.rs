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
    pub quality: f32,
}

impl Default for PlanarOptions {
    fn default() -> PlanarOptions {
        PlanarOptions {
            tile_size: 512,
            quality: 80.0,
        }
    }
}

/// Convert a decoded planar RGB image into a single-face tilepack pyramid.
pub fn convert_planar(img: &RgbSlab, opts: &PlanarOptions) -> Result<Vec<u8>, TilerError> {
    let tile_size = opts.tile_size;
    let levels = levels_for(img.w, img.h, tile_size);
    let group = GroupDescriptor {
        semantic: Semantic::Rgb,
        codec: Codec::Webp,
        sample: SampleType::Rgb8,
        flags: GroupFlags::default(),
        level_count: levels,
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
    let pyramid = build_pyramid(img.clone(), &dims)?;

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
            let bytes = encode_webp(&tile, opts.quality)?;
            Ok((ordinal, bytes))
        })
        .collect();
    for (ordinal, bytes) in encoded? {
        writer.set_ordinal(ordinal, bytes)?;
    }
    writer.finish().map_err(Into::into)
}

/// Decode a JPEG/PNG planar image and convert it.
pub fn convert_planar_bytes(bytes: &[u8], opts: &PlanarOptions) -> Result<Vec<u8>, TilerError> {
    let img = decode_rgb(bytes)?;
    convert_planar(&img, opts)
}
