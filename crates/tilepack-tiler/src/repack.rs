//! Lossless remux of Deep Zoom archives (`DZP` cubemaps, `SZI` planar) into
//! tilepack. Tile blobs are copied byte-for-byte; nothing is re-encoded.
//!
//! Deep Zoom stores `{name}_files/{level}/{col}_{row}.{ext}` plus a
//! `{name}.dzi` manifest, optionally nested in one wrapper folder. A `DZP`
//! carries six faces named by the letters `f b l r d u`; an `SZI` carries a
//! single pyramid. Deep Zoom numbers levels `0 = 1x1` up to full resolution;
//! tilepack keeps only the finest levels down to a single-tile coarsest level,
//! so the deep sub-tile levels are dropped (they carry no information a
//! single-tile overview lacks).

use std::collections::HashMap;
use std::io::{Cursor, Read};

use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::layout::Face;
use tilepack::{Writer, WriterParams};

use crate::{TilerError, levels_for, sniff_codec};

/// Whether the source archive was a cubemap or a planar pyramid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepackKind {
    Dzp,
    Szi,
}

/// Options controlling repack behavior.
#[derive(Debug, Clone, Default)]
pub struct RepackOptions {
    /// Reserved for a future re-encoding fallback. In v1, repack is always
    /// strict: mixed codecs are an error, never re-encoded.
    pub _reserved: (),
}

/// What a repack did, for logging and verification.
#[derive(Debug, Clone)]
pub struct RepackReport {
    pub kind: RepackKind,
    pub codec: Codec,
    pub root_w: u32,
    pub root_h: u32,
    pub tile_size: u16,
    pub levels: u8,
    pub tiles_copied: usize,
    pub tiles_absent: usize,
    /// Deep Zoom sub-tile levels below the kept range, dropped losslessly.
    pub dropped_coarse_levels: u32,
    pub notes: Vec<String>,
}

/// One Deep Zoom pyramid extracted from the archive.
struct Pyramid {
    tile_size: u16,
    overlap: u32,
    root_w: u32,
    root_h: u32,
    max_level: u32,
    tiles: HashMap<(u32, u32, u32), Vec<u8>>,
}

/// Repack a `DZP` or `SZI` archive into a tilepack file.
pub fn repack(zip_bytes: &[u8], _opts: &RepackOptions) -> Result<(Vec<u8>, RepackReport), TilerError> {
    let (dzis, mut tiles) = read_archive(zip_bytes)?;

    let names: Vec<String> = tiles.keys().cloned().collect();
    let is_dzp = ["f", "b", "l", "r", "d", "u"].iter().all(|f| names.iter().any(|n| n == f));

    if is_dzp {
        repack_dzp(&dzis, &mut tiles)
    } else if names.len() == 1 {
        repack_szi(&names[0], &dzis, &mut tiles)
    } else {
        Err(TilerError::Archive(format!(
            "expected 6 DZP faces or 1 SZI pyramid, found names: {names:?}"
        )))
    }
}

fn repack_dzp(dzis: &HashMap<String, DziInfo>, tiles: &mut TileMap) -> Result<(Vec<u8>, RepackReport), TilerError> {
    // All faces share geometry; read it from `f`.
    let info = dzis.get("f").ok_or_else(|| TilerError::Dzi("missing f.dzi".into()))?;
    if info.width != info.height {
        return Err(TilerError::Geometry(format!(
            "DZP face is not square: {}x{}",
            info.width, info.height
        )));
    }
    let mut pyramids: HashMap<Face, Pyramid> = HashMap::new();
    for (letter, face) in [
        ("f", Face::Front),
        ("b", Face::Back),
        ("l", Face::Left),
        ("r", Face::Right),
        ("d", Face::Down),
        ("u", Face::Up),
    ] {
        let dzi = dzis.get(letter).ok_or_else(|| TilerError::Dzi(format!("missing {letter}.dzi")))?;
        let face_tiles = tiles.remove(letter).unwrap_or_default();
        let max_level = face_tiles.keys().map(|(l, _, _)| *l).max().unwrap_or(0);
        pyramids.insert(
            face,
            Pyramid {
                tile_size: dzi.tile_size,
                overlap: dzi.overlap,
                root_w: dzi.width,
                root_h: dzi.height,
                max_level,
                tiles: face_tiles,
            },
        );
    }
    assemble(RepackKind::Dzp, 6, &pyramids)
}

