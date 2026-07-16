# Integration and performance notes

How the reference implementation slots into the surrounding systems, and
where its time and SIMD actually go.

## Crates and features

- `tilepack` — the format core. Pure Rust, compiles for
  `wasm32-unknown-unknown`, depends only on `thiserror`. Header, descriptor,
  and index parsing; the `Layout` geometry and canonical-order math; the
  `Writer`; and `split16` pack/unpack. No image codecs, no I/O beyond the
  optional `TilepackReader` over `Read + Seek`.
- `tilepack-tiler` — native converter, repack, and composition. Feature
  flags:
  - `repack` (default) — lossless DZP/SZI remux. Pulls in `zip` + `quick-xml`.
  - `convert` (default) — decode, remap, pyramid, encode, depth, split16,
    composition. Pulls in `zune-jpeg`, `png`, `fast_image_resize`, `webp`
    (libwebp via `cc`), `pulp`, `rayon`, and `depthpack`.
  - `turbojpeg` (opt-in) — swaps JPEG decode to libjpeg-turbo. Faster than
    the pure-Rust `zune-jpeg`, but needs a C toolchain plus `nasm`. Native
    ingest wants it.

The `webp` encoder is libjpeg-turbo-adjacent: there is no pure-Rust lossy
WebP encoder, so the default `convert` build already links libwebp (a
`cc`-only build, no cmake/nasm). Only the core `tilepack` crate is
wasm-safe; the tiler is native-only.

## georizon_next ingest

The two Redis queue workers (`GenerateDzpWorker`, `GenerateSziWorker`) call
the external `dzp` crate today. The replacement is a bytes-in, bytes-out call
inside the existing `spawn_blocking`:

```rust
// panorama worker
let tpc = spawn_blocking(move || {
    tilepack_tiler::convert_equirect_bytes(&source_bytes, &PanoOptions::default())
}).await??;
// planar (oblique/nadir) worker — replaces the OpenCV decode path
let tpc = spawn_blocking(move || {
    tilepack_tiler::convert_planar_bytes(&source_bytes, &PlanarOptions::default())
}).await??;
```

The converter is stateless (no shared cache like `dzp`'s
`Arc<RwLock<HashMap>>` face-size cache), so one global `rayon` pool sized to
the pod's CPUs is all the setup it needs. Output content type is up to the
service; the file is self-describing from its header.

Build the ingest image with `--features convert,turbojpeg` and, for depth,
`depthpack`'s `zstd-c` feature (see below).

## Depth from pointcloud-utils

`depthmap-gen` currently writes a bespoke `mm`-in-WebP file plus a
`meta.json`. The new boundary: it emits a raw `u16` lattice (millimetres,
`0` = nodata) and the tiler owns the container.

```rust
// panorama depth: one untiled equirect blob, fetched whole for reprojection
let tpc = tilepack_tiler::convert_depth_equirect(&u16_slab, &DepthOptions::default())?;
// perspective-photo depth: tiled, nearest-decimated pyramid
let tpc = tilepack_tiler::convert_depth_planar(&u16_slab, 512, &DepthOptions::default())?;
```

Both encode through `depthpack`; the descriptor's `scale`/`offset`/`unit`/
`nodata` mirror the blob so a viewer can configure units and windowing before
fetching a tile. For bulk native encoding, enable `depthpack`'s `zstd-c`
feature from the workspace — roughly 2.6x faster encode, smaller blobs; decode
stays pure Rust.

## NIR / TIR bit depth and codec choice

Match the codec to the sample depth:

- 16-bit rasters (`convert_raster_split16`) use `webp-split16` — lossless, the
  full u16 range, filterable after reconstruction.
- 8-bit rasters (`convert_raster_gray8`) use a `gray8` WebP, with the encode
  mode chosen by `RasterOptions.gray8`:
  - `Gray8Encoding::Lossless` — exact, for calibrated analysis bands.
  - `Gray8Encoding::NearLossless(level)` — **the recommended default.** WebP
    near-lossless preprocessing bounds the per-pixel error while the tile still
    decodes as an ordinary lossless WebP, so there is no special decode path.
  - `Gray8Encoding::Lossy(q)` — smallest, display bands only.

