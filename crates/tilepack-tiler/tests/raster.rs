//! Depth (depthpack) and NIR/TIR (split16) raster conversion round-trips.

use tilepack::layout::{Face, TileLoc};
use tilepack::{Codec, SampleType, Semantic, TilepackView, split16_unpack_vec};
use tilepack_tiler::{DepthOptions, Radiometrics, RasterOptions, U16Slab, convert_depth_equirect, convert_raster_split16};

fn ramp_u16(w: u32, h: u32, nodata_stripe: bool) -> U16Slab {
    let mut slab = U16Slab::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = if nodata_stripe && x % 7 == 0 {
                0
            } else {
                1 + (x * 10 + y * 3) % 60000
            };
            slab.data[(y as usize) * w as usize + x as usize] = v as u16;
        }
    }
    slab
}

#[test]
fn depth_equirect_roundtrips_through_depthpack() {
    let slab = ramp_u16(320, 160, true);
    let opts = DepthOptions {
        radiometry: Radiometrics {
            scale: 0.001,
            offset: 0.0,
            unit: "m".into(),
            nodata: 0,
            min: 0,
            max: 60000,
        },
        zstd_level: 3,
    };
    let tpc = convert_depth_equirect(&slab, &opts).unwrap();

    let view = TilepackView::new(&tpc).unwrap();
    assert_eq!(view.fm.header.face_count, 1);
    assert_eq!(view.fm.header.levels, 1);
    let g = &view.fm.layout.groups()[0];
    assert_eq!(g.semantic, Semantic::Depth);
    assert_eq!(g.codec, Codec::Depthpack);
    assert_eq!(g.sample, SampleType::U16);
    assert!(g.flags.untiled());
    // Descriptor radiometry mirrors the blob.
    assert_eq!(g.radiometry.scale, 0.001);
    assert_eq!(g.radiometry.unit_str(), Some("m"));

    // The single untiled tile is the whole depth field.
    let blob = view.tile(TileLoc::new(0, 0, Face::Front, 0, 0)).unwrap();
    let scaled = depthpack::decode_scaled(blob).unwrap();
    assert_eq!((scaled.width, scaled.height), (320, 160));

    for (i, &count) in slab.data.iter().enumerate() {
        let got = scaled.values[i];
        if count == 0 {
            assert!(got.is_nan(), "nodata should decode to NaN at {i}");
        } else {
            let want = count as f32 * 0.001;
            assert!((got - want).abs() < 1e-6, "value at {i}: got {got} want {want}");
        }
    }
}

#[test]
fn nir_split16_is_lossless_at_finest_level() {
    let slab = ramp_u16(300, 200, false);
    let opts = RasterOptions {
        tile_size: 128,
        semantic: Semantic::Nir,
        radiometry: Radiometrics {
            scale: 1.0,
            offset: 0.0,
            unit: "".into(),
            nodata: 0,
            min: 0,
            max: 65535,
        },
    };
    let tpc = convert_raster_split16(&slab, &opts).unwrap();

    let view = TilepackView::new(&tpc).unwrap();
    let layout = &view.fm.layout;
    let g = &layout.groups()[0];
    assert_eq!(g.semantic, Semantic::Nir);
    assert_eq!(g.codec, Codec::WebpSplit16);

    // Reconstruct the finest level from its tiles and compare to the source.
    let finest = layout.header().levels - 1;
    let (cols, rows) = layout.grid(finest);
    for row in 0..rows {
        for col in 0..cols {
            let loc = TileLoc::new(0, finest, Face::Front, row, col);
            let blob = view.tile(loc).unwrap();
            let decoded = webp::Decoder::new(blob).decode().expect("decode split16 webp");
            let (tw, th) = layout.tile_dims(0, finest, col, row);
            assert_eq!((decoded.width(), decoded.height()), (tw, th));
            let counts = split16_unpack_vec(&decoded);

            for ty in 0..th {
                for tx in 0..tw {
                    let sx = col * 128 + tx;
                    let sy = row * 128 + ty;
                    let want = slab.data[(sy as usize) * slab.w as usize + sx as usize];
                    let got = counts[(ty as usize) * tw as usize + tx as usize];
                    assert_eq!(got, want, "split16 lossless at {sx},{sy}");
                }
            }
        }
    }
}
