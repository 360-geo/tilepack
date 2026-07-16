# tilepack

Single-file, header-first container for tiled multi-band image
pyramids, designed for HTTP range reads from immutable object storage.

One tilepack holds one photographic asset — a street-level panorama as
a cubemap, or a planar photo such as an oblique aerial capture — as one
or more **band groups**: RGB imagery, near-infrared, thermal, depth.
Each group is a pyramid of independently fetchable tiles; raw-value
bands carry their radiometric mapping (scale, offset, unit, nodata) so
viewers can window and colormap on the GPU instead of shipping baked
display pixels.

## Why not DZI-in-ZIP, COG, PMTiles, or KTX2

The classic Deep Zoom approach — a DZI tile tree inside a ZIP — puts
the directory at the end of the file and stores data offsets only in
per-file local headers, so a web client pays a HEAD request plus a tail
fetch before the first tile, and two range requests for every tile
after that. Tile pixel sizes aren't recorded anywhere, level dimensions
round unevenly, and the pyramid must be reverse-engineered from file
path strings.

tilepack inverts that: a fixed header, the band descriptors, and a
dense offset index sit at the front, so one small range request plans
everything, every tile is one request, and all tile dimensions are
exact from the header alone.

Cloud Optimized GeoTIFF is the right wheel for map-space rasters —
orthomosaics, elevation models — and tilepack does not compete there.
It fails for camera-space photo assets on structure, not metadata:
TIFF has no cube faces, compression is a per-image property so one
file cannot mix JPEG color with a depth codec, and the best raster
codec TIFF offers for depth (LERC) is twice the size of
[depthpack](https://github.com/360-geo/depthpack) lossless. A COG
carrying private compression codes is unreadable to the GIS ecosystem
anyway — and a cube face or an oblique frame has a pose, not a
geotransform, so that ecosystem has nothing to open it *with*. A COG
only your own reader consumes is just a custom format wearing TIFF's
parsing burden.

[PMTiles](https://github.com/protomaps/PMTiles) solves the same
transport problem for z/x/y map tiles but has no cube faces, no bands,
and no radiometry. KTX2 handles cubemaps and mip chains but is a GPU
texture container, not a streaming-tile one. tilepack is the small
format in between, built for controlling both the writer and the
reader.

## Format

```text
┌────────────────────────────────────┐
│ header             24 bytes        │
│ group descriptors  48 × group_count│
│ tile index         8 × (tiles + 1) │
│ tile blobs         …               │
└────────────────────────────────────┘
```

Tile payloads are ordinary codec blobs: JPEG or WebP for display
imagery, lossless split-16 WebP for continuous raw fields like
near-infrared and thermal, and
[depthpack](https://github.com/360-geo/depthpack) for depth. Blobs are
stored group-major and coarse-first, so streaming one band at one
level is a single contiguous range, and adding a band group to an
existing file never re-encodes or reorders existing tiles.

The full byte-level layout, pyramid math, cubemap convention, codec
rules, and writer requirements are in [SPEC.md](SPEC.md).

## Status

Draft specification, version 1. Reference implementation (converter
and repack tooling) in progress.

## Related

- [360-geo/depthpack](https://github.com/360-geo/depthpack) — the
  self-describing u16 raster codec tilepack uses for depth bands.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms
or conditions.
