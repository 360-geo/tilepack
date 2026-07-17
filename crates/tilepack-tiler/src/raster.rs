//! Raw-value raster conversion: depth (depthpack) and near-infrared / thermal
//! (webp-split16). Planar for NIR/TIR; depth ships as an untiled equirect
//! sibling, an untiled cubemap, or a tiled planar pyramid.

use rayon::prelude::*;
use tilepack::cube::{col_to_a, row_to_b};
use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::layout::{Face, TileLoc};
use tilepack::{Writer, WriterParams};

#[cfg(feature = "convert")]
use crate::encode::{Gray8Encoding, encode_webp_gray8, encode_webp_lossless};
use crate::remap::coords::face_source_coord;
#[cfg(feature = "convert")]
use crate::slab::RgbSlab;
use crate::slab::U16Slab;
use crate::{TilerError, levels_for};
#[cfg(feature = "convert")]
use tilepack::split16_pack_vec;

/// Physical mapping and windowing for a raw-value raster.
#[derive(Debug, Clone)]
pub struct Radiometrics {
    pub scale: f64,
    pub offset: f64,
    pub unit: String,
    pub nodata: u16,
    pub min: u16,
    pub max: u16,
}

impl Radiometrics {
    fn descriptor(&self) -> Radiometry {
        Radiometry {
            scale: self.scale,
            offset: self.offset,
            nodata: self.nodata,
            min: self.min,
            max: self.max,
            unit: Radiometry::unit_from_str(&self.unit),
        }
    }
}

/// Options for a tiled raw-value raster (NIR / TIR).
#[cfg(feature = "convert")]
#[derive(Debug, Clone)]
pub struct RasterOptions {
    pub tile_size: u16,
    pub semantic: Semantic,
    pub radiometry: Radiometrics,
    /// How the gray8 path encodes tiles. Ignored by split16, which is always
    /// lossless.
    pub gray8: Gray8Encoding,
}

/// Options for a depth raster.
#[derive(Debug, Clone)]
pub struct DepthOptions {
    pub radiometry: Radiometrics,
    /// zstd level for the depthpack entropy stage.
    pub zstd_level: i32,
}

impl Default for DepthOptions {
    fn default() -> DepthOptions {
        DepthOptions {
            radiometry: Radiometrics {
                scale: 0.001,
                offset: 0.0,
                unit: "m".into(),
                nodata: 0,
                min: 0,
                max: 65000,
            },
            zstd_level: 1,
        }
    }
}

/// Crop a `tile_size` tile out of a u16 level. Edge tiles are smaller.
fn crop_u16_tile(level: &U16Slab, col: u32, row: u32, tile_size: u32) -> U16Slab {
    let x0 = col * tile_size;
    let y0 = row * tile_size;
    let tw = tile_size.min(level.w - x0);
    let th = tile_size.min(level.h - y0);
    let mut out = U16Slab::new(tw, th);
    for r in 0..th {
        let src = ((y0 + r) as usize) * level.w as usize + x0 as usize;
        let dst = (r as usize) * tw as usize;
        out.data[dst..dst + tw as usize].copy_from_slice(&level.data[src..src + tw as usize]);
    }
    out
}

/// Nodata-aware mean 2:1 downsample: each dest pixel averages its up-to-4
/// valid source pixels; an all-nodata footprint stays nodata.
#[cfg(feature = "convert")]
fn halve_u16_mean(src: &U16Slab, dw: u32, dh: u32, nodata: u16) -> U16Slab {
    let mut out = U16Slab::new(dw, dh);
    for y in 0..dh {
        for x in 0..dw {
            let mut sum = 0u32;
            let mut n = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (x * 2 + dx).min(src.w - 1);
                    let sy = (y * 2 + dy).min(src.h - 1);
                    let v = src.data[(sy as usize) * src.w as usize + sx as usize];
                    if v != nodata {
                        sum += v as u32;
                        n += 1;
                    }
                }
            }
            out.data[(y as usize) * dw as usize + x as usize] = if n == 0 { nodata } else { (sum / n) as u16 };
        }
    }
    out
}

