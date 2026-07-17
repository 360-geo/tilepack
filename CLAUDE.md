# tilepack — agent guide

Single-file, header-first container for tiled multi-band image pyramids, read
over HTTP range requests. This repo is both the spec ([SPEC.md](SPEC.md)) and
its reference implementation. Consumed by the argos viewer (reader) and the
ingest pipeline (writer). Sibling repo: [depthpack](https://github.com/360-geo/depthpack)
(the u16 depth codec tilepack embeds).

## Crate layout

- `crates/tilepack` — **the format core. MUST stay `wasm32-unknown-unknown`
  clean and pure Rust (only dep: `thiserror`).** This is what the browser
  viewer embeds. Header/descriptor/index parsing, the `Layout` geometry +
  canonical-order math, the `Writer`, `split16` pack/unpack. No image codecs,
  no I/O beyond the optional `TilepackReader` over `Read + Seek`. Do not add
  dependencies here without a hard reason; never add a C-linked or
  wasm-hostile crate.
- `crates/tilepack-tiler` — native converter, repack, composition. `publish =
  false`. Has the heavy deps (libwebp, turbojpeg, fast_image_resize, pulp,
  depthpack) behind feature flags. Never imported by the viewer.
- `fuzz/` — cargo-fuzz, non-workspace-member (own `[workspace]`).
- `fixtures/golden.tpc` — byte-stable golden file (see below).

## Commands

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check                       # max_width = 140 (rustfmt.toml)
cargo build -p tilepack --release --target wasm32-unknown-unknown   # the wasm gate
cargo test -p tilepack-tiler --features turbojpeg                   # needs nasm + C toolchain
```

All four (test / wasm build / clippy / fmt) must pass — they are the CI gates
(`.github/workflows/ci.yaml`). The wasm gate only builds `-p tilepack`; the
tiler is native-only by design.

Dev tools are examples, not a CLI crate (deliberately — see git history):
`cargo run -p tilepack-tiler --release --example inspect -- file.tpc`, and
`repack` / `convert` / `rgbi` likewise.

## Invariants — do not break these

- **Cube face convention = the production dzp/argos convention** (`front =
  (−1,−a,−b)`, …), encoded once in `crates/tilepack/src/cube.rs` as the single
  source of truth. SPEC.md's table was reconciled to it. The orientation e2e
  test (`tiler/tests/orientation.rs`) validates it end to end — if you touch
  cube math, that test must stay green.
- **Level 0 = coarsest, `levels-1` = finest**, in both the format and the
  argos reader. `Layout` dimensions are exact `ceil`-halving from the root; a
  reader never decodes a tile to learn its size.
- **Canonical tile order** (group-major, coarse→fine, face, row, col) is what
  makes runs contiguous and composition cheap. `Layout::tile_ordinal` /
  `ordinal_loc` are inverses; the oracle test enforces it.
- **Untrusted input**: the parser must never panic or allocate on an
  adversarial count. `Layout::new` caps tile count; `robustness.rs` +
  `fuzz/` guard it. Keep that property when editing parse paths.
- **Codec/semantic registries are additive** — new values never bump the
  format version; readers skip unknown-codec groups. Only layout changes bump
  the version byte.

## Codec choices (learned from real data)

- Display imagery: JPEG or WebP (lossy). Panorama fresh-convert to WebP q80 is
  ~55% smaller than the JPEG-era DZPs; repack of current WebP DZPs is
  lossless and just strips ZIP overhead (~0.3–0.6%).
- **16-bit** NIR/TIR → `webp-split16` (lossless, `convert_raster_split16`).
- **8-bit** NIR/TIR → `gray8` WebP (`convert_raster_gray8`). Real aerial NIR
  is 8-bit; split16 wastes an all-zero high-byte plane. Default to
  `Gray8Encoding::NearLossless(60)` — browser-native decode, bounded error,
  ~36% smaller than lossless (aerial NIR is high-entropy, so exact lossless is
  only ~1.3:1). JPEG XL lossless (codec 4, reserved) is the future exact
  upgrade.
- **Depth** → depthpack blobs (`convert_depth_*`). depthpack is for smooth
  fields; never use it for noisy imagery. Panorama depth is a cube or equirect
  untiled group; perspective depth is tiled + nearest downsample.
- There is **no pure-Rust lossy WebP encoder**, so the `convert` feature links
  libwebp (cc-only build). This is why the tiler is native-only.

## Gotchas

- The golden fixture is byte-stable: `tests/golden.rs` embeds it and asserts
  the `Writer` output never drifts. If you intentionally change the wire
  format or writer, regenerate with `cargo run -p tilepack --example
  gen_golden` and review the diff — do not edit the bytes by hand.
- The writer keeps `Cargo.lock` committed (deterministic CI). depthpack is a
  git dep pinned by rev.
- Downsampling rules matter for correctness: display = any resampler; raw
  continuous (NIR/TIR) = nodata-aware mean; depth = nearest (never average
  across silhouettes). The `raster.rs` builders already encode this.
- `docs/integration.md` is the wiring guide (georizon_next ingest, argos
  reader prefix protocol, pointcloud-utils depth boundary, perf budget). Keep
  it current when the public API changes.
