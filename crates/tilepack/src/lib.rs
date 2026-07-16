//! # tilepack
//!
//! Reader and writer for the [tilepack] container format: a single-file,
//! header-first container for tiled multi-band image pyramids, designed to be
//! read over HTTP range requests from immutable object storage. One file
//! holds one photographic asset (a planar raster or a cubemap panorama) as
//! one or more **band groups** — RGB, near-infrared, thermal, depth — each a
//! pyramid of independently fetchable tiles.
//!
//! This crate is the format core: header, descriptor, and index parsing, the
//! pyramid and canonical-order math ([`Layout`]), a [`Writer`], and the
//! [`split16`] helpers. It is pure Rust and compiles for
//! `wasm32-unknown-unknown` — image decoding and tile encoding live in the
//! caller (the browser, or the native tiler crate). It performs no I/O of its
//! own beyond the optional [`TilepackReader`] convenience over a
//! `Read + Seek` source.
//!
//! ## Reading over range requests
//!
//! ```no_run
//! use tilepack::{FrontMatter, required_len};
//! # fn fetch(_range: std::ops::Range<usize>) -> Vec<u8> { unimplemented!() }
//!
//! // Fetch a generous opening prefix, then let the format tell you exactly
//! // how much front matter it needs.
//! let mut prefix = fetch(0..65_536);
//! loop {
//!     match FrontMatter::parse(&prefix) {
//!         Ok(fm) => {
//!             // fm.tile_range(loc) / fm.level_range(g, level) now plan every fetch.
//!             let _ = fm;
//!             break;
//!         }
//!         Err(tilepack::ParseError::Truncated { needed }) => {
//!             prefix = fetch(0..needed);
//!         }
//!         Err(e) => panic!("bad tilepack: {e}"),
//!     }
//! }
//! # let _ = required_len(&[]);
//! ```
//!
//! [tilepack]: https://github.com/360-geo/tilepack

pub mod cube;
pub mod descriptor;
pub mod error;
pub mod header;
pub mod layout;
pub mod reader;
pub mod split16;
pub mod writer;

pub use descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
pub use error::{ParseError, WriteError};
pub use header::{HEADER_LEN, Header, MAGIC, VERSION};
pub use layout::{Face, Layout, TileLoc};
pub use reader::{FrontMatter, TilepackReader, TilepackView, required_len};
pub use split16::{split16_pack, split16_pack_vec, split16_unpack, split16_unpack_vec};
pub use writer::{Writer, WriterParams};
