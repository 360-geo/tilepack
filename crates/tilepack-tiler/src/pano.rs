//! Equirectangular panorama to cubemap tilepack conversion.

use rayon::prelude::*;
use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::layout::{Face, TileLoc};
use tilepack::{Writer, WriterParams};

use crate::decode::decode_rgb;
use crate::encode::{crop_tile, encode_webp};
use crate::pyramid::build_pyramid;
use crate::remap::remap_face;
use crate::slab::RgbSlab;
use crate::{TilerError, levels_for};

/// Options for panorama conversion.
#[derive(Debug, Clone)]
pub struct PanoOptions {
    pub tile_size: u16,
    /// WebP quality 0..=100.
    pub quality: f32,
    /// Per-face edge length. Defaults to `equirect_width / 4` (the legacy DZP
    /// convention). Determines pyramid depth and output resolution.
    pub face_size: Option<u32>,
}

impl Default for PanoOptions {
    fn default() -> PanoOptions {
        PanoOptions {
            tile_size: 512,
            quality: 80.0,
            face_size: None,
        }
    }
}

/// Convert a decoded equirectangular panorama into a cubemap tilepack.
pub fn convert_equirect(eq: &RgbSlab, opts: &PanoOptions) -> Result<Vec<u8>, TilerError> {
    if eq.w != eq.h * 2 {
        return Err(TilerError::Geometry(format!("equirect must be 2:1, got {}x{}", eq.w, eq.h)));
    }
    let face_size = opts.face_size.unwrap_or(eq.w / 4).max(1);
    let tile_size = opts.tile_size;
    let levels = levels_for(face_size, face_size, tile_size);

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
        face_count: 6,
        levels,
        tile_size,
        root_w: face_size,
        root_h: face_size,
    };
    let mut writer = Writer::new(params, vec![group])?;
    let layout = writer.layout().clone();

    let dims: Vec<(u32, u32)> = (0..levels).map(|l| layout.level_dims(l)).collect();

    // Remap + build each face's pyramid, faces in parallel.
    let mut face_pyramids: Vec<Vec<RgbSlab>> = Face::ALL
        .par_iter()
        .map(|&face| {
            let finest = remap_face(eq, face, face_size);
            build_pyramid(finest, &dims)
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Flatten every tile into one work list, then crop + encode in parallel —
    // this stage dominates conversion time.
    struct Work {
        ordinal: usize,
        face: usize,
        level: u8,
        col: u32,
        row: u32,
    }
    let mut work = Vec::new();
    for (fi, &face) in Face::ALL.iter().enumerate() {
        for level in 0..levels {
            let (cols, rows) = layout.grid(level);
            for row in 0..rows {
                for col in 0..cols {
                    let loc = TileLoc::new(0, level, face, row, col);
                    let ordinal = layout.tile_ordinal(loc).expect("tile in layout");
                    work.push(Work {
                        ordinal,
                        face: fi,
                        level,
                        col,
                        row,
                    });
                }
            }
        }
    }

    let encoded: Result<Vec<(usize, Vec<u8>)>, TilerError> = work
        .par_iter()
        .map(|w| {
            let level_slab = &face_pyramids[w.face][w.level as usize];
            let tile = crop_tile(level_slab, w.col, w.row, tile_size as u32);
            let bytes = encode_webp(&tile, opts.quality)?;
            Ok((w.ordinal, bytes))
        })
        .collect();

    for (ordinal, bytes) in encoded? {
        writer.set_ordinal(ordinal, bytes)?;
    }

    // Drop the pyramids before serializing to keep peak memory down.
    face_pyramids.clear();
    writer.finish().map_err(Into::into)
}

/// Decode a JPEG/PNG equirect and convert it — the ingest-service entry point
/// (`spawn_blocking(|| convert_equirect_bytes(&bytes, &opts))`).
pub fn convert_equirect_bytes(bytes: &[u8], opts: &PanoOptions) -> Result<Vec<u8>, TilerError> {
    let eq = decode_rgb(bytes)?;
    convert_equirect(&eq, opts)
}