Aerial NIR is high entropy, so exact lossless is only ~1.3:1 and no codec
changes that much (JPEG XL lossless, reserved as codec 4, would save ~15% and
is the future exact-lossless upgrade). The real lever is near-lossless. Measured
on a 100 MP nadir NIR band:

| mode | NIR band size | vs lossless |
|------|---------------|-------------|
| lossless | 81 MB | — |
| near-lossless 60 (default) | 52 MB | −36% |
| near-lossless 40 | 45 MB | −45% |

Near-lossless bounds the error per pixel (unlike DCT-lossy, whose error is
spatially-correlated ringing you cannot cap), which is what makes it safe for
NDVI and classification while still shrinking the band. Keep exact lossless only
for DN-level radiometric or change-detection work.

Native 4-band GeoTIFF ingestion is not built in; split the bands upstream (for
example `gdal_translate -b 1 -b 2 -b 3` and `-b 4`) and feed the RGB and NIR
rasters to `convert_planar` and the raster converters, then `merge_groups` them
into one file. The `rgbi` example does exactly this.

## argos viewer

The reader is the wasm-safe `tilepack` core. The app does its own HTTP:

1. Fetch a 64 KB opening prefix.
2. `tilepack::required_len(&prefix)` (or catch `FrontMatter::parse`'s
   `Truncated { needed }`) tells you exactly how many front-matter bytes to
   fetch; refetch if short.
3. `FrontMatter::parse(&prefix)` gives header, descriptors, `Layout`, and the
   offset index.
4. `fm.tile_range(loc)` plans one range request per tile;
   `fm.level_range(g, level)` coalesces a whole level. Absent tiles return
   `None`.

This deletes the whole class of decoded-size inference the DZP/SZI reader
carried (`FaceLevelLayout`, stride inference, `min_display_level`, path
parsing, the LFH probe): every tile dimension is exact from the header.

For depth, `depthpack::decode_scaled_into` writes physical `f32` (NaN nodata)
straight into the GPU upload buffer — no CPU mm-to-metre expansion pass.

## Where the time goes

Rough budget for an 8192x4096 panorama (face 2048, WebP q80, ~33 MP encoded):

| stage | cost | how it is handled |
|-------|------|-------------------|
| JPEG decode | ~250-400 ms zune, ~150 ms turbojpeg | existing codec SIMD |
| equirect to cube remap (6 x 2048^2) | ~100-250 ms | scalar trig, `rayon` per-row + per-face |
| pyramid (fast_image_resize Lanczos) | ~30-80 ms | SSE4/AVX2/NEON in the resizer |
| WebP encode (~33 MP of tiles) | ~3-4 core-seconds | libwebp SIMD, `rayon` per-tile |
| assemble | negligible | single pass |

WebP encode dominates. The engineering that matters is therefore: never
re-resample each level from the root (successive halving instead), and encode
tiles in parallel across all cores — both done. Decode, resize, and encode all
ride mature SIMD in their respective libraries; remap is fully parallelized.

## The one hand-SIMD candidate

The remap coordinate transform (`face_dir` then `atan2`/`acos` to equirect
`(u, v)`) is the only stage that would benefit from a hand-written `pulp`
kernel, following the branchless-polynomial pattern in pointcloud-utils'
`colorizer/src/project/pulp_kernel.rs`. It is deliberately not written yet:
it is a non-dominant stage (encode is the ceiling), the numerics are delicate,
and it needs real before/after benchmarking to justify. The scalar
`remap::coords::face_row_coords` is structured as the parity oracle a future
kernel is checked against, exactly as colorizer checks its kernel against a
scalar reference. Do that work against a real fixture with the perf harness,
not speculatively.

`split16` packing is left as a plain byte-strided loop: it autovectorizes and
runs at ~1 GB/s, well below any stage that matters.

## Measuring

The `convert` example prints wall time. For per-stage numbers, instrument with
`std::time::Instant` around decode / remap+pyramid / encode and compare against
the current `dzp` crate on the same fixture (the `dzp` CLI has no timing, so
wrap it in a small harness for the baseline). Track results on fixed fixtures
(an 8192x4096 and a 16384x8192 panorama, a large oblique) run over run.
