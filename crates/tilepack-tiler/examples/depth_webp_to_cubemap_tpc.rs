//! Convert a legacy packed-mm equirect depth WebP into a cubemap depthpack
//! tilepack. The WebP stores `mm = (R << 8) | G`, `0` = nodata (the layout the
//! production WebP baked-depth path reads today).
//!
//! `cargo run --release -p tilepack-tiler --example depth_webp_to_cubemap_tpc -- depth.webp out.tpc [max_range_m] [face_size]`

use std::process::exit;

use tilepack_tiler::{DepthOptions, Radiometrics, U16Slab, convert_depth_cubemap};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: depth_webp_to_cubemap_tpc <depth.webp> <out.tpc> [max_range_m] [face_size]");
        exit(2);
    };
    let max_range_m: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30.0);

    let bytes = std::fs::read(&input).unwrap_or_else(|e| {
        eprintln!("read {input}: {e}");
        exit(1);
    });
    let decoded = webp::Decoder::new(&bytes).decode().unwrap_or_else(|| {
        eprintln!("decode {input}: not a WebP");
        exit(1);
    });
    let (w, h) = (decoded.width(), decoded.height());
    let px = decoded.as_ref();
    let channels = px.len() / (w as usize * h as usize);
    if channels < 3 {
        eprintln!("expected RGB(A) depth WebP, got {channels} channels");
        exit(1);
    }

    // Unpack mm = (R << 8) | G into a u16 lattice; 0 stays nodata.
    let mut eq = U16Slab::new(w, h);
    for (i, chunk) in px.chunks_exact(channels).enumerate() {
        eq.data[i] = ((chunk[0] as u16) << 8) | chunk[1] as u16;
    }

    let face_size: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(w / 4);
    let opts = DepthOptions {
        // 1 mm lattice read as metres.
        radiometry: Radiometrics {
            scale: 0.001,
            offset: 0.0,
            unit: "m".into(),
            nodata: 0,
            min: 0,
            max: (max_range_m * 1000.0).round().min(65535.0) as u16,
        },
        zstd_level: 9,
    };

    let tpc = convert_depth_cubemap(&eq, face_size, &opts).unwrap_or_else(|e| {
        eprintln!("convert: {e}");
        exit(1);
    });
    std::fs::write(&output, &tpc).unwrap_or_else(|e| {
        eprintln!("write {output}: {e}");
        exit(1);
    });
    println!("depth {w}x{h} -> cubemap face {face_size} -> {output} ({} bytes)", tpc.len());
}
