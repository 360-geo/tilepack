//! Reading a tilepack: parse front matter from a prefix, then address tiles
//! by byte range or by slice.

use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;

use crate::descriptor::{DESCRIPTOR_LEN, GroupDescriptor};
use crate::error::ParseError;
use crate::header::{HEADER_LEN, Header};
use crate::layout::{Layout, TileLoc};

/// Parsed header, descriptors, layout, and offset index — everything needed
/// to plan tile fetches without touching a single tile blob.
#[derive(Debug, Clone)]
pub struct FrontMatter {
    pub header: Header,
    pub layout: Layout,
    /// Absolute file offsets, `total_tiles + 1` entries.
    offsets: Vec<u64>,
}

/// The smallest prefix length that lets parsing advance past its current
/// point, given the bytes in `prefix`. Grows monotonically across calls:
/// `HEADER_LEN`, then the descriptor-table end, then the full front matter.
///
/// A range-reading client calls this, fetches at least that many bytes, and
/// retries until [`FrontMatter::parse`] succeeds.
pub fn required_len(prefix: &[u8]) -> Result<usize, ParseError> {
    if prefix.len() < HEADER_LEN {
        return Ok(HEADER_LEN);
    }
    let header = Header::parse(prefix)?;
    let descriptors_end = header.descriptors_end();
    if prefix.len() < descriptors_end {
        return Ok(descriptors_end);
    }
    let groups = parse_descriptors(header, prefix)?;
    let layout = Layout::new(header, groups)?;
    front_matter_len_usize(&layout)
}

impl FrontMatter {
    /// Parse the front matter from a prefix of the file. If `prefix` is too
    /// short, returns [`ParseError::Truncated`] with the byte count needed.
    pub fn parse(prefix: &[u8]) -> Result<FrontMatter, ParseError> {
        let header = Header::parse(prefix)?;
        let descriptors_end = header.descriptors_end();
        if prefix.len() < descriptors_end {
            return Err(ParseError::Truncated { needed: descriptors_end });
        }
        let groups = parse_descriptors(header, prefix)?;
        let layout = Layout::new(header, groups)?;

        let fm_len = front_matter_len_usize(&layout)?;
        if prefix.len() < fm_len {
            return Err(ParseError::Truncated { needed: fm_len });
        }

        let count = layout.total_tiles() as usize + 1;
        let mut offsets = Vec::with_capacity(count);
        let mut pos = descriptors_end;
        for _ in 0..count {
            let bytes: [u8; 8] = prefix[pos..pos + 8].try_into().unwrap();
            offsets.push(u64::from_le_bytes(bytes));
            pos += 8;
        }

        // The index must be non-decreasing, and the first blob must start
        // exactly at the end of the front matter (blobs are back to back).
        if offsets[0] != fm_len as u64 {
            return Err(ParseError::BadIndex("first offset does not equal front-matter length"));
        }
        for w in offsets.windows(2) {
            if w[1] < w[0] {
                return Err(ParseError::BadIndex("offsets are not non-decreasing"));
            }
        }

        Ok(FrontMatter { header, layout, offsets })
    }

    /// Byte length of the whole file implied by the index (end of the last
    /// blob).
    pub fn file_len(&self) -> u64 {
        *self.offsets.last().unwrap()
    }

    /// Byte range of a tile, or `None` if the location is invalid or the tile
    /// is absent (zero length).
    pub fn tile_range(&self, loc: TileLoc) -> Option<Range<u64>> {
        let ordinal = self.layout.tile_ordinal(loc)?;
        self.ordinal_range(ordinal)
    }

    /// Byte range of a tile by its flat ordinal, or `None` if absent.
    pub fn ordinal_range(&self, ordinal: usize) -> Option<Range<u64>> {
        let start = self.offsets[ordinal];
        let end = self.offsets[ordinal + 1];
        if start == end { None } else { Some(start..end) }
    }

    /// Byte range spanning every tile at `level` in group `g` — the
    /// coalesced-fetch unit. Empty range if the whole level is absent.
    pub fn level_range(&self, g: usize, level: u8) -> Option<Range<u64>> {
        let run = self.layout.level_run(g, level)?;
        Some(self.offsets[run.start]..self.offsets[run.end])
    }

    /// Whether a tile is present (non-zero length).
    pub fn has_tile(&self, loc: TileLoc) -> bool {
        self.tile_range(loc).is_some()
    }

    /// The offset index. `total_tiles + 1` absolute file offsets.
    pub fn offsets(&self) -> &[u64] {
        &self.offsets
    }
}

