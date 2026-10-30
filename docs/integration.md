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
    (libwebp via `cc`), `fearless_simd`, `rayon`, and `depthpack`.
  - `turbojpeg` (opt-in) — swaps JPEG decode to libjpeg-turbo. Faster than
    the pure-Rust `zune-jpeg`, but needs a C toolchain plus `nasm`. Native
    ingest wants it.

The `webp` encoder is libjpeg-turbo-adjacent: there is no pure-Rust lossy
WebP encoder, so the default `convert` build already links libwebp (a
`cc`-only build, no cmake/nasm). Only the core `tilepack` crate is
wasm-safe; the tiler is native-only.

## Ingest service

The ingest service's panorama and planar queue workers call the legacy DZP
converter today. The replacement is a bytes-in, bytes-out call inside the
existing `spawn_blocking`:

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

The converter is stateless (no shared face-size cache like the legacy
converter's), so one global `rayon` pool sized to
the pod's CPUs is all the setup it needs. Output content type is up to the
service; the file is self-describing from its header.

Build the ingest image with `--features convert,turbojpeg` and, for depth,
`depthpack`'s `zstd-c` feature (see below).

## Depth producer

The depth producer currently writes a bespoke `mm`-in-WebP file plus a
`meta.json`. The new boundary: it emits a raw `u16` lattice (millimetres,
`0` = nodata) and the tiler owns the container.

```rust
// panorama depth destined for ONE file with the RGB cubemap: pick the RGB
// pyramid level nearest the depth source's native resolution, remap to a
// cubemap at exactly that size, then merge. merge_groups re-anchors the depth
// group onto that level (`level_skip`), pixel-exact co-registered with the
// RGB mip of the same file level.
let face = tilepack_tiler::nearest_level_face_size(rgb_face, rgb_levels, depth_native / 4);
let depth_tpc = tilepack_tiler::convert_depth_cubemap(&u16_slab, face, &DepthOptions::default())?;
let one_file = tilepack_tiler::merge_groups(&rgb_tpc, &depth_tpc)?;

// standalone equirect sibling: one untiled blob, fetched whole for reprojection
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

## Lazy index reads (WMTS / huge planar levels)

A map-tile (WMTS) pipeline can pack a whole zoom level as one single-level
planar tilepack — millions of tiles, an offset index in the tens of MB. A
serving process that extracts tiles on demand never fetches that index:
it caches the ~100-byte prelude per pack, computes the tile ordinal from
the level's known matrix window, and does one 16-byte range read into the
index plus one blob read. Crate support: `Prelude` (parse
header+descriptors, `index_entry_span`, `tile_range_from_entry` with
per-entry validation) and `SparseReader` (the `Read + Seek` twin; the
roundtrip property test holds it equal to the eager reader for every
ordinal). `FrontMatter` remains the right choice whenever the index
comfortably fits — panorama viewers keep using it unchanged.

## Viewer

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

To find depth in a combined file, scan descriptors for `Semantic::Depth`;
`fm.layout.group_levels(g)` gives the file level(s) it covers (for a pano
depth group that is one coarse level — `level_count = 1` with a non-zero
`level_skip`), and `fm.level_range(g, level)` fetches all six faces in one
request. Depth texels at that level are exactly co-registered with the RGB
mip of the same file level.

## Where the time goes

Rough budget for an 8192x4096 panorama (face 2048, ~33 MP encoded at WebP method 4, q80):

| stage | cost | how it is handled |
|-------|------|-------------------|
| JPEG decode | ~250-400 ms zune, ~150 ms turbojpeg | existing codec SIMD |
| equirect to cube remap (6 x 2048^2) | ~30-80 ms | `fearless_simd` kernels, `rayon` per-row + per-face |
| pyramid (fast_image_resize Lanczos) | ~30-80 ms | SSE4/AVX2/NEON in the resizer, `rayon` per-level |
| WebP encode (~33 MP of tiles) | ~3-4 core-seconds | libwebp SIMD, `rayon` per-tile |
| assemble | negligible | single pass |

WebP encode dominates. The engineering that matters is therefore: never
re-resample each level from the root (successive halving instead), and encode
tiles in parallel across all cores — both done. Decode, resize, and encode all
ride mature SIMD in their respective libraries; remap is fully parallelized.

A planar photo has no remap, so its budget is simply decode, pyramid and
encode. For a real 151 MP Leica CityMapper oblique (14192x10640, 801 tiles,
WebP q80, turbojpeg), single-threaded on a Ryzen 7 7700:

| stage | cost | share |
|-------|------|-------|
| JPEG decode | ~0.9 s | ~6%, serial by nature |
| pyramid | ~0.3 s | ~2%, parallel |
| WebP encode | ~13.3 s | ~91%, parallel per tile |

libwebp is the ceiling for planar conversion. Neither AVX-512 nor building the
whole stack for x86-64-v4 changed encode time measurably on that CPU. Converting
photos concurrently in the shared `rayon` pool does help: ~1.5x photos/s over
one at a time on 16 threads, because decode runs serially per photo.

Storage outweighs encode time for these files, so an encoder setting only earns
its place if it does not grow them. Against libwebp method 4 at q80, on four
obliques (one per camera), 16 threads, with size interpolated to equal PSNR:

| encoder | speed | size at equal PSNR |
|---------|-------|--------------------|
| libwebp m6 | 0.32x | -1.9% |
| libwebp m5 | 0.81x | -0.1% |
| libwebp m1 | 2.71x | +3.5% |
| zenwebp 0.4.6 m4 | 0.78x | +1.6% |
| zenwebp 0.4.6 m6 | 0.28x | -0.5% |

`PlanarOptions` therefore trades a little quality instead of size: method 1 at
q79.7 lands on the same size as method 4 at q80 (-0.12% over 12 obliques, three
per camera) at ~2.7x the encode speed, for ~0.35 dB PSNR and SSIM 0.970 against
0.975. No one setting holds that balance on all content, so re-check it when the
source imagery changes. `PanoOptions` uses the same defaults; they were tuned on
obliques and not measured on panoramas.

`zenwebp` is also AGPL-3.0 or commercial, so it is no replacement for libwebp here.

## Hand SIMD

The remap is the one stage with kernels of its own: the coordinate transform
(`face_dir`, then `atan2` twice) and the bilinear sampler, in
`remap::kernel`. One generic body runs at every `fearless_simd` level, 16 `f32`
lanes on AVX-512. `remap::coords` stays the scalar parity oracle, and
`remap_face_scalar` the reference path.

On a Ryzen 7 7700 the kernels take the whole remap from 38 to 6.8 ns/px on one
thread (5.6x). AVX-512 beats AVX2 by 1.6x on the coordinates but only 1.16x on
the whole remap: the sampler's scalar texel fetches now dominate, at ~6 ns/px.
Zen 4 also executes 512-bit operations on 256-bit units, so other CPUs may differ.

`fearless_simd` takes its AVX-512 level only on the Ice Lake feature set
(Ice Lake and newer, Zen 4 and newer). An x86-64-v4 CPU without it, such as
Skylake-SP or Cascade Lake, runs the 8-lane AVX2 path.

`split16` packing is left as a plain byte-strided loop: it autovectorizes and
runs at ~1 GB/s, well below any stage that matters.

## Measuring

The `tiler_bench` example prints per-stage numbers in one process:

```bash
cargo run -p tilepack-tiler --release --features turbojpeg --example tiler_bench -- planar oblique.jpg
```

```bash
cargo run -p tilepack-tiler --release --example tiler_bench -- remap 16384
```

`planar` also sweeps WebP method against size and PSNR, and compares converting
a batch one photo at a time against all at once. `remap` compares the scalar
oracle against the kernels. Track results on fixed fixtures (an 8192x4096 and a
16384x8192 panorama, a large oblique) run over run. Compare against the legacy
converter on the same fixture; it has no timing of its own, so wrap it in a
small harness for the baseline.
