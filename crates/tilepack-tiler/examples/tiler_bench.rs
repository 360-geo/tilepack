//! Time the converter stages on this machine, in one process.
//!
//! `tiler_bench remap [photo.jpg|eq_w] [face_size]`: scalar oracle against SIMD
//! kernels, on a photo's central 2:1 band or a synthetic panorama.
//!
//! `tiler_bench planar <photo.jpg|megapixels> [batch]`: the oblique path, stage
//! by stage, then WebP method and batch concurrency trade-offs. Pass a real
//! photo: synthetic content only approximates encode cost.
//!
//! `cargo run -p tilepack-tiler --release --example tiler_bench -- planar photo.jpg`

use std::hint::black_box;
use std::time::{Duration, Instant};

use fearless_simd::{Level, Simd, dispatch, prelude::*};
use rayon::prelude::*;
use tilepack::layout::Face;
use tilepack_tiler::decode::decode_rgb;
use tilepack_tiler::encode::{crop_tile, encode_webp};
use tilepack_tiler::pyramid::build_pyramid;
use tilepack_tiler::remap::coords::face_row_coords;
use tilepack_tiler::remap::kernel::face_row_coords_simd;
use tilepack_tiler::remap::{remap_face, remap_face_scalar};
use tilepack_tiler::{PlanarOptions, RgbSlab, convert_planar, convert_planar_bytes, levels_for};

const RUNS: usize = 3;

type CoordsFn = fn(Face, u32, u32, u32, u32, &mut [f32], &mut [f32]);

/// Deterministic gradient plus noise, so sampling is not trivially predictable.
fn synthetic_equirect(w: u32, h: u32) -> RgbSlab {
    let mut data = vec![0u8; (w as usize) * (h as usize) * 3];
    data.par_chunks_mut(w as usize * 3).enumerate().for_each(|(y, row)| {
        let mut state = 0x9E37_79B9u32 ^ (y as u32).wrapping_mul(0x85EB_CA6B);
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            px[0] = ((x * 255) / w as usize) as u8 ^ (state as u8 & 0x1F);
            px[1] = ((y * 255) / h as usize) as u8 ^ ((state >> 8) as u8 & 0x1F);
            px[2] = (state >> 16) as u8;
        }
    });
    RgbSlab::from_data(w, h, data)
}

/// Aerial-like texture: octaves of smoothed noise plus sensor grain, mirrored
/// to size so no two neighbouring tiles repeat exactly.
fn synthetic_photo(megapixels: f64) -> RgbSlab {
    const SIZE: usize = 2048;
    let mut texture = vec![0f32; SIZE * SIZE * 3];
    let mut state = 0x2545_F491u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as f32 / u32::MAX as f32
    };
    let mut cell = 256usize;
    let mut amplitude = 70.0f32;
    while cell >= 2 {
        let side = SIZE / cell + 2;
        let lattice: Vec<f32> = (0..side * side * 3).map(|_| next() * amplitude).collect();
        for y in 0..SIZE {
            let (gy, ty) = (y / cell, (y % cell) as f32 / cell as f32);
            for x in 0..SIZE {
                let (gx, tx) = (x / cell, (x % cell) as f32 / cell as f32);
                for c in 0..3 {
                    let at = |i: usize, j: usize| lattice[(j * side + i) * 3 + c];
                    let top = at(gx, gy) * (1.0 - tx) + at(gx + 1, gy) * tx;
                    let bot = at(gx, gy + 1) * (1.0 - tx) + at(gx + 1, gy + 1) * tx;
                    texture[(y * SIZE + x) * 3 + c] += top * (1.0 - ty) + bot * ty;
                }
            }
        }
        cell /= 2;
        amplitude *= 0.62;
    }
    let texture: Vec<u8> = texture.iter().map(|v| (v + next() * 6.0).clamp(0.0, 255.0) as u8).collect();

    let (w, h) = (
        (megapixels * 1e6 * 4.0 / 3.0).sqrt() as u32,
        (megapixels * 1e6 * 3.0 / 4.0).sqrt() as u32,
    );
    let mut data = vec![0u8; (w as usize) * (h as usize) * 3];
    let mirror = |v: usize| {
        let m = v % (2 * SIZE);
        if m < SIZE { m } else { 2 * SIZE - 1 - m }
    };
    data.par_chunks_mut(w as usize * 3).enumerate().for_each(|(y, row)| {
        let ty = mirror(y);
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let i = (ty * SIZE + mirror(x + y / 2048 * 509)) * 3;
            px.copy_from_slice(&texture[i..i + 3]);
        }
    });
    RgbSlab::from_data(w, h, data)
}

