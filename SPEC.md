# tilepack container format

Version 1, draft. Magic `TPCK`, file extension `.tpc`.

## Overview

A tilepack is a single-file, header-first container for tiled image
pyramids, designed to be read over HTTP range requests from immutable
object storage. One file holds one photographic asset as one or more
**band groups** — RGB imagery, near-infrared, thermal, depth — sharing
one geometry (a planar raster or a cubemap), each group a pyramid of
independently fetchable tiles.

Design goals:

- **Open in one request.** Header, group descriptors, and the complete
  tile index sit at the front of the file, so a single small range
  request yields everything needed to plan all further reads.
- **One request per tile.** The index stores absolute byte offsets;
  no per-tile probing, no directory walking.
- **Band-selective streaming.** Each group's blobs are contiguous,
  each level run inside a group is contiguous. Fetching one band at
  one level is a single coalescible range.
- **Raw values, not just display pixels.** Raw-value bands carry
  sample type, radiometric mapping, and a nodata sentinel, so clients
  can window and colormap on the GPU.
- **Cheap composition.** Adding a band group to an existing file is a
  blob-run concatenation plus a header/index rewrite. Tiles are never
  re-encoded and never reordered.

Non-goals: general archive features, in-place mutation, container-level
compression (tiles are individually compressed), and asset metadata.
Pose, orientation, capture time, and provenance deliberately do **not**
live in the container — they belong to whatever catalog or API serves
the file. Likewise the container does not distinguish a perspective
photo from an equirectangular panorama; a planar tilepack is just a
raster, and projection semantics are asset-level knowledge.

All integers are little-endian. All reserved bytes MUST be written as
zero and ignored by readers.

## Layout

```text
┌────────────────────────────────────┐
│ header             24 bytes        │
│ group descriptors  48 × group_count│
│ tile index         8 × (tiles + 1) │
│ tile blobs         …               │
└────────────────────────────────────┘
```

The tile count is fully computable from the header and descriptors, so
after reading the first 24 bytes a reader knows the exact byte length
of the descriptor table and index.

## Header

| offset | size | field |
|--------|------|-------|
| 0      | 4    | magic `TPCK` |
| 4      | 1    | version, 1 |
| 5      | 1    | face_count — 1 planar, 6 cubemap |
| 6      | 1    | levels — pyramid level count, level 0 is coarsest |
| 7      | 1    | group_count, at least 1 |
| 8      | 2    | tile_size in pixels, e.g. 512 (u16) |
| 10     | 2    | reserved |
| 12     | 4    | root_w — finest-level width in pixels (u32) |
| 16     | 4    | root_h — finest-level height in pixels (u32) |
| 20     | 4    | reserved |

Cubemap files (`face_count = 6`) require `root_w == root_h`; the root
dimensions describe each face.

## Group descriptor

48 bytes per group, `group_count` entries immediately after the header.

| offset | size | field |
|--------|------|-------|
| 0      | 1    | semantic — 0 rgb, 1 nir, 2 tir, 3 depth, 240–255 private use |
| 1      | 1    | codec — 0 jpeg, 1 webp, 2 webp-split16, 3 depthpack |
| 2      | 1    | sample — 0 rgb8, 1 gray8, 2 u16 |
| 3      | 1    | flags — bit 0 untiled, bit 1 nearest downsample |
| 4      | 1    | level_count — 1..=levels, counted from the finest level |
| 5      | 3    | reserved |
| 8      | 8    | scale (f64) |
| 16     | 8    | offset (f64) |
| 24     | 2    | nodata sentinel in counts (u16) |
| 26     | 2    | min — default display window low, in counts (u16) |
| 28     | 2    | max — default display window high, in counts (u16) |
| 30     | 2    | reserved |
| 32     | 8    | unit — ASCII, null-padded, opaque |
| 40     | 8    | reserved |

Radiometry (`scale`, `offset`, `nodata`, `min`, `max`, `unit`) applies
to raw-value groups (`sample` gray8 or u16): physical value =
count × scale + offset, and `unit` labels the result (`"m"`, `"K"`, …)
without ever being interpreted by the container. Display imagery
groups (rgb8) write all radiometry fields as zero.

