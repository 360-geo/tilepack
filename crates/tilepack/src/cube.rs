//! Cubemap face geometry.
//!
//! This encodes the **production convention** used by the legacy DZP
//! converter and the production viewer, so tiles produced here render
//! identically to today's panoramas. The direction functions match the legacy
//! per-face orientation exactly:
//!
//! ```text
//! front (−1, −a, −b)   back  ( 1,  a, −b)
//! left  (−a,  1, −b)   right ( a, −1, −b)
//! down  ( b, −a, −1)   up    (−b, −a,  1)
//! ```
//!
//! Note: SPEC.md's cubemap table was rederived independently and differs in
//! sign from this. The production convention here is authoritative; the spec
//! table is to be reconciled to it (Phase 0 end-to-end orientation check).
//! Because this module is the one source of truth, that reconciliation is a
//! docs change, not a code change.

use crate::layout::Face;

/// Face coordinate `a` (horizontal) for a pixel column, texel-center convention.
pub fn col_to_a(col: u32, width: u32) -> f64 {
    2.0 * (col as f64 + 0.5) / width as f64 - 1.0
}

/// Face coordinate `b` (vertical) for a pixel row, row 0 at the top.
pub fn row_to_b(row: u32, height: u32) -> f64 {
    2.0 * (row as f64 + 0.5) / height as f64 - 1.0
}

/// View direction in the panorama's local frame (Z up, right-handed) for a
/// face and its `(a, b)` coordinates. The returned vector is not normalized;
/// its length is `sqrt(1 + a^2 + b^2)`.
pub fn face_dir(face: Face, a: f64, b: f64) -> [f64; 3] {
    match face {
        Face::Front => [-1.0, -a, -b],
        Face::Back => [1.0, a, -b],
        Face::Left => [-a, 1.0, -b],
        Face::Right => [a, -1.0, -b],
        Face::Down => [b, -a, -1.0],
        Face::Up => [-b, -a, 1.0],
    }
}

/// The face and `(a, b)` a direction lands on. Inverse of [`face_dir`] up to
/// the ray's scale. Used to build orientation test fixtures and to verify
/// edge consistency between faces.
pub fn dir_to_face_ab(dir: [f64; 3]) -> (Face, f64, f64) {
    let [x, y, z] = dir;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        if x < 0.0 {
            let s = -1.0 / x; // Front: [-1, -a, -b]
            (Face::Front, -(y * s), -(z * s))
        } else {
            let s = 1.0 / x; // Back: [1, a, -b]
            (Face::Back, y * s, -(z * s))
        }
    } else if ay >= az {
        if y > 0.0 {
            let s = 1.0 / y; // Left: [-a, 1, -b]
            (Face::Left, -(x * s), -(z * s))
        } else {
            let s = -1.0 / y; // Right: [a, -1, -b]
            (Face::Right, x * s, -(z * s))
        }
    } else if z < 0.0 {
        let s = -1.0 / z; // Down: [b, -a, -1]
        (Face::Down, -(y * s), x * s)
    } else {
        let s = 1.0 / z; // Up: [-b, -a, 1]
        (Face::Up, -(y * s), -(x * s))
    }
}
