//! The cubemap direction table must be edge-consistent (adjacent faces agree
//! along shared edges) and self-inverse. This is the production DZP
//! convention; the end-to-end orientation check against a real render is a
//! separate tiler-crate fixture (Phase 0).

use tilepack::cube::{col_to_a, dir_to_face_ab, face_dir, row_to_b};
use tilepack::layout::Face;

fn normalize(d: [f64; 3]) -> [f64; 3] {
    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    [d[0] / n, d[1] / n, d[2] / n]
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
}

#[test]
fn dir_and_inverse_round_trip() {
    // Every face, a grid of (a, b), maps to a direction and back to the same
    // face and coordinates.
    for face in Face::ALL {
        for ai in 0..9 {
            for bi in 0..9 {
                let a = -0.9 + 0.225 * ai as f64;
                let b = -0.9 + 0.225 * bi as f64;
                let dir = face_dir(face, a, b);
                let (f2, a2, b2) = dir_to_face_ab(dir);
                assert_eq!(f2, face, "face for a={a} b={b} on {face:?}");
                assert!((a2 - a).abs() < 1e-9, "a mismatch on {face:?}: {a} vs {a2}");
                assert!((b2 - b).abs() < 1e-9, "b mismatch on {face:?}: {b} vs {b2}");
            }
        }
    }
}

#[test]
fn front_right_edge_meets_right_left_edge() {
    // Front's rightmost column (a = +1) and Right's leftmost column (a = -1)
    // are the same physical edge; their directions must coincide.
    for bi in 0..5 {
        let b = -0.8 + 0.4 * bi as f64;
        let front = normalize(face_dir(Face::Front, 1.0, b));
        let right = normalize(face_dir(Face::Right, -1.0, b));
        assert!(close(front, right), "front/right edge mismatch at b={b}: {front:?} vs {right:?}");
    }
}

#[test]
fn front_left_edge_meets_left_right_edge() {
    for bi in 0..5 {
        let b = -0.8 + 0.4 * bi as f64;
        let front = normalize(face_dir(Face::Front, -1.0, b));
        let left = normalize(face_dir(Face::Left, 1.0, b));
        assert!(close(front, left), "front/left edge mismatch at b={b}: {front:?} vs {left:?}");
    }
}

#[test]
fn pixel_to_face_coords_are_texel_centered() {
    // Center pixel of an even-sized face lands near (0,0) but offset by half a
    // texel; corners approach ±1.
    assert!((col_to_a(0, 4) - (-0.75)).abs() < 1e-12);
    assert!((col_to_a(3, 4) - 0.75).abs() < 1e-12);
    assert!((row_to_b(0, 4) - (-0.75)).abs() < 1e-12);
}
