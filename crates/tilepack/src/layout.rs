//! Pyramid geometry and canonical tile ordering — the math shared by the
//! reader, the writer, and any producer. Every dimension and every tile
//! ordinal is derived from the header plus descriptors; nothing here decodes
//! a tile.

use crate::descriptor::{DESCRIPTOR_LEN, GroupDescriptor};
use crate::error::ParseError;
use crate::header::{HEADER_LEN, Header};

/// A cube face, in canonical order. Planar files use only [`Face::Front`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Front,
    Back,
    Left,
    Right,
    Down,
    Up,
}

impl Face {
    /// All faces in canonical (index) order.
    pub const ALL: [Face; 6] = [Face::Front, Face::Back, Face::Left, Face::Right, Face::Down, Face::Up];

    /// Canonical index `0..6`.
    pub const fn index(self) -> usize {
        match self {
            Face::Front => 0,
            Face::Back => 1,
            Face::Left => 2,
            Face::Right => 3,
            Face::Down => 4,
            Face::Up => 5,
        }
    }

    /// Face for a canonical index, or `None` if out of range.
    pub const fn from_index(i: usize) -> Option<Face> {
        match i {
            0 => Some(Face::Front),
            1 => Some(Face::Back),
            2 => Some(Face::Left),
            3 => Some(Face::Right),
            4 => Some(Face::Down),
            5 => Some(Face::Up),
            _ => None,
        }
    }
}

/// Identity of one tile within a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileLoc {
    pub group: u8,
    pub level: u8,
    pub face: Face,
    pub row: u32,
    pub col: u32,
}

impl TileLoc {
    pub const fn new(group: u8, level: u8, face: Face, row: u32, col: u32) -> TileLoc {
        TileLoc {
            group,
            level,
            face,
            row,
            col,
        }
    }
}

/// A hard cap on tile count for untrusted input. The offset index for this
/// many tiles is 512 MB — far beyond any real asset, and a DoS guard against
/// adversarial headers that imply an enormous index.
const MAX_TILES: u64 = 64 * 1024 * 1024;

/// `ceil(root / 2^shift)`, guarding the `shift >= 32` case where the divisor
/// exceeds any `u32` root and the result is always 1.
fn dim_at(root: u32, shift: u32) -> u32 {
    if shift >= 32 { 1 } else { root.div_ceil(1u32 << shift) }
}

/// Precomputed geometry and ordinal bases for one file. Cheap to build
/// (`O(groups * levels)`, both bounded by 255) and gives `O(1)` tile ordinals.
#[derive(Debug, Clone)]
pub struct Layout {
    header: Header,
    groups: Vec<GroupDescriptor>,
    /// Cumulative tile count before each group. `len == group_count`.
    group_base: Vec<u64>,
    /// Per group, the ordinal base (relative to `group_base[g]`) of each
    /// covered level, indexed by `level - lo(g)`. `len == level_count(g)`.
    level_base: Vec<Vec<u64>>,
    total: u64,
}