/// Parsed header, descriptors, and layout — the prelude BEFORE the offset
/// index. Everything a reader needs to plan **lazy** index lookups: hold
/// these ~100 bytes, and any tile is locatable with one 16-byte read into
/// the index region ([`Prelude::index_entry_span`]) plus one blob read.
///
/// This exists for indexes too large to fetch whole: the index costs 8
/// bytes per tile, and at WMTS scale (a ~9.5M-tile ortho zoom level) that
/// is a ~76 MB [`FrontMatter`] — fine on disk, hostile to a CDN worker
/// that holds one per request. The trade: no upfront index validation
/// (monotonicity, first-offset), so each entry is checked at decode time
/// by [`Prelude::tile_range_from_entry`] instead. Prefer [`FrontMatter`]
/// whenever fetching the whole index is acceptable.
///
/// Parsing needs at most `descriptors_end` bytes; like
/// [`FrontMatter::parse`], a short prefix returns
/// [`ParseError::Truncated`] with the byte count that lets it advance.
#[derive(Debug, Clone)]
pub struct Prelude {
    pub header: Header,
    pub layout: Layout,
}

impl Prelude {
    /// Parse header + descriptors from a prefix of the file (the index and
    /// blobs are not touched).
    pub fn parse(prefix: &[u8]) -> Result<Prelude, ParseError> {
        let header = Header::parse(prefix)?;
        let descriptors_end = header.descriptors_end();
        if prefix.len() < descriptors_end {
            return Err(ParseError::Truncated { needed: descriptors_end });
        }
        let groups = parse_descriptors(header, prefix)?;
        let layout = Layout::new(header, groups)?;
        Ok(Prelude { header, layout })
    }

    /// Byte length of the full front matter (header + descriptors + index)
    /// — also the offset of the first blob, which every valid index entry
    /// must be ≥.
    pub fn front_matter_len(&self) -> u64 {
        self.layout.front_matter_len()
    }

    /// The 16-byte span of the file holding tile `ordinal`'s index entry
    /// (its start offset and, adjacent, its end offset). `None` if the
    /// ordinal is outside the layout. Fetch this span, then decode with
    /// [`Prelude::tile_range_from_entry`].
    pub fn index_entry_span(&self, ordinal: usize) -> Option<Range<u64>> {
        if (ordinal as u64) >= self.layout.total_tiles() {
            return None;
        }
        let at = self.header.descriptors_end() as u64 + ordinal as u64 * 8;
        Some(at..at + 16)
    }

    /// [`Prelude::index_entry_span`] by tile location.
    pub fn index_entry_span_for(&self, loc: TileLoc) -> Option<Range<u64>> {
        self.index_entry_span(self.layout.tile_ordinal(loc)?)
    }

    /// Decode one fetched index entry into the tile's blob range, applying
    /// the per-entry half of the validation [`FrontMatter::parse`] does for
    /// the whole index: the range must start at or after the front matter
    /// and must not run backwards; when the file length is known (HTTP
    /// `Content-Range`, object metadata), it must not run past the end.
    /// `Ok(None)` is an absent tile (zero length — content, not an error).
    pub fn tile_range_from_entry(&self, entry: &[u8; 16], file_len: Option<u64>) -> Result<Option<Range<u64>>, ParseError> {
        let start = u64::from_le_bytes(entry[..8].try_into().unwrap());
        let end = u64::from_le_bytes(entry[8..].try_into().unwrap());
        if end < start {
            return Err(ParseError::BadIndex("entry runs backwards"));
        }
        if start < self.front_matter_len() {
            return Err(ParseError::BadIndex("entry starts inside the front matter"));
        }
        if let Some(len) = file_len
            && end > len
        {
            return Err(ParseError::BadIndex("entry runs past the end of the file"));
        }
        Ok(if start == end { None } else { Some(start..end) })
    }
}

/// A tilepack read lazily from a seekable source: only the [`Prelude`] is
/// parsed up front, and each tile lookup reads its 16-byte index entry and
/// then the blob — three bounded reads total, O(1) memory regardless of
/// index size. The `Read + Seek` twin of the HTTP lazy-range pattern (and
/// its executable specification: the oracle test holds it equal to
/// [`TilepackReader`]).
pub struct SparseReader<R: Read + Seek> {
    prelude: Prelude,
    /// Source length, used to bound entry validation.
    len: u64,
    src: R,
}

