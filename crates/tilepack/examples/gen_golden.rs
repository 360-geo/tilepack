//! Regenerate the golden fixture at `fixtures/golden.tpc`.
//!
//! Run with `cargo run -p tilepack --example gen_golden`. The bytes are
//! deterministic; `tests/golden.rs` asserts they never change.

use std::path::Path;

use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::{Writer, WriterParams};

fn main() {
    let bytes = golden_bytes();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden.tpc");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    println!("wrote {} bytes to {}", bytes.len(), path.display());
}

/// The golden file: a 2-level 128px cubemap with an RGB group (full pyramid,
/// webp) and a depth group (untiled depthpack anchored at the coarse 64 px
/// level, `level_skip = 1` — the lower-resolution-band shape). Tile blobs are
/// deterministic stand-ins, not real codec output — this fixture exercises
/// the container, not the codecs.
pub fn golden_bytes() -> Vec<u8> {
    let params = WriterParams {
        face_count: 6,
        levels: 2,
        tile_size: 64,
        root_w: 128,
        root_h: 128,
    };
    let groups = vec![
        GroupDescriptor {
            semantic: Semantic::Rgb,
            codec: Codec::Webp,
            sample: SampleType::Rgb8,
            flags: GroupFlags::default(),
            level_count: 2,
            level_skip: 0,
            radiometry: Radiometry::default(),
        },
        GroupDescriptor {
            semantic: Semantic::Depth,
            codec: Codec::Depthpack,
            sample: SampleType::U16,
            flags: GroupFlags::new(true, false),
            level_count: 1,
            level_skip: 1,
            radiometry: Radiometry {
                scale: 0.001,
                offset: 0.0,
                nodata: 0,
                min: 0,
                max: 65000,
                unit: Radiometry::unit_from_str("m"),
            },
        },
    ];
    let mut w = Writer::new(params, groups).unwrap();
    let total = w.total_tiles();
    for ord in 0..total {
        // A short deterministic blob per tile; leave every 5th tile absent.
        if ord % 5 == 4 {
            continue;
        }
        let blob: Vec<u8> = (0..=(ord as u8 % 11)).map(|i| i.wrapping_mul(37).wrapping_add(ord as u8)).collect();
        w.set_ordinal(ord, blob).unwrap();
    }
    w.finish().unwrap()
}