/// Nearest 2:1 decimation: dest samples the top-left source pixel. Preserves
/// hard discontinuities (depth silhouettes) without inventing values.
fn halve_u16_nearest(src: &U16Slab, dw: u32, dh: u32) -> U16Slab {
    let mut out = U16Slab::new(dw, dh);
    for y in 0..dh {
        for x in 0..dw {
            let sx = (x * 2).min(src.w - 1);
            let sy = (y * 2).min(src.h - 1);
            out.data[(y as usize) * dw as usize + x as usize] = src.data[(sy as usize) * src.w as usize + sx as usize];
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Downsample {
    #[cfg(feature = "convert")]
    Mean(u16),
    Nearest,
}

fn build_u16_pyramid(finest: U16Slab, dims: &[(u32, u32)], mode: Downsample) -> Vec<U16Slab> {
    let levels = dims.len();
    let mut out: Vec<Option<U16Slab>> = (0..levels).map(|_| None).collect();
    out[levels - 1] = Some(finest);
    for l in (0..levels - 1).rev() {
        let finer = out[l + 1].as_ref().unwrap();
        let (dw, dh) = dims[l];
        let coarse = match mode {
            #[cfg(feature = "convert")]
            Downsample::Mean(nodata) => halve_u16_mean(finer, dw, dh, nodata),
            Downsample::Nearest => halve_u16_nearest(finer, dw, dh),
        };
        out[l] = Some(coarse);
    }
    out.into_iter().map(|s| s.unwrap()).collect()
}

/// Convert an NIR or TIR u16 raster into a planar `webp-split16` tilepack.
#[cfg(feature = "convert")]
pub fn convert_raster_split16(slab: &U16Slab, opts: &RasterOptions) -> Result<Vec<u8>, TilerError> {
    let tile_size = opts.tile_size;
    let levels = levels_for(slab.w, slab.h, tile_size);
    let group = GroupDescriptor {
        semantic: opts.semantic,
        codec: Codec::WebpSplit16,
        sample: SampleType::U16,
        flags: GroupFlags::default(),
        level_count: levels,
        level_skip: 0,
        radiometry: opts.radiometry.descriptor(),
    };
    let params = WriterParams {
        face_count: 1,
        levels,
        tile_size,
        root_w: slab.w,
        root_h: slab.h,
    };
    let mut writer = Writer::new(params, vec![group])?;
    let layout = writer.layout().clone();

    let dims: Vec<(u32, u32)> = (0..levels).map(|l| layout.level_dims(l)).collect();
    let pyramid = build_u16_pyramid(slab.clone(), &dims, Downsample::Mean(opts.radiometry.nodata));

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
            let tile = crop_u16_tile(&pyramid[level as usize], col, row, tile_size as u32);
            let rgb = split16_pack_vec(&tile.data);
            let slab = RgbSlab::from_data(tile.w, tile.h, rgb);
            let bytes = encode_webp_lossless(&slab)?;
            Ok((ordinal, bytes))
        })
        .collect();
    for (ordinal, bytes) in encoded? {
        writer.set_ordinal(ordinal, bytes)?;
    }
    writer.finish().map_err(Into::into)
}

/// Convert an 8-bit NIR or TIR raster into a planar `gray8` WebP tilepack.
/// Values must fit in 8 bits; the sample type is `gray8` and radiometry
/// applies. Smaller than split16 for genuinely 8-bit data, which has no
/// high byte to carry.
#[cfg(feature = "convert")]
pub fn convert_raster_gray8(slab: &U16Slab, opts: &RasterOptions) -> Result<Vec<u8>, TilerError> {
    let tile_size = opts.tile_size;
    let levels = levels_for(slab.w, slab.h, tile_size);
    let group = GroupDescriptor {
        semantic: opts.semantic,
        codec: Codec::Webp,
        sample: SampleType::Gray8,
        flags: GroupFlags::default(),
        level_count: levels,
        level_skip: 0,
        radiometry: opts.radiometry.descriptor(),
    };
    let params = WriterParams {
        face_count: 1,
        levels,
        tile_size,
        root_w: slab.w,
        root_h: slab.h,
    };
    let mut writer = Writer::new(params, vec![group])?;
    let layout = writer.layout().clone();

    let dims: Vec<(u32, u32)> = (0..levels).map(|l| layout.level_dims(l)).collect();
    let pyramid = build_u16_pyramid(slab.clone(), &dims, Downsample::Mean(opts.radiometry.nodata));

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
            let tile = crop_u16_tile(&pyramid[level as usize], col, row, tile_size as u32);
            let gray: Vec<u8> = tile.data.iter().map(|&v| v as u8).collect();
            let bytes = encode_webp_gray8(&gray, tile.w, tile.h, opts.gray8)?;
            Ok((ordinal, bytes))
        })
        .collect();
    for (ordinal, bytes) in encoded? {
        writer.set_ordinal(ordinal, bytes)?;
    }
    writer.finish().map_err(Into::into)
}