fn repack_szi(name: &str, dzis: &HashMap<String, DziInfo>, tiles: &mut TileMap) -> Result<(Vec<u8>, RepackReport), TilerError> {
    let dzi = dzis.get(name).ok_or_else(|| TilerError::Dzi(format!("missing {name}.dzi")))?;
    let face_tiles = tiles.remove(name).unwrap_or_default();
    let max_level = face_tiles.keys().map(|(l, _, _)| *l).max().unwrap_or(0);
    let mut pyramids: HashMap<Face, Pyramid> = HashMap::new();
    pyramids.insert(
        Face::Front,
        Pyramid {
            tile_size: dzi.tile_size,
            overlap: dzi.overlap,
            root_w: dzi.width,
            root_h: dzi.height,
            max_level,
            tiles: face_tiles,
        },
    );
    assemble(RepackKind::Szi, 1, &pyramids)
}

fn assemble(kind: RepackKind, face_count: u8, pyramids: &HashMap<Face, Pyramid>) -> Result<(Vec<u8>, RepackReport), TilerError> {
    let first = pyramids
        .get(&Face::Front)
        .ok_or_else(|| TilerError::Archive("no front face".into()))?;
    let (tile_size, root_w, root_h) = (first.tile_size, first.root_w, first.root_h);

    if first.overlap != 0 {
        return Err(TilerError::Geometry(format!(
            "Deep Zoom overlap {} is unsupported; tilepack has no tile overlap",
            first.overlap
        )));
    }
    if tile_size == 0 {
        return Err(TilerError::Dzi("tile size 0".into()));
    }

    let levels = levels_for(root_w, root_h, tile_size);
    let dzi_finest = first.max_level;
    if (dzi_finest + 1) < levels as u32 {
        return Err(TilerError::Geometry(format!(
            "archive has {} Deep Zoom levels, tilepack needs {}",
            dzi_finest + 1,
            levels
        )));
    }
    let dropped_coarse_levels = (dzi_finest + 1) - levels as u32;

    // First pass: gather (ordinal, bytes) for present tiles and settle the
    // codec, verifying the whole archive is uniform.
    let probe = Writer::new(
        WriterParams {
            face_count,
            levels,
            tile_size,
            root_w,
            root_h,
        },
        vec![group_template(levels, Codec::Webp)],
    )?;
    let layout = probe.layout().clone();
    let total = probe.total_tiles();

    let mut codec: Option<Codec> = None;
    let mut copied = 0usize;
    let mut absent = 0usize;
    let mut sets: Vec<(usize, Vec<u8>)> = Vec::new();

    for ord in 0..total {
        let loc = layout.ordinal_loc(ord).expect("ordinal in range");
        let Some(pyr) = pyramids.get(&loc.face) else {
            absent += 1;
            continue;
        };
        let dzi_level = dzi_finest - (levels as u32 - 1 - loc.level as u32);
        match pyr.tiles.get(&(dzi_level, loc.col, loc.row)) {
            Some(bytes) => {
                let this = sniff_codec(bytes).ok_or_else(|| TilerError::Archive("tile is neither JPEG nor WebP".into()))?;
                match codec {
                    None => codec = Some(this),
                    Some(c) if c != this => return Err(TilerError::MixedCodec),
                    _ => {}
                }
                sets.push((ord, bytes.clone()));
                copied += 1;
            }
            None => absent += 1,
        }
    }

    let codec = codec.ok_or_else(|| TilerError::Archive("archive has no tiles".into()))?;

    // Second pass: build the file with the settled codec.
    let mut writer = Writer::new(
        WriterParams {
            face_count,
            levels,
            tile_size,
            root_w,
            root_h,
        },
        vec![group_template(levels, codec)],
    )?;
    for (ord, bytes) in sets {
        writer.set_ordinal(ord, bytes)?;
    }
    let out = writer.finish()?;

    let mut notes = Vec::new();
    if face_count == 6 && root_w != tile_size as u32 * (1u32 << (levels - 1)) {
        notes.push("DZP face is not tile_size * 2^(levels-1); coarsest level is sub-tile-size but legal".into());
    }
    if absent > 0 {
        notes.push(format!("{absent} tiles absent in source; written as zero-length"));
    }

    Ok((
        out,
        RepackReport {
            kind,
            codec,
            root_w,
            root_h,
            tile_size,
            levels,
            tiles_copied: copied,
            tiles_absent: absent,
            dropped_coarse_levels,
            notes,
        },
    ))
}