impl<R: Read + Seek> SparseReader<R> {
    /// Read and parse the prelude, leaving the index and blobs on the source.
    pub fn new(mut src: R) -> Result<SparseReader<R>, ParseError> {
        let len = src
            .seek(SeekFrom::End(0))
            .map_err(|_| ParseError::Truncated { needed: HEADER_LEN })?;
        let mut buf = vec![0u8; HEADER_LEN];
        read_exact_at(&mut src, 0, &mut buf)?;
        loop {
            match Prelude::parse(&buf) {
                Ok(prelude) => return Ok(SparseReader { prelude, len, src }),
                Err(ParseError::Truncated { needed }) => {
                    buf.resize(needed, 0);
                    read_exact_at(&mut src, 0, &mut buf)?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn prelude(&self) -> &Prelude {
        &self.prelude
    }

    /// Read a tile's compressed bytes into a fresh `Vec`; `None` if the
    /// location is invalid or the tile is absent.
    pub fn read_tile(&mut self, loc: TileLoc) -> Result<Option<Vec<u8>>, ParseError> {
        let Some(ordinal) = self.prelude.layout.tile_ordinal(loc) else {
            return Ok(None);
        };
        self.read_tile_by_ordinal(ordinal)
    }

    /// [`SparseReader::read_tile`] by flat ordinal.
    pub fn read_tile_by_ordinal(&mut self, ordinal: usize) -> Result<Option<Vec<u8>>, ParseError> {
        let Some(span) = self.prelude.index_entry_span(ordinal) else {
            return Ok(None);
        };
        let mut entry = [0u8; 16];
        read_exact_at(&mut self.src, span.start, &mut entry)?;
        let Some(range) = self.prelude.tile_range_from_entry(&entry, Some(self.len))? else {
            return Ok(None);
        };
        let mut buf = vec![0u8; (range.end - range.start) as usize];
        read_exact_at(&mut self.src, range.start, &mut buf)?;
        Ok(Some(buf))
    }
}

/// A tilepack held entirely in memory: address tiles as byte slices.
#[derive(Debug, Clone)]
pub struct TilepackView<'a> {
    pub fm: FrontMatter,
    data: &'a [u8],
}

impl<'a> TilepackView<'a> {
    /// Parse a complete in-memory tilepack. `data` must be the whole file.
    pub fn new(data: &'a [u8]) -> Result<TilepackView<'a>, ParseError> {
        let fm = FrontMatter::parse(data)?;
        if (data.len() as u64) < fm.file_len() {
            return Err(ParseError::Truncated {
                needed: fm.file_len() as usize,
            });
        }
        Ok(TilepackView { fm, data })
    }

    /// The compressed bytes of a tile, or `None` if absent.
    pub fn tile(&self, loc: TileLoc) -> Option<&'a [u8]> {
        let r = self.fm.tile_range(loc)?;
        Some(&self.data[r.start as usize..r.end as usize])
    }

    /// The compressed bytes of a tile by ordinal, or `None` if absent.
    pub fn tile_by_ordinal(&self, ordinal: usize) -> Option<&'a [u8]> {
        let r = self.fm.ordinal_range(ordinal)?;
        Some(&self.data[r.start as usize..r.end as usize])
    }
}

/// A tilepack read from a seekable source, fetching tile bytes on demand.
pub struct TilepackReader<R: Read + Seek> {
    pub fm: FrontMatter,
    src: R,
}

impl<R: Read + Seek> TilepackReader<R> {
    /// Read and parse the front matter, leaving tile blobs on the source.
    pub fn new(mut src: R) -> Result<TilepackReader<R>, ParseError> {
        // Read the header, then grow the buffer to the descriptor end, then to
        // the full front-matter length — the same staged fetch a range client
        // would do.
        let mut buf = vec![0u8; HEADER_LEN];
        read_exact_at(&mut src, 0, &mut buf)?;
        loop {
            match FrontMatter::parse(&buf) {
                Ok(fm) => return Ok(TilepackReader { fm, src }),
                Err(ParseError::Truncated { needed }) => {
                    buf.resize(needed, 0);
                    read_exact_at(&mut src, 0, &mut buf)?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Read a tile's compressed bytes into a fresh `Vec`, or `None` if absent.
    pub fn read_tile(&mut self, loc: TileLoc) -> Result<Option<Vec<u8>>, ParseError> {
        let Some(r) = self.fm.tile_range(loc) else {
            return Ok(None);
        };
        let mut buf = vec![0u8; (r.end - r.start) as usize];
        read_exact_at(&mut self.src, r.start, &mut buf)?;
        Ok(Some(buf))
    }
}

fn read_exact_at<R: Read + Seek>(src: &mut R, at: u64, buf: &mut [u8]) -> Result<(), ParseError> {
    src.seek(SeekFrom::Start(at)).map_err(|_| ParseError::Truncated {
        needed: at as usize + buf.len(),
    })?;
    src.read_exact(buf).map_err(|_| ParseError::Truncated {
        needed: at as usize + buf.len(),
    })?;
    Ok(())
}

fn parse_descriptors(header: Header, prefix: &[u8]) -> Result<Vec<GroupDescriptor>, ParseError> {
    let mut groups = Vec::with_capacity(header.group_count as usize);
    let mut pos = HEADER_LEN;
    for _ in 0..header.group_count {
        groups.push(GroupDescriptor::parse(&prefix[pos..pos + DESCRIPTOR_LEN])?);
        pos += DESCRIPTOR_LEN;
    }
    Ok(groups)
}

fn front_matter_len_usize(layout: &Layout) -> Result<usize, ParseError> {
    let fm_len = layout.front_matter_len();
    usize::try_from(fm_len).map_err(|_| ParseError::Inconsistent("front matter length exceeds address space"))
}