fn best_of(runs: usize, mut f: impl FnMut()) -> Duration {
    (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

fn row(name: &str, scalar: Duration, simd: Duration, pixels: u64) {
    let ns_px = |d: Duration| d.as_secs_f64() * 1e9 / pixels as f64;
    println!(
        "{name:<20} {:>10.1} {:>8.2} {:>10.1} {:>8.2} {:>7.2}x",
        scalar.as_secs_f64() * 1e3,
        ns_px(scalar),
        simd.as_secs_f64() * 1e3,
        ns_px(simd),
        scalar.as_secs_f64() / simd.as_secs_f64()
    );
}

/// Every remap of all six faces, in whichever rayon pool is current.
fn time_remap(eq: &RgbSlab, face_size: u32, remap: fn(&RgbSlab, Face, u32) -> RgbSlab) -> Duration {
    best_of(RUNS, || {
        for face in Face::ALL {
            black_box(remap(eq, face, face_size));
        }
    })
}

fn time_coords(eq: &RgbSlab, face_size: u32, coords: CoordsFn) -> Duration {
    let (mut sx, mut sy) = (vec![0f32; face_size as usize], vec![0f32; face_size as usize]);
    best_of(RUNS, || {
        for face in Face::ALL {
            for row in 0..face_size {
                coords(face, row, face_size, eq.w, eq.h, &mut sx, &mut sy);
                black_box((&sx, &sy));
            }
        }
    })
}

/// The central 2:1 band of a photo, standing in for a real equirect.
fn equirect_from_photo(path: &str) -> RgbSlab {
    let photo = decode_rgb(&std::fs::read(path).expect("read photo")).expect("decode photo");
    let (w, h) = (photo.w.min(photo.h * 2) & !1, photo.w.min(photo.h * 2) / 2);
    let top = ((photo.h - h) / 2) as usize * photo.w as usize * 3;
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for row in photo.data[top..].chunks_exact(photo.w as usize * 3).take(h as usize) {
        data.extend_from_slice(&row[..w as usize * 3]);
    }
    RgbSlab::from_data(w, h, data)
}

fn remap(args: &[String]) {
    let source = args.first().map(String::as_str).unwrap_or("8192");
    let eq = match source.parse::<u32>() {
        Ok(eq_w) => synthetic_equirect(eq_w, eq_w / 2),
        Err(_) => equirect_from_photo(source),
    };
    let (eq_w, eq_h) = (eq.w, eq.h);
    let face_size = args.get(1).map_or(eq_w / 4, |s| s.parse().expect("numeric face size"));
    let pixels = 6 * face_size as u64 * face_size as u64;

    println!("equirect {eq_w}x{eq_h}, face {face_size}, {}", machine());

    println!(
        "{:<20} {:>10} {:>8} {:>10} {:>8} {:>8}",
        "stage", "scalar ms", "ns/px", "simd ms", "ns/px", "speedup"
    );
    row(
        "coords, 1 thread",
        time_coords(&eq, face_size, face_row_coords),
        time_coords(&eq, face_size, face_row_coords_simd),
        pixels,
    );
    let (scalar, simd) = one_thread().install(|| {
        (
            time_remap(&eq, face_size, remap_face_scalar),
            time_remap(&eq, face_size, remap_face),
        )
    });
    row("remap, 1 thread", scalar, simd, pixels);
    row(
        "remap, all threads",
        time_remap(&eq, face_size, remap_face_scalar),
        time_remap(&eq, face_size, remap_face),
        pixels,
    );
}

/// Level dimensions, coarse to fine: exact `ceil` halving from the root.
fn level_dims(w: u32, h: u32, tile_size: u16) -> Vec<(u32, u32)> {
    let levels = levels_for(w, h, tile_size) as u32;
    (0..levels)
        .rev()
        .map(|shift| (w.div_ceil(1 << shift), h.div_ceil(1 << shift)))
        .collect()
}

fn tiles(pyramid: &[RgbSlab], tile_size: u32) -> Vec<(usize, u32, u32)> {
    let mut out = Vec::new();
    for (level, slab) in pyramid.iter().enumerate() {
        for row in 0..slab.h.div_ceil(tile_size) {
            for col in 0..slab.w.div_ceil(tile_size) {
                out.push((level, col, row));
            }
        }
    }
    out
}

/// PSNR of decoded tiles against their sources, pooled over every tile.
fn psnr(work: &[(usize, u32, u32)], encoded: &[Vec<u8>], pyramid: &[RgbSlab], tile_size: u32) -> f64 {
    let (error, samples) = work
        .par_iter()
        .zip(encoded)
        .map(|(&(level, col, row), bytes)| {
            let source = crop_tile(&pyramid[level], col, row, tile_size);
            let decoded = webp::Decoder::new(bytes).decode().expect("decode tile");
            let error: u64 = source
                .data
                .iter()
                .zip(decoded.iter())
                .map(|(a, b)| (a.abs_diff(*b) as u64).pow(2))
                .sum();
            (error, source.data.len() as u64)
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    10.0 * (255.0 * 255.0 * samples as f64 / error as f64).log10()
}

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

fn stage(name: &str, one: Duration, all: Duration) {
    println!("{name:<22} {:>10.2} {:>10.2} {:>7.2}x", secs(one), secs(all), secs(one) / secs(all));
}

fn planar(args: &[String]) {
    let source = args.first().map(String::as_str).unwrap_or("150");
    let threads = rayon::current_num_threads();
    let batch = args.get(1).map_or(threads.min(8), |s| s.parse().expect("numeric batch"));

    let (photo, jpeg) = match source.parse::<f64>() {
        Ok(megapixels) => (synthetic_photo(megapixels), None),
        Err(_) => {
            let bytes = std::fs::read(source).expect("read photo");
            (decode_rgb(&bytes).expect("decode photo"), Some(bytes))
        }
    };
    let opts = PlanarOptions::default();
    let tile_size = opts.tile_size;
    let dims = level_dims(photo.w, photo.h, tile_size);
    let single = one_thread();

    let kind = if jpeg.is_some() { "real" } else { "SYNTHETIC" };
    println!(
        "{kind} photo {}x{} ({:.0} MP), {} levels, {}",
        photo.w,
        photo.h,
        photo.w as f64 * photo.h as f64 / 1e6,
        dims.len(),
        machine()
    );
    println!("{:<22} {:>10} {:>10} {:>8}", "stage (seconds)", "1 thread", "all", "speedup");

    if let Some(bytes) = &jpeg {
        let d = best_of(RUNS, || {
            black_box(decode_rgb(bytes).unwrap());
        });
        stage("jpeg decode (serial)", d, d);
    }

    let pyramid_once = || {
        let finest = photo.clone();
        let t = Instant::now();
        let pyramid = build_pyramid(finest, &dims).unwrap();
        (t.elapsed(), pyramid)
    };
    let pyramid_all = (0..RUNS).map(|_| pyramid_once().0).min().unwrap();
    let pyramid_one = single.install(|| pyramid_once().0);
    stage("pyramid (lanczos3)", pyramid_one, pyramid_all);

    let pyramid = build_pyramid(photo.clone(), &dims).unwrap();
    let work = tiles(&pyramid, tile_size as u32);
    let encode_all_tiles = || {
        work.par_iter()
            .map(|&(level, col, row)| {
                encode_webp(&crop_tile(&pyramid[level], col, row, tile_size as u32), opts.quality, opts.method)
                    .unwrap()
                    .len()
            })
            .sum::<usize>()
    };
    let t = Instant::now();
    black_box(encode_all_tiles());
    let encode_all = t.elapsed();
    let t = Instant::now();
    black_box(single.install(encode_all_tiles));
    stage("tile encode (webp q80)", t.elapsed(), encode_all);

    let convert_one = || match &jpeg {
        Some(bytes) => convert_planar_bytes(bytes, &opts).unwrap(),
        None => convert_planar(&photo, &opts).unwrap(),
    };
    let t = Instant::now();
    let tpc = convert_one();
    let whole_all = t.elapsed();
    let t = Instant::now();
    single.install(|| black_box(convert_one()));
    stage("whole photo", t.elapsed(), whole_all);
    println!("output {:.1} MB, {} tiles", tpc.len() as f64 / 1e6, work.len());

    println!(
        "\n{:<7} {:>7} {:>9} {:>12} {:>7} {:>7} {:>9}",
        "method", "quality", "encode s", "bytes", "time", "size", "psnr dB"
    );
    let mut baseline = None;
    for (method, quality) in [
        (4, 80.0),
        (3, 80.0),
        (2, 80.0),
        (1, 80.0),
        (0, 80.0),
        (opts.method as i32, opts.quality),
    ] {
        let mut config = webp::WebPConfig::new().unwrap();
        config.quality = quality;
        config.method = method;
        let t = Instant::now();
        let encoded: Vec<Vec<u8>> = work
            .par_iter()
            .map(|&(level, col, row)| {
                let tile = crop_tile(&pyramid[level], col, row, tile_size as u32);
                webp::Encoder::from_rgb(&tile.data, tile.w, tile.h)
                    .encode_advanced(&config)
                    .unwrap()
                    .to_vec()
            })
            .collect();
        let d = t.elapsed();
        let bytes: usize = encoded.iter().map(Vec::len).sum();
        let (base_d, base_bytes) = *baseline.get_or_insert((d, bytes));
        println!(
            "{method:<7} {:>7.0} {:>9.2} {bytes:>12} {:>6.0}% {:>+6.1}% {:>9.2}",
            config.quality,
            secs(d),
            100.0 * secs(d) / secs(base_d),
            100.0 * (bytes as f64 / base_bytes as f64 - 1.0),
            psnr(&work, &encoded, &pyramid, tile_size as u32)
        );
    }

    let t = Instant::now();
    for _ in 0..batch {
        black_box(convert_one());
    }
    let serial = t.elapsed();
    let t = Instant::now();
    (0..batch).into_par_iter().for_each(|_| {
        black_box(convert_one());
    });
    let concurrent = t.elapsed();
    println!("\n{batch} photos, {threads} threads");
    println!(
        "one at a time    {:>8.2} s  {:>6.3} photos/s",
        secs(serial),
        batch as f64 / secs(serial)
    );
    println!(
        "all at once      {:>8.2} s  {:>6.3} photos/s",
        secs(concurrent),
        batch as f64 / secs(concurrent)
    );
}

fn one_thread() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("thread pool")
}

#[inline(always)]
fn lanes<S: Simd>(_: S) -> usize {
    S::f32s::LEN
}

fn machine() -> String {
    let level = Level::new();
    format!(
        "{} threads, {} f32 lanes",
        rayon::current_num_threads(),
        dispatch!(level, simd => lanes(simd))
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("remap") => remap(&args[1..]),
        Some("planar") => planar(&args[1..]),
        _ => {
            eprintln!("usage: tiler_bench remap [photo.jpg|eq_w] [face_size]");
            eprintln!("       tiler_bench planar <photo.jpg|megapixels> [batch]");
            std::process::exit(2);
        }
    }
}
