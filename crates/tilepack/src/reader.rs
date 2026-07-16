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