impl Layout {
    /// Build the layout from a validated header and its descriptors.
    ///
    /// Fails if the descriptor set is inconsistent with the header, or if the
    /// implied tile count is absurdly large (untrusted-input guard).
    pub fn new(header: Header, groups: Vec<GroupDescriptor>) -> Result<Layout, ParseError> {
        if groups.len() != header.group_count as usize {
            return Err(ParseError::Inconsistent("group_count does not match descriptor count"));
        }
        for g in &groups {
            if g.level_count < 1 || g.level_count > header.levels {
                return Err(ParseError::Inconsistent("level_count out of 1..=levels"));
            }
            if g.level_skip > header.levels - g.level_count {
                return Err(ParseError::Inconsistent("level_skip + level_count exceeds levels"));
            }
        }

        let mut group_base = Vec::with_capacity(groups.len());
        let mut level_base = Vec::with_capacity(groups.len());
        let mut running: u64 = 0;

        for g in &groups {
            group_base.push(running);
            let hi = header.levels - g.level_skip;
            let lo = hi - g.level_count;
            let mut bases = Vec::with_capacity(g.level_count as usize);
            let mut group_running: u64 = 0;
            for level in lo..hi {
                bases.push(group_running);
                let count = level_tile_count(&header, g, level);
                group_running = group_running
                    .checked_add(count)
                    .ok_or(ParseError::Inconsistent("tile count overflow"))?;
                if group_running > MAX_TILES {
                    return Err(ParseError::Inconsistent("implied tile count exceeds the sanity cap"));
                }
            }
            level_base.push(bases);
            running = running
                .checked_add(group_running)
                .ok_or(ParseError::Inconsistent("tile count overflow"))?;
            if running > MAX_TILES {
                return Err(ParseError::Inconsistent("implied tile count exceeds the sanity cap"));
            }
        }

        Ok(Layout {
            header,
            groups,
            group_base,
            level_base,
            total: running,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn groups(&self) -> &[GroupDescriptor] {
        &self.groups
    }

    /// Total number of tiles across every group. The offset index has
    /// `total + 1` entries.
    pub fn total_tiles(&self) -> u64 {
        self.total
    }

    /// Byte length of header + descriptors + index.
    pub fn front_matter_len(&self) -> u64 {
        HEADER_LEN as u64 + (DESCRIPTOR_LEN as u64) * self.header.group_count as u64 + 8 * (self.total + 1)
    }

    /// Composited pixel dimensions of one face at `level` (per-face for
    /// cubemaps). `level` must be `< header.levels`.
    pub fn level_dims(&self, level: u8) -> (u32, u32) {
        let shift = (self.header.levels - 1 - level) as u32;
        (dim_at(self.header.root_w, shift), dim_at(self.header.root_h, shift))
    }

    /// Tile grid `(cols, rows)` of a tiled group at `level`.
    pub fn grid(&self, level: u8) -> (u32, u32) {
        let (w, h) = self.level_dims(level);
        (w.div_ceil(self.header.tile_size as u32), h.div_ceil(self.header.tile_size as u32))
    }

    /// Grid `(cols, rows)` for group `g` at `level`, honoring the untiled
    /// flag (which collapses to `1x1` per face).
    pub fn group_grid(&self, g: usize, level: u8) -> (u32, u32) {
        if self.groups[g].flags.untiled() { (1, 1) } else { self.grid(level) }
    }

    /// Exact pixel dimensions of one tile. Edge tiles on the right/bottom are
    /// smaller than `tile_size`. Untiled groups return the whole level size.
    pub fn tile_dims(&self, g: usize, level: u8, col: u32, row: u32) -> (u32, u32) {
        let (lw, lh) = self.level_dims(level);
        if self.groups[g].flags.untiled() {
            return (lw, lh);
        }
        let ts = self.header.tile_size as u32;
        let tw = ts.min(lw.saturating_sub(col * ts));
        let th = ts.min(lh.saturating_sub(row * ts));
        (tw.max(1), th.max(1))
    }

    /// The inclusive-exclusive range of file levels group `g` covers,
    /// coarse to fine. The window ends `level_skip` levels below the finest.
    pub fn group_levels(&self, g: usize) -> std::ops::Range<u8> {
        let hi = self.header.levels - self.groups[g].level_skip;
        (hi - self.groups[g].level_count)..hi
    }

    /// Number of faces in this file (1 or 6).
    pub fn face_count(&self) -> u32 {
        self.header.face_count as u32
    }

    /// Ordinal range `[start, end)` of every tile in group `g`.
    pub fn group_run(&self, g: usize) -> std::ops::Range<usize> {
        let start = self.group_base[g] as usize;
        let end = if g + 1 < self.group_base.len() {
            self.group_base[g + 1] as usize
        } else {
            self.total as usize
        };
        start..end
    }

    /// Ordinal range `[start, end)` of every tile at `level` within group `g`.
    /// A whole-level contiguous span — the coalesced-fetch unit.
    pub fn level_run(&self, g: usize, level: u8) -> Option<std::ops::Range<usize>> {
        let levels = self.group_levels(g);
        if level < levels.start || level >= levels.end {
            return None;
        }
        let lo = levels.start;
        let base = self.group_base[g] + self.level_base[g][(level - lo) as usize];
        let count = level_tile_count(&self.header, &self.groups[g], level);
        Some(base as usize..(base + count) as usize)
    }

    /// Flat tile ordinal for a location in canonical order, or `None` if the
    /// location is outside the group's covered levels or grid.
    pub fn tile_ordinal(&self, loc: TileLoc) -> Option<usize> {
        let g = loc.group as usize;
        if g >= self.groups.len() {
            return None;
        }
        let levels = self.group_levels(g);
        if loc.level < levels.start || loc.level >= levels.end {
            return None;
        }
        let face_i = loc.face.index() as u32;
        if face_i >= self.face_count() {
            return None;
        }
        let (cols, rows) = self.group_grid(g, loc.level);
        if loc.col >= cols || loc.row >= rows {
            return None;
        }
        let lo = levels.start;
        let base = self.group_base[g] + self.level_base[g][(loc.level - lo) as usize];
        let per_face = (cols as u64) * (rows as u64);
        let within = (face_i as u64) * per_face + (loc.row as u64) * (cols as u64) + loc.col as u64;
        Some((base + within) as usize)
    }

    /// The location a flat ordinal decodes to, or `None` if out of range.
    /// Inverse of [`Layout::tile_ordinal`]; used by writers and tests.
    pub fn ordinal_loc(&self, ordinal: usize) -> Option<TileLoc> {
        let ord = ordinal as u64;
        if ord >= self.total {
            return None;
        }
        // Which group.
        let g = match self.group_base.binary_search(&ord) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let mut rem = ord - self.group_base[g];
        let levels = self.group_levels(g);
        for level in levels {
            let count = level_tile_count(&self.header, &self.groups[g], level);
            if rem < count {
                let (cols, rows) = self.group_grid(g, level);
                let per_face = (cols as u64) * (rows as u64);
                let face_i = (rem / per_face) as usize;
                let in_face = rem % per_face;
                let row = (in_face / cols as u64) as u32;
                let col = (in_face % cols as u64) as u32;
                return Some(TileLoc {
                    group: g as u8,
                    level,
                    face: Face::from_index(face_i)?,
                    row,
                    col,
                });
            }
            rem -= count;
        }
        None
    }
}

/// Tile count for one group at one level: `faces * cols * rows` (or
/// `faces * 1` when untiled).
fn level_tile_count(header: &Header, g: &GroupDescriptor, level: u8) -> u64 {
    let (cols, rows) = if g.flags.untiled() {
        (1u32, 1u32)
    } else {
        let shift = (header.levels - 1 - level) as u32;
        let lw = dim_at(header.root_w, shift);
        let lh = dim_at(header.root_h, shift);
        (lw.div_ceil(header.tile_size as u32), lh.div_ceil(header.tile_size as u32))
    };
    (header.face_count as u64) * (cols as u64) * (rows as u64)
}
