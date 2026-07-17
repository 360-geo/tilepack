//! Editing existing tilepacks without re-encoding: merging a sibling's groups
//! in (same resolution, or a lower-resolution sibling re-anchored onto the
//! matching pyramid level), and dropping the finest pyramid levels for
//! archival. Both operations copy tile blobs verbatim and rewrite only the
//! front matter.

use tilepack::{GroupDescriptor, TilepackView, Writer, WriterParams};

use crate::TilerError;

/// Append every group of `sibling` to `primary`, producing one file. The two
/// must share face count and tile size, and the sibling's root dimensions
/// must equal the primary's dimensions at some pyramid level — a
/// lower-resolution sibling (a 900 px depth field beside 3600 px RGB faces)
/// re-anchors onto that level via `level_skip`, per-level geometry being
/// identical by the nested-ceil halving identity. Existing tiles keep their
/// ordinals; the sibling's groups are appended, so no blob is re-encoded or
/// moved relative to its group.
pub fn merge_groups(primary: &[u8], sibling: &[u8]) -> Result<Vec<u8>, TilerError> {
    let a = TilepackView::new(primary).map_err(|e| TilerError::Io(format!("parse primary: {e}")))?;
    let b = TilepackView::new(sibling).map_err(|e| TilerError::Io(format!("parse sibling: {e}")))?;
    let (ha, hb) = (&a.fm.header, &b.fm.header);

    if ha.face_count != hb.face_count {
        return Err(TilerError::Geometry("merge requires matching face count".into()));
    }
    // Tile size only shapes tiled groups' grids; a fully-untiled sibling
    // (the depth shape) needs no tile-grid agreement.
    let sibling_all_untiled = b.fm.layout.groups().iter().all(|g| g.flags.untiled());
    if ha.tile_size != hb.tile_size && !sibling_all_untiled {
        return Err(TilerError::Geometry("merge of tiled groups requires matching tile size".into()));
    }
    if ha.group_count as usize + hb.group_count as usize > u8::MAX as usize {
        return Err(TilerError::Geometry("merged group count exceeds 255".into()));
    }

    // The sibling's finest level must sit somewhere in the primary's pyramid:
    // find the finest primary level whose dimensions equal the sibling root.
    // `shift` is how many levels below the primary's finest that is; sibling
    // groups re-anchor by adding it to their level_skip.
    let anchor = (0..ha.levels)
        .rev()
        .find(|&level| a.fm.layout.level_dims(level) == (hb.root_w, hb.root_h) && level + 1 >= hb.levels);
    let Some(anchor) = anchor else {
        return Err(TilerError::Geometry(format!(
            "sibling root {}x{} with {} levels does not fit the primary pyramid",
            hb.root_w, hb.root_h, hb.levels
        )));
    };
    let shift = ha.levels - 1 - anchor;

    let mut groups: Vec<GroupDescriptor> = a.fm.layout.groups().to_vec();
    for g in b.fm.layout.groups() {
        let mut ng = *g;
        ng.level_skip = g
            .level_skip
            .checked_add(shift)
            .ok_or_else(|| TilerError::Geometry("re-anchored level_skip exceeds 255".into()))?;
        groups.push(ng);
    }

    let params = WriterParams {
        face_count: ha.face_count,
        levels: ha.levels,
        tile_size: ha.tile_size,
        root_w: ha.root_w,
        root_h: ha.root_h,
    };
    let mut writer = Writer::new(params, groups)?;

    let a_total = a.fm.layout.total_tiles() as usize;
    let b_total = b.fm.layout.total_tiles() as usize;
    for ord in 0..a_total {
        if let Some(blob) = a.tile_by_ordinal(ord) {
            writer.set_ordinal(ord, blob.to_vec())?;
        }
    }
    // Sibling groups are appended, so sibling ordinal j -> new ordinal a_total + j.
    for j in 0..b_total {
        if let Some(blob) = b.tile_by_ordinal(j) {
            writer.set_ordinal(a_total + j, blob.to_vec())?;
        }
    }
    writer.finish().map_err(Into::into)
}

/// Drop the `n` finest pyramid levels of every group, for archival. Coarse
/// levels keep identical geometry (the nested-ceil halving identity), so tile
/// blobs are copied verbatim; a group that only existed in dropped levels is
/// removed. The retained coarsest level becomes the new finest.
pub fn strip_finest_levels(existing: &[u8], n: u8) -> Result<Vec<u8>, TilerError> {
    let view = TilepackView::new(existing).map_err(|e| TilerError::Io(format!("parse: {e}")))?;
    let old = &view.fm.layout;
    let h = &view.fm.header;

    if n == 0 {
        return Ok(existing.to_vec());
    }
    if n >= h.levels {
        return Err(TilerError::Geometry("cannot strip all levels".into()));
    }
    let new_levels = h.levels - n;
    // New finest dims = old dims at the level that becomes the new finest.
    let (new_root_w, new_root_h) = old.level_dims(h.levels - 1 - n);

    // Retained groups, with the level window clamped to what survives. Level
    // indices below the cut are unchanged, so a group keeps its coarse bound
    // and loses only covered levels at or above `new_levels`.
    let mut new_groups: Vec<GroupDescriptor> = Vec::new();
    let mut kept_old_indices: Vec<usize> = Vec::new();
    for (gi, g) in old.groups().iter().enumerate() {
        let window = old.group_levels(gi);
        if window.start >= new_levels {
            continue; // group lived only in dropped fine levels
        }
        let new_hi = window.end.min(new_levels);
        let mut ng = *g;
        ng.level_count = new_hi - window.start;
        ng.level_skip = new_levels - new_hi;
        new_groups.push(ng);
        kept_old_indices.push(gi);
    }
    if new_groups.is_empty() {
        return Err(TilerError::Geometry("no groups survive the strip".into()));
    }

    let params = WriterParams {
        face_count: h.face_count,
        levels: new_levels,
        tile_size: h.tile_size,
        root_w: new_root_w,
        root_h: new_root_h,
    };
    let mut writer = Writer::new(params, new_groups)?;
    let new_layout = writer.layout().clone();

    // Copy each retained tile by matching (group, level, face, col, row); coarse
    // geometry is identical, so blobs transfer verbatim.
    let total = writer.total_tiles();
    for new_ord in 0..total {
        let loc = new_layout.ordinal_loc(new_ord).unwrap();
        let old_gi = kept_old_indices[loc.group as usize];
        let old_loc = tilepack::TileLoc {
            group: old_gi as u8,
            ..loc
        };
        if let Some(blob) = view.tile(old_loc) {
            writer.set_ordinal(new_ord, blob.to_vec())?;
        }
    }
    writer.finish().map_err(Into::into)
}
