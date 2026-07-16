//! Convert an equirectangular JPEG/PNG panorama into a cubemap tilepack.
//!
//! `cargo run -p tilepack-tiler --release --example convert -- pano.jpg out.tpc [quality]`

use std::process::exit;
use std::time::Instant;

use tilepack_tiler::{PanoOptions, convert_equirect_bytes};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: convert <pano.jpg|png> <out.tpc> [quality]");
        exit(2);
    };
    let quality: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(80.0);

    let bytes = std::fs::read(&input).unwrap_or_else(|e| {
        eprintln!("read {input}: {e}");
        exit(1);
    });

    let opts = PanoOptions {
        quality,
        ..Default::default()
    };
    let start = Instant::now();
    let tpc = convert_equirect_bytes(&bytes, &opts).unwrap_or_else(|e| {
        eprintln!("convert: {e}");
        exit(1);
    });
    let elapsed = start.elapsed();

    std::fs::write(&output, &tpc).unwrap_or_else(|e| {
        eprintln!("write {output}: {e}");
        exit(1);
    });
    println!(
        "converted {input} -> {output} ({} bytes) in {:.3}s",
        tpc.len(),
        elapsed.as_secs_f64()
    );
}
