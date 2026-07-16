//! Losslessly remux a Deep Zoom archive (`.dzp` / `.szi`) into a tilepack.
//!
//! `cargo run -p tilepack-tiler --example repack -- in.dzp out.tpc`

use std::process::exit;

use tilepack_tiler::repack::{RepackOptions, repack};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: repack <in.dzp|in.szi> <out.tpc>");
        exit(2);
    };

    let zip_bytes = std::fs::read(&input).unwrap_or_else(|e| {
        eprintln!("read {input}: {e}");
        exit(1);
    });

    let (tpc, report) = repack(&zip_bytes, &RepackOptions::default()).unwrap_or_else(|e| {
        eprintln!("repack: {e}");
        exit(1);
    });

    std::fs::write(&output, &tpc).unwrap_or_else(|e| {
        eprintln!("write {output}: {e}");
        exit(1);
    });

    println!(
        "{:?} {}x{} tile {} levels {} codec {:?}: {} tiles copied, {} absent, {} coarse levels dropped -> {} ({} bytes)",
        report.kind,
        report.root_w,
        report.root_h,
        report.tile_size,
        report.levels,
        report.codec,
        report.tiles_copied,
        report.tiles_absent,
        report.dropped_coarse_levels,
        output,
        tpc.len(),
    );
    for note in &report.notes {
        println!("  note: {note}");
    }
}
