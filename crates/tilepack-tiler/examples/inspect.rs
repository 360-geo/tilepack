//! Dump a tilepack's structure and run structural integrity checks.
//!
//! `cargo run -p tilepack-tiler --example inspect -- path/to/file.tpc`

use std::process::exit;

use tilepack::{Codec, TilepackView};
use tilepack_tiler::sniff_codec;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: inspect <file.tpc>");
        exit(2);
    };
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        exit(1);
    });

    let view = match TilepackView::new(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("parse: {e}");
            exit(1);
        }
    };
    let fm = &view.fm;
    let h = &fm.header;

    println!("file:        {path} ({} bytes)", bytes.len());
    println!("kind:        {}", if h.face_count == 6 { "cubemap" } else { "planar" });
    println!(
        "root:        {}x{}  tile_size {}  levels {}",
        h.root_w, h.root_h, h.tile_size, h.levels
    );
    println!("total tiles: {}", fm.layout.total_tiles());
    println!("groups:      {}", h.group_count);

    for (i, g) in fm.layout.groups().iter().enumerate() {
        let levels = fm.layout.group_levels(i);
        println!(
            "  [{i}] {:?} {:?} {:?} level_count={} levels={:?} untiled={} nearest={}",
            g.semantic,
            g.codec,
            g.sample,
            g.level_count,
            levels,
            g.flags.untiled(),
            g.flags.nearest_downsample(),
        );
        if let Some(u) = g.radiometry.unit_str() {
            if !u.is_empty() {
                println!(
                    "      radiometry: value = count * {} + {}  unit={u:?}  nodata={}  window {}..{}",
                    g.radiometry.scale, g.radiometry.offset, g.radiometry.nodata, g.radiometry.min, g.radiometry.max
                );
            }
        }
    }

    // Structural integrity: per group, count present tiles and confirm each
    // present tile's blob magic matches the group codec (JPEG/WebP only).
    let mut problems = 0usize;
    for gi in 0..fm.layout.groups().len() {
        let codec = fm.layout.groups()[gi].codec;
        let run = fm.layout.group_run(gi);
        let mut present = 0usize;
        for ord in run.clone() {
            if let Some(blob) = view.tile_by_ordinal(ord) {
                present += 1;
                if matches!(codec, Codec::Jpeg | Codec::Webp) {
                    match sniff_codec(blob) {
                        Some(c) if c == codec => {}
                        other => {
                            problems += 1;
                            if problems <= 5 {
                                eprintln!("  ! group {gi} ordinal {ord}: blob magic {other:?} != group codec {codec:?}");
                            }
                        }
                    }
                }
            }
        }
        println!("  group {gi}: {present}/{} tiles present", run.len());
    }

    if problems == 0 {
        println!("integrity: OK");
    } else {
        eprintln!("integrity: {problems} problem(s)");
        exit(1);
    }
}
