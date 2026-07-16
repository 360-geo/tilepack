//! Build a multi-band RGBI tilepack from separate RGB and NIR rasters
//! (e.g. bands split out of a 4-band GeoTIFF with `gdal_translate`).
//!
//! `cargo run --release -p tilepack-tiler --example rgbi -- rgb.png nir.png out.tpc [tile_size]`
//!
//! RGB goes through the planar WebP path; NIR (here 8-bit, promoted to u16)
//! through the gray8 raster path with an unused nodata sentinel so no valid
//! pixel is dropped. The two same-geometry groups are merged into one file.

use std::process::exit;
use std::time::Instant;

use tilepack::Semantic;
use tilepack_tiler::decode::decode_rgb;
use tilepack_tiler::{
    Gray8Encoding, PlanarOptions, Radiometrics, RasterOptions, U16Slab, convert_planar, convert_raster_gray8, merge_groups,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(rgb_path), Some(nir_path), Some(out_path)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: rgbi <rgb.png> <nir.png> <out.tpc> [tile_size] [nir_near_lossless]");
        eprintln!("  nir_near_lossless: WebP near-lossless level 0-100 (default 60; 100 = exact lossless)");
        exit(2);
    };
    let tile_size: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(512);
    let near_level: u8 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let nir_encoding = if near_level >= 100 {
        Gray8Encoding::Lossless
    } else {
        Gray8Encoding::NearLossless(near_level)
    };

    let start = Instant::now();

    let rgb = decode_rgb(&read(&rgb_path)).unwrap_or_else(|e| fail("decode rgb", e));
    let rgb_tpc = convert_planar(&rgb, &PlanarOptions { tile_size, quality: 80.0 }).unwrap_or_else(|e| fail("convert rgb", e));
    println!("rgb {}x{} -> {} bytes", rgb.w, rgb.h, rgb_tpc.len());

    // NIR arrives as a grayscale image; decode expands it to RGB, so the R
    // channel carries the NIR value. Promote 8-bit -> u16.
    let nir_rgb = decode_rgb(&read(&nir_path)).unwrap_or_else(|e| fail("decode nir", e));
    if (nir_rgb.w, nir_rgb.h) != (rgb.w, rgb.h) {
        eprintln!("nir dimensions {}x{} != rgb {}x{}", nir_rgb.w, nir_rgb.h, rgb.w, rgb.h);
        exit(1);
    }
    let mut nir = U16Slab::new(nir_rgb.w, nir_rgb.h);
    for (i, px) in nir_rgb.data.chunks_exact(3).enumerate() {
        nir.data[i] = px[0] as u16;
    }
    // 8-bit NIR -> gray8 WebP (no wasted high-byte plane). Sentinel 65535 is
    // unused so every pixel is valid.
    let nir_tpc = convert_raster_gray8(
        &nir,
        &RasterOptions {
            tile_size,
            semantic: Semantic::Nir,
            radiometry: Radiometrics {
                scale: 1.0,
                offset: 0.0,
                unit: String::new(),
                nodata: 65535,
                min: 0,
                max: 255,
            },
            gray8: nir_encoding,
        },
    )
    .unwrap_or_else(|e| fail("convert nir", e));
    println!("nir {}x{} -> {} bytes", nir.w, nir.h, nir_tpc.len());

    let merged = merge_groups(&rgb_tpc, &nir_tpc).unwrap_or_else(|e| fail("merge", e));
    std::fs::write(&out_path, &merged).unwrap_or_else(|e| {
        eprintln!("write {out_path}: {e}");
        exit(1);
    });

    println!(
        "rgbi -> {out_path} ({} bytes, {} + {} merged) in {:.2}s",
        merged.len(),
        rgb_tpc.len(),
        nir_tpc.len(),
        start.elapsed().as_secs_f64()
    );
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        exit(1);
    })
}

fn fail(what: &str, e: tilepack_tiler::TilerError) -> ! {
    eprintln!("{what}: {e}");
    exit(1);
}