Counts are u16, so `scale` trades range against precision: a 1 mm
lattice reaches 65.5 m, 1 cm reaches 655 m, 0.01 K reaches 655 K.
Writers pick the coarsest scale whose quantization error sits below
the sensor's own error floor.

For `depthpack` groups the radiometry fields MUST equal the
corresponding fields in every blob's own header; the duplication lets a
client configure units and display windows before fetching any tile.

`level_count = n` means the group covers the finest *n* levels of the
file pyramid, i.e. file levels `levels − n` through `levels − 1`. A
group with `level_count = 1` has only the finest level — typical for
depth, where coarse silhouette-averaged levels would be geometrically
wrong.

The `untiled` flag replaces the tile grid with exactly one blob per
face per level, covering the whole face. Use it when clients always
consume the band whole (for example a panorama depth field fetched once
for reprojection). Untiled trades request count for decode
concentration — one blob for a full-resolution panorama field is a
CPU-scale decode, so single-threaded clients should hand it to a
worker or thread pool rather than decode inline.

## Pyramid geometry

Level `levels − 1` is the finest at `root_w × root_h`. Dimensions at
level `L`:

```text
w(L) = ceil(root_w / 2^(levels − 1 − L))
h(L) = ceil(root_h / 2^(levels − 1 − L))
```

When any group covers level 0, writers MUST choose `levels` such that
`max(w(0), h(0)) <= tile_size` — the coarsest level is a single tile
per face. The primary display group SHOULD cover the full pyramid
(`level_count = levels`) so it always has a single-tile overview. In a
file whose only groups skip the coarse levels (for example a
depth-only sibling with `level_count = 1`), `levels` describes the
notional pyramid the finest level belongs to and the rule is
vacuous. Cubemap writers SHOULD
choose `root_w = tile_size × 2^(levels − 1)` so every level halves
exactly and every tile is square; producers that resample into the cube
anyway (equirect stitching) get this for free.

The tile grid at level `L` is `ceil(w(L) / tile_size)` columns by
`ceil(h(L) / tile_size)` rows. Tiles have no overlap and no padding;
right and bottom edge tiles are smaller. Tile pixel dimensions are
therefore exact from the header alone — clients never need to decode a
tile to learn its size.

## Tile index

Immediately after the descriptors: an array of u64 absolute file
offsets, one per tile in canonical order, plus one final end offset.
The byte length of tile `i` is `offset[i+1] − offset[i]`; zero length
means the tile is absent.

An absent tile is content, not an error: readers render it as nodata
(raw-value groups) or leave it transparent (display groups), and
completeness gates such as "coarse shell loaded" MUST count absent
tiles as satisfied. Display imagery groups SHOULD be dense.

Blobs have no alignment requirement and are stored back to back —
lengths are derived by subtraction, so any padding between blobs would
be miscounted into a tile's length. Alignment buys nothing here: HTTP
ranges are byte-granular and every codec accepts a byte slice.

Canonical order:

```text
for each group, in descriptor order
  for each level the group covers, coarse to fine
    for each face: front, back, left, right, down, up
      for each row, top to bottom
        for each column, left to right
```

Offsets MUST be non-decreasing and blobs MUST be stored in canonical
order. This is what makes group and level runs contiguous, and what
makes composition cheap: appending a group appends one blob run and
rewrites only the header, descriptors, and index.

Order groups by display priority — the primary display band (usually
rgb) first, so a client's opening prefix request also captures that
band's coarsest levels.

## Cubemap convention

Directions are expressed in the panorama's local frame: right-handed,
Z up. How that frame is oriented in the world (heading, pose) is
asset-level metadata outside the container. Pixel `(col, row)` of a
face, with row 0 at the top of the image, maps to face coordinates

```text
a = 2 (col + 0.5) / w − 1
b = 2 (row + 0.5) / h − 1
```

and the view direction per face is

| face  | direction (dx, dy, dz) |
|-------|------------------------|
| front | (−1,  a, −b) |
| back  | ( 1, −a, −b) |
| left  | (−a, −1, −b) |
| right | ( a,  1, −b) |
| down  | ( b,  a, −1) |
| up    | ( b,  a,  1) |

