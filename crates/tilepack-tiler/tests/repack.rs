//! Repack synthesizes then round-trips Deep Zoom archives losslessly.

use std::io::Write;

use tilepack::layout::{Face, TileLoc};
use tilepack::{Codec, TilepackView};
use tilepack_tiler::repack::{RepackKind, RepackOptions, repack};

/// A fake but magic-valid WebP blob, unique per tile so byte-equality is
/// meaningful. Repack never decodes it.
fn fake_webp(tag: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(b"WEBP");
    v.extend_from_slice(tag.as_bytes());
    v
}

fn dzi_xml(tile_size: u32, w: u32, h: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Image xmlns="http://schemas.microsoft.com/deepzoom/2008" TileSize="{tile_size}" Overlap="0" Format="webp">
    <Size Width="{w}" Height="{h}"/>
</Image>"#
    )
}

/// dzp's per-face level count: ceil(sqrt(face / tile)) + 1.
fn dzp_levels(face: u32, tile: u32) -> u32 {
    ((face as f64 / tile as f64).sqrt().ceil() as u32) + 1
}

fn level_dim(root: u32, finest: u32, level: u32) -> u32 {
    let shift = finest - level;
    root.div_ceil(1 << shift)
}

/// Build a DZP zip: 6 faces, each a DZI pyramid of fake webp tiles.
fn build_dzp(face_size: u32, tile: u32) -> (Vec<u8>, u32) {
    let mut buf = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let levels = dzp_levels(face_size, tile);
        let finest = levels - 1;
        for face in ["f", "b", "l", "r", "d", "u"] {
            zw.start_file(format!("{face}.dzi"), opts).unwrap();
            zw.write_all(dzi_xml(tile, face_size, face_size).as_bytes()).unwrap();
            for level in 0..levels {
                let dim = level_dim(face_size, finest, level);
                let cols = dim.div_ceil(tile);
                let rows = dim.div_ceil(tile);
                for row in 0..rows {
                    for col in 0..cols {
                        zw.start_file(format!("{face}_files/{level}/{col}_{row}.webp"), opts).unwrap();
                        zw.write_all(&fake_webp(&format!("{face}-{level}-{col}-{row}"))).unwrap();
                    }
                }
            }
        }
        zw.finish().unwrap();
    }
    (buf, dzp_levels(face_size, tile))
}

#[test]
fn dzp_repack_is_lossless() {
    let face = 1024;
    let tile = 512;
    let (zip_bytes, dzi_levels) = build_dzp(face, tile);

    let (tpc, report) = repack(&zip_bytes, &RepackOptions::default()).unwrap();

    assert_eq!(report.kind, RepackKind::Dzp);
    assert_eq!(report.codec, Codec::Webp);
    assert_eq!((report.root_w, report.root_h), (face, face));
    assert_eq!(report.tile_size, tile as u16);
    // tilepack keeps 1024 and 512 (2 levels); dzp had 3 (incl. 256).
    assert_eq!(report.levels, 2);
    assert_eq!(report.dropped_coarse_levels, dzi_levels - 2);
    assert_eq!(report.tiles_absent, 0);

    let view = TilepackView::new(&tpc).unwrap();
    let finest_dzi = dzi_levels - 1;

    // Every tilepack tile equals the source Deep Zoom tile it maps from.
    for face_letter in ["f", "b", "l", "r", "d", "u"] {
        let face_enum = match face_letter {
            "f" => Face::Front,
            "b" => Face::Back,
            "l" => Face::Left,
            "r" => Face::Right,
            "d" => Face::Down,
            "u" => Face::Up,
            _ => unreachable!(),
        };
        for tp_level in 0..report.levels {
            let dzi_level = finest_dzi - (report.levels as u32 - 1 - tp_level as u32);
            let dim = level_dim(face, finest_dzi, dzi_level);
            let cols = dim.div_ceil(tile);
            let rows = dim.div_ceil(tile);
            for row in 0..rows {
                for col in 0..cols {
                    let loc = TileLoc::new(0, tp_level, face_enum, row, col);
                    let got = view.tile(loc).expect("present tile");
                    let expected = fake_webp(&format!("{face_letter}-{dzi_level}-{col}-{row}"));
                    assert_eq!(got, expected.as_slice(), "{face_letter} tp_level {tp_level} {col},{row}");
                }
            }
        }
    }
}

#[test]
fn dzp_repack_power_of_two_face() {
    // 2048 face, tile 512: dzp sqrt levels = ceil(sqrt(4))+1 = 3 (512,1024,2048).
    // tilepack levels_for = 3 (2048,1024,512). No coarse levels dropped.
    let (zip_bytes, _) = build_dzp(2048, 512);
    let (tpc, report) = repack(&zip_bytes, &RepackOptions::default()).unwrap();
    assert_eq!(report.levels, 3);
    assert_eq!(report.dropped_coarse_levels, 0);
    assert_eq!(report.tiles_absent, 0);
    let view = TilepackView::new(&tpc).unwrap();
    // 6 faces * (1 + 4 + 16) tiles = 126.
    assert_eq!(view.fm.layout.total_tiles(), 126);
}
