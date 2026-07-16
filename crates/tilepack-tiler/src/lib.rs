//! Native converter, repack, and composition tools for the [tilepack]
//! container format.
//!
//! This crate turns source imagery into tilepack files and losslessly remuxes
//! existing Deep Zoom (`DZP`/`SZI`) archives. It depends on the wasm-safe
//! [`tilepack`] core for the format itself and adds the native-only pieces:
//! ZIP/XML parsing, and (in later phases) decode, equirect→cube remap,
//! pyramid building, and tile encoding.
//!
//! [tilepack]: https://github.com/360-geo/tilepack

use thiserror::Error;

#[cfg(feature = "repack")]
pub mod repack;

#[cfg(feature = "convert")]
pub mod compose;
#[cfg(feature = "convert")]
pub mod decode;
#[cfg(feature = "convert")]
pub mod encode;
#[cfg(feature = "convert")]
pub mod pano;
#[cfg(feature = "convert")]
pub mod planar;
#[cfg(feature = "convert")]
pub mod pyramid;
#[cfg(feature = "convert")]
pub mod raster;
#[cfg(feature = "convert")]
pub mod remap;
#[cfg(feature = "convert")]
pub mod slab;

#[cfg(feature = "convert")]
pub use compose::{merge_groups, strip_finest_levels};
#[cfg(feature = "convert")]
pub use pano::{PanoOptions, convert_equirect, convert_equirect_bytes};
#[cfg(feature = "convert")]
pub use planar::{PlanarOptions, convert_planar, convert_planar_bytes};
#[cfg(feature = "convert")]
pub use raster::{DepthOptions, Radiometrics, RasterOptions, convert_depth_equirect, convert_depth_planar, convert_raster_split16};
#[cfg(feature = "convert")]
pub use slab::{RgbSlab, U16Slab};

/// Errors from tiler operations.
#[derive(Error, Debug)]
pub enum TilerError {
    #[error("archive structure: {0}")]
    Archive(String),

    #[error("Deep Zoom manifest: {0}")]
    Dzi(String),

    #[error("level geometry mismatch: {0}")]
    Geometry(String),

    #[error("mixed tile codecs in one archive; strict repack refuses to re-encode")]
    MixedCodec,

    #[error("tilepack write: {0}")]
    Write(#[from] tilepack::WriteError),

    #[error("io: {0}")]
    Io(String),
}

/// Smallest pyramid level count whose coarsest level fits in one tile, i.e.
/// the number of levels tilepack keeps for a root of `root_w x root_h` at
/// `tile_size`. Matches the writer rule `max(w(0), h(0)) <= tile_size`.
pub fn levels_for(root_w: u32, root_h: u32, tile_size: u16) -> u8 {
    let mut levels = 1u8;
    let (mut w, mut h) = (root_w, root_h);
    while w.max(h) > tile_size as u32 {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        levels += 1;
    }
    levels
}

/// Sniff a tile's image codec from its leading bytes. Returns the tilepack
/// [`Codec`](tilepack::Codec) for JPEG or WebP, or `None` if unrecognized.
pub fn sniff_codec(bytes: &[u8]) -> Option<tilepack::Codec> {
    if bytes.len() >= 3 && bytes[0..3] == [0xFF, 0xD8, 0xFF] {
        return Some(tilepack::Codec::Jpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(tilepack::Codec::Webp);
    }
    None
}