/// Convert an equirectangular depth raster into an untiled single-level
/// depthpack tilepack — the panorama depth sibling fetched whole for
/// reprojection.
pub fn convert_depth_equirect(slab: &U16Slab, opts: &DepthOptions) -> Result<Vec<u8>, TilerError> {
    // Untiled single blob: pick a tile_size that makes the whole raster one
    // tile so the single-tile geometry rule holds regardless of reading.
    let tile_size =
        u16::try_from(slab.w.max(slab.h)).map_err(|_| TilerError::Geometry("depth raster exceeds 65535 px; tile it instead".into()))?;
    let group = GroupDescriptor {
        semantic: Semantic::Depth,
        codec: Codec::Depthpack,
        sample: SampleType::U16,
        flags: GroupFlags::new(true, true), // untiled + nearest (no averaging)
        level_count: 1,
        level_skip: 0,
        radiometry: opts.radiometry.descriptor(),
    };
    let params = WriterParams {
        face_count: 1,
        levels: 1,
        tile_size,
        root_w: slab.w,
        root_h: slab.h,
    };
    let mut writer = Writer::new(params, vec![group])?;

    let enc = depthpack::EncodeOptions {
        scale: opts.radiometry.scale,
        offset: opts.radiometry.offset,
        unit: opts.radiometry.unit.clone(),
        zstd_level: opts.zstd_level,
    };
    let blob = depthpack::encode(&slab.data, slab.w, slab.h, &enc).map_err(|e| TilerError::Io(format!("depthpack encode: {e}")))?;
    writer.set_ordinal(0, blob)?;
    writer.finish().map_err(Into::into)
}

/// Nearest-sample the equirect u16 at source coords. Longitude wraps at the
/// seam, latitude clamps at the poles. Depth is never interpolated — averaging
/// across a silhouette invents a value that exists nowhere.
fn nearest_u16(eq: &U16Slab, sx: f32, sy: f32) -> u16 {
    let w = eq.w as i64;
    let h = eq.h as i64;
    let x = (sx.round() as i64).rem_euclid(w) as usize;
    let y = (sy.round() as i64).clamp(0, h - 1) as usize;
    eq.data[y * eq.w as usize + x]
}

/// Remap an equirect depth raster to one cube face by nearest sampling, using
/// the production cube convention.
fn remap_depth_face(eq: &U16Slab, face: Face, face_size: u32) -> U16Slab {
    let mut out = U16Slab::new(face_size, face_size);
    out.data.par_chunks_mut(face_size as usize).enumerate().for_each(|(row, dst)| {
        let b = row_to_b(row as u32, face_size);
        for (col, slot) in dst.iter_mut().enumerate() {
            let a = col_to_a(col as u32, face_size);
            let (sx, sy) = face_source_coord(face, a, b, eq.w, eq.h);
            *slot = nearest_u16(eq, sx, sy);
        }
    });
    out
}

/// The face size a derived cube band (depth) must use to sit beside a primary
/// RGB pyramid in one file: the primary's level dimension nearest the band's
/// native resolution. Merging via `merge_groups` then re-anchors the band onto
/// that level (`level_skip`), pixel-exact co-registered with the primary.
pub fn nearest_level_face_size(rgb_face_size: u32, rgb_levels: u8, native: u32) -> u32 {
    (0..rgb_levels)
        .map(|l| {
            let shift = (rgb_levels - 1 - l) as u32;
            if shift >= 32 { 1 } else { rgb_face_size.div_ceil(1 << shift) }
        })
        .min_by_key(|&dim| dim.abs_diff(native))
        .unwrap_or(rgb_face_size)
}

