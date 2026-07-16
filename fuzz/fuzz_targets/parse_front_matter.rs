//! Fuzz the parser: any input must return Ok/Err without panicking or
//! allocating on an untrusted count. Run with
//! `cargo +nightly fuzz run parse_front_matter`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tilepack::required_len(data);
    if let Ok(view) = tilepack::TilepackView::new(data) {
        // Touch every tile range and a few locations to exercise the index.
        let total = view.fm.layout.total_tiles() as usize;
        for ord in 0..total {
            let _ = view.tile_by_ordinal(ord);
        }
    }
    let _ = tilepack::FrontMatter::parse(data);
});
