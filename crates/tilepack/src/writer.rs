//! Assembling a tilepack from encoded tile blobs.

use std::io::Write;

use crate::descriptor::GroupDescriptor;
use crate::error::WriteError;
use crate::header::{Header, VERSION};
use crate::layout::{Layout, TileLoc};

/// Header-level parameters for a new file. The version is always current.
#[derive(Debug, Clone, Copy)]
pub struct WriterParams {
    /// 1 for planar, 6 for cubemap.
    pub face_count: u8,
    pub levels: u8,
    pub tile_size: u16,
    pub root_w: u32,
    pub root_h: u32,
}

/// Collects encoded tile blobs and serializes a complete tilepack.
///
/// All groups are declared up front, which fixes the layout, so blobs can be
/// filled in any order (`set_tile` / `set_ordinal`) or streamed in canonical
/// order (`push_blob`). Unset tiles are absent (zero length).
pub struct Writer {
    header: Header,
    groups: Vec<GroupDescriptor>,
    layout: Layout,
    blobs: Vec<Vec<u8>>,
    push_cursor: usize,
}

impl Writer {
    /// Create a writer for a file with the given geometry and groups.
    pub fn new(params: WriterParams, groups: Vec<GroupDescriptor>) -> Result<Writer, WriteError> {
        let header = Header {
            version: VERSION,
            face_count: params.face_count,
            levels: params.levels,
            group_count: u8::try_from(groups.len()).map_err(|_| WriteError::InvalidParams("group_count exceeds 255"))?,
            tile_size: params.tile_size,
            root_w: params.root_w,
            root_h: params.root_h,
        };
        // Validate header and each descriptor by serializing them once.
        header.to_bytes()?;
        for g in &groups {
            g.to_bytes(header.levels)?;
        }
        let layout =
            Layout::new(header, groups.clone()).map_err(|_| WriteError::InvalidParams("descriptor set is inconsistent with the header"))?;
        let total = usize::try_from(layout.total_tiles()).map_err(|_| WriteError::InvalidParams("tile count exceeds address space"))?;

        Ok(Writer {
            header,
            groups,
            layout,
            blobs: vec![Vec::new(); total],
            push_cursor: 0,
        })
    }

    /// The layout, for computing ordinals during parallel assembly.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Set a tile's blob by location. Overwrites any previous blob there.
    pub fn set_tile(&mut self, loc: TileLoc, bytes: Vec<u8>) -> Result<(), WriteError> {
        let ordinal = self
            .layout
            .tile_ordinal(loc)
            .ok_or(WriteError::TileOutOfRange("tile location outside layout"))?;
        self.blobs[ordinal] = bytes;
        Ok(())
    }

    /// Set a tile's blob by flat canonical ordinal.
    pub fn set_ordinal(&mut self, ordinal: usize, bytes: Vec<u8>) -> Result<(), WriteError> {
        if ordinal >= self.blobs.len() {
            return Err(WriteError::TileOutOfRange("ordinal outside layout"));
        }
        self.blobs[ordinal] = bytes;
        Ok(())
    }

    /// Append the next blob in canonical order. An empty `Vec` marks an
    /// absent tile.
    pub fn push_blob(&mut self, bytes: Vec<u8>) -> Result<(), WriteError> {
        if self.push_cursor >= self.blobs.len() {
            return Err(WriteError::TileOutOfRange("pushed more blobs than the layout has tiles"));
        }
        self.blobs[self.push_cursor] = bytes;
        self.push_cursor += 1;
        Ok(())
    }

    /// Number of tiles in the layout.
    pub fn total_tiles(&self) -> usize {
        self.blobs.len()
    }

    /// Serialize the whole file into a `Vec`.
    pub fn finish(self) -> Result<Vec<u8>, WriteError> {
        let fm_len = self.layout.front_matter_len();
        let total = self.blobs.len();

        // Offset index: absolute file offsets, back-to-back blobs after the
        // front matter.
        let mut offsets = Vec::with_capacity(total + 1);
        let mut pos = fm_len;
        offsets.push(pos);
        for blob in &self.blobs {
            pos += blob.len() as u64;
            offsets.push(pos);
        }
        let file_len = usize::try_from(pos).map_err(|_| WriteError::InvalidParams("file length exceeds address space"))?;

        let mut out = Vec::with_capacity(file_len);
        out.extend_from_slice(&self.header.to_bytes()?);
        for g in &self.groups {
            out.extend_from_slice(&g.to_bytes(self.header.levels)?);
        }
        for off in &offsets {
            out.extend_from_slice(&off.to_le_bytes());
        }
        for blob in &self.blobs {
            out.extend_from_slice(blob);
        }
        debug_assert_eq!(out.len(), file_len);
        Ok(out)
    }

    /// Serialize the whole file into a writer.
    pub fn finish_into<W: Write>(self, mut w: W) -> Result<(), WriteError> {
        let bytes = self.finish()?;
        w.write_all(&bytes).map_err(|_| WriteError::InvalidParams("write failed"))?;
        Ok(())
    }
}