fn group_template(levels: u8, codec: Codec) -> GroupDescriptor {
    GroupDescriptor {
        semantic: Semantic::Rgb,
        codec,
        sample: SampleType::Rgb8,
        flags: GroupFlags::default(),
        level_count: levels,
        level_skip: 0,
        radiometry: Radiometry::default(),
    }
}

// ---- ZIP + DZI parsing ----------------------------------------------------

struct DziInfo {
    tile_size: u16,
    overlap: u32,
    width: u32,
    height: u32,
}

type TileMap = HashMap<String, HashMap<(u32, u32, u32), Vec<u8>>>;

/// Read a Deep Zoom ZIP into `(name -> dzi info, name -> tiles)`.
fn read_archive(zip_bytes: &[u8]) -> Result<(HashMap<String, DziInfo>, TileMap), TilerError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| TilerError::Archive(format!("open zip: {e}")))?;
    let mut dzis: HashMap<String, DziInfo> = HashMap::new();
    let mut tiles: TileMap = HashMap::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| TilerError::Archive(format!("zip entry {i}: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| TilerError::Io(format!("read {name}: {e}")))?;

        let segments: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
        if let Some(last) = segments.last() {
            if let Some(stem) = last.strip_suffix(".dzi") {
                dzis.insert(stem.to_string(), parse_dzi(&bytes)?);
                continue;
            }
        }
        // Tile: {name}_files/{level}/{col}_{row}.{ext}, optionally with one
        // wrapper folder in front.
        if let Some((files_seg, level_seg, tile_seg)) = tile_segments(&segments) {
            let Some(pyr_name) = files_seg.strip_suffix("_files") else {
                continue;
            };
            let Ok(level) = level_seg.parse::<u32>() else {
                continue;
            };
            let stem = tile_seg.split('.').next().unwrap_or("");
            let mut cr = stem.split('_');
            let (Some(c), Some(r)) = (cr.next(), cr.next()) else {
                continue;
            };
            let (Ok(col), Ok(row)) = (c.parse::<u32>(), r.parse::<u32>()) else {
                continue;
            };
            tiles.entry(pyr_name.to_string()).or_default().insert((level, col, row), bytes);
        }
    }

    Ok((dzis, tiles))
}

/// `{name}_files/{level}/{tile}` possibly nested one folder deep.
fn tile_segments<'a>(segments: &[&'a str]) -> Option<(&'a str, &'a str, &'a str)> {
    match segments.len() {
        3 => Some((segments[0], segments[1], segments[2])),
        4 => Some((segments[1], segments[2], segments[3])),
        _ => None,
    }
}

fn parse_dzi(bytes: &[u8]) -> Result<DziInfo, TilerError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut tile_size: Option<u16> = None;
    let mut overlap: u32 = 0;
    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"Image" => {
                        for attr in e.attributes().flatten() {
                            match attr.key.local_name().as_ref() {
                                b"TileSize" => tile_size = attr_u32(&attr).map(|v| v as u16),
                                b"Overlap" => overlap = attr_u32(&attr).unwrap_or(0),
                                _ => {}
                            }
                        }
                    }
                    b"Size" => {
                        for attr in e.attributes().flatten() {
                            match attr.key.local_name().as_ref() {
                                b"Width" => width = attr_u32(&attr),
                                b"Height" => height = attr_u32(&attr),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(TilerError::Dzi(format!("xml: {e}"))),
            _ => {}
        }
        buf.clear();
    }

    Ok(DziInfo {
        tile_size: tile_size.ok_or_else(|| TilerError::Dzi("missing TileSize".into()))?,
        overlap,
        width: width.ok_or_else(|| TilerError::Dzi("missing Width".into()))?,
        height: height.ok_or_else(|| TilerError::Dzi("missing Height".into()))?,
    })
}

fn attr_u32(attr: &quick_xml::events::attributes::Attribute) -> Option<u32> {
    std::str::from_utf8(&attr.value).ok()?.trim().parse().ok()
}