/// Convert an equirectangular depth raster into an untiled **cubemap**
/// depthpack tilepack: 6 nearest-sampled faces, one depthpack blob each, all
/// contiguous so a viewer fetches them in a single range request. As a
/// standalone sibling, `face_size` is typically `equirect_width / 4`; when the
/// file will be merged into an RGB cubemap, pass
/// [`nearest_level_face_size`] so the depth group lands exactly on a primary
/// pyramid level.
pub fn convert_depth_cubemap(eq: &U16Slab, face_size: u32, opts: &DepthOptions) -> Result<Vec<u8>, TilerError> {
    if eq.w != eq.h * 2 {
        return Err(TilerError::Geometry(format!("equirect must be 2:1, got {}x{}", eq.w, eq.h)));
    }
    let tile_size = u16::try_from(face_size).map_err(|_| TilerError::Geometry("depth face exceeds 65535 px".into()))?;
    let group = GroupDescriptor {
        semantic: Semantic::Depth,
        codec: Codec::Depthpack,
        sample: SampleType::U16,
        flags: GroupFlags::new(true, true), // untiled + nearest
        level_count: 1,
        level_skip: 0,
        radiometry: opts.radiometry.descriptor(),
    };
    let params = WriterParams {
        face_count: 6,
        levels: 1,
        tile_size,
        root_w: face_size,
        root_h: face_size,
    };
    let mut writer = Writer::new(params, vec![group])?;
    let layout = writer.layout().clone();

    let enc = depthpack::EncodeOptions {
        scale: opts.radiometry.scale,
        offset: opts.radiometry.offset,
        unit: opts.radiometry.unit.clone(),
        zstd_level: opts.zstd_level,
    };

    let encoded: Result<Vec<(usize, Vec<u8>)>, TilerError> = Face::ALL
        .par_iter()
        .map(|&face| {
            let f = remap_depth_face(eq, face, face_size);
            let blob =
                depthpack::encode(&f.data, face_size, face_size, &enc).map_err(|e| TilerError::Io(format!("depthpack encode: {e}")))?;
            let ordinal = layout.tile_ordinal(TileLoc::new(0, 0, face, 0, 0)).expect("face tile in layout");
            Ok((ordinal, blob))
        })
        .collect();
    for (ordinal, blob) in encoded? {
        writer.set_ordinal(ordinal, blob)?;
    }
    writer.finish().map_err(Into::into)
}

/// Convert a planar depth raster into a tiled depthpack tilepack with a
/// nearest-decimated pyramid — for higher-resolution perspective-photo depth.
pub fn convert_depth_planar(slab: &U16Slab, tile_size: u16, opts: &DepthOptions) -> Result<Vec<u8>, TilerError> {
    let levels = levels_for(slab.w, slab.h, tile_size);
    let group = GroupDescriptor {
        semantic: Semantic::Depth,
        codec: Codec::Depthpack,
        sample: SampleType::U16,
        flags: GroupFlags::new(false, true), // tiled + nearest downsample
        level_count: levels,
        level_skip: 0,
        radiometry: opts.radiometry.descriptor(),
    };
    let params = WriterParams {
        face_count: 1,
        levels,
        tile_size,
        root_w: slab.w,
        root_h: slab.h,
    };
    let mut writer = Writer::new(params, vec![group])?;
    let layout = writer.layout().clone();

    let dims: Vec<(u32, u32)> = (0..levels).map(|l| layout.level_dims(l)).collect();
    let pyramid = build_u16_pyramid(slab.clone(), &dims, Downsample::Nearest);

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

    let enc = depthpack::EncodeOptions {
        scale: opts.radiometry.scale,
        offset: opts.radiometry.offset,
        unit: opts.radiometry.unit.clone(),
        zstd_level: opts.zstd_level,
    };
    let encoded: Result<Vec<(usize, Vec<u8>)>, TilerError> = work
        .par_iter()
        .map(|&(ordinal, level, col, row)| {
            let tile = crop_u16_tile(&pyramid[level as usize], col, row, tile_size as u32);
            let blob = depthpack::encode(&tile.data, tile.w, tile.h, &enc).map_err(|e| TilerError::Io(format!("depthpack encode: {e}")))?;
            Ok((ordinal, blob))
        })
        .collect();
    for (ordinal, bytes) in encoded? {
        writer.set_ordinal(ordinal, bytes)?;
    }
    writer.finish().map_err(Into::into)
}