The table is edge-consistent: the front face's right edge (`a = 1`)
equals the right face's left edge (`a = −1`), the front face's top
edge continues onto the up face, and so on around the cube. Writers
producing faces from equirectangular sources MUST verify their
longitude convention against this table end to end — a sign error here
renders panoramas mirrored.

## Codecs

Every blob, whatever its codec, MUST decode to exactly the tile
dimensions implied by the header — clients never decode a tile to
learn its size, so a mismatch is a writer bug and readers MAY treat it
as a hard error.

**0 jpeg.** Baseline JPEG, sRGB display imagery. `sample` rgb8.

**1 webp.** WebP, lossy or lossless; display imagery (rgb8) or 8-bit
raw gray (gray8, radiometry applies).

**2 webp-split16.** Lossless WebP RGB carrying a u16 count per pixel as
`count = R × 256 + G`, with B zero. Lossless WebP is byte-exact through
browser decode paths, and the reconstruction is linear in R and G, so
GPU bilinear filtering of the split channels interpolates counts
correctly — reconstruct first, then window and colormap. That holds
only where the whole 2×2 filter footprint is valid: a nodata sentinel
inside the footprint bleeds into the interpolated count, so groups
that use nodata need nearest sampling or nodata-aware manual
filtering near holes. Suited to continuous fields (near-infrared,
thermal).

**3 depthpack.** One [depthpack](https://github.com/360-geo/depthpack)
blob per tile, each a self-describing `DPCK` unit. Decodes to raw u16
counts or directly to physical f32 with NaN nodata. Suited to fields
with hard discontinuities that must never be interpolated across —
depth above all — where clients sample nearest or apply their own
edge-aware filtering.

## Downsampling rules for writers

- Display imagery: any good resampler (Lanczos and friends).
- Continuous raw fields (nir, tir): mean of valid counts, nodata-aware
  — nodata texels do not contribute, all-nodata footprints stay nodata.
- Discontinuous fields (depth): averaging across silhouettes invents
  values that exist nowhere. Either build no pyramid
  (`level_count = 1`) or set the nearest-downsample flag and decimate.

## HTTP access pattern

Informative. The front matter of a file with `g` groups and `T` total
tiles is exactly

```text
24 + 48 g + 8 (T + 1) bytes
```

A single-group cubemap with 8192 px faces at tile size 512 has
T = 2046 and 16,448 bytes of front matter — just over 16 KB — and a
16384 px cubemap has a ~64 KB index, so clients SHOULD open with a
64 KB range request (`Range: bytes=0-65535`); total file size comes
from the `Content-Range` response header. When the front matter
extends past the opening request, one follow-up range completes it.
Tile blobs themselves rarely ride along in the opening request — a
512 px JPEG tile is tens of kilobytes — but because groups are ordered
by display priority, the primary band's coarsest tiles sit immediately
after the front matter and are the natural first data fetch.

Every tile is exactly one range request at a known offset. Canonical
order keeps a level's tiles contiguous per row, so clients can merge
adjacent index entries into span requests; when doing so, keep the
request granularity consistent between prefetch and demand paths —
byte-identical ranges are what HTTP caches deduplicate.

## Sibling files

Informative. Bands produced by different pipelines may ship as separate
tilepack files next to the primary asset (for example
`photo.tpc` and `photo.depth.tpc`) rather than coupling those pipelines
at write time. Siblings MAY use a different parameterization than the
primary — an equirectangular depth field next to a cubemap color file —
because co-registration is a property of the asset, not the pixel grid.
A later batch job can merge siblings into one file by concatenating
blob runs and rewriting the front matter; tiles are never re-encoded.

## Versioning

The version byte increments on incompatible layout changes. Readers
MUST reject unknown versions and `face_count` values other than 1
and 6. A group with an unknown codec or semantic value is skipped, not
fatal — the index geometry is still computable from its descriptor, so
the remaining groups stay readable and new band types can be added
without breaking old readers. Reserved bytes and unknown flag bits
MUST be ignored and are available for backward-compatible extension.
