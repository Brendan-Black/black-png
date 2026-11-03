//! PNG's per-row filters, which turn pixels into differences from their neighbours so they
//! compress better. Each row is filtered against the row above (zeros for the first), with
//! `bpp` the bytes per pixel.

/// Filter types, in the order PNG numbers them.
pub const NONE: u8 = 0;
pub const SUB: u8 = 1;
pub const UP: u8 = 2;
pub const AVERAGE: u8 = 3;
pub const PAETH: u8 = 4;

/// The neighbour Paeth picks: whichever of left, above and above-left is closest to
/// left + above - above-left.
pub fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Filter `row` with filter `kind` into `out`.
pub fn apply(kind: u8, row: &[u8], prev: &[u8], bpp: usize, out: &mut [u8]) {
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let predicted = match kind {
            NONE => 0,
            SUB => a,
            UP => b,
            AVERAGE => ((a as u16 + b as u16) / 2) as u8,
            _ => paeth(a, b, c),
        };
        out[i] = row[i].wrapping_sub(predicted);
    }
}

/// Pick the filter for a row the way libpng does: the one whose output, read as signed
/// bytes, has the smallest total size. Writes the filter type and filtered row to `out`.
pub fn best(row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>, scratch: &mut [u8]) {
    let mut best = (u64::MAX, NONE);
    for kind in [NONE, SUB, UP, AVERAGE, PAETH] {
        apply(kind, row, prev, bpp, scratch);
        let score: u64 = scratch.iter().map(|&x| (x as i8).unsigned_abs() as u64).sum();
        if score < best.0 {
            best = (score, kind);
        }
    }
    apply(best.1, row, prev, bpp, scratch);
    out.push(best.1);
    out.extend_from_slice(scratch);
}


/// Undo filter `kind` on `row` in place, given the already-decoded row above. False if
/// `kind` isn't a filter type.
pub fn undo(kind: u8, row: &mut [u8], prev: &[u8], bpp: usize) -> bool {
    match kind {
        NONE => {}
        SUB => {
            for i in bpp..row.len() {
                row[i] = row[i].wrapping_add(row[i - bpp]);
            }
        }
        UP => {
            for (x, &b) in row.iter_mut().zip(prev) {
                *x = x.wrapping_add(b);
            }
        }
        AVERAGE => {
            for i in 0..row.len() {
                let a = if i >= bpp { row[i - bpp] } else { 0 };
                row[i] = row[i].wrapping_add(((a as u16 + prev[i] as u16) / 2) as u8);
            }
        }
        PAETH => {
            for i in 0..row.len() {
                let (a, c) = if i >= bpp { (row[i - bpp], prev[i - bpp]) } else { (0, 0) };
                row[i] = row[i].wrapping_add(paeth(a, prev[i], c));
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paeth_picks_the_closest_neighbour() {
        assert_eq!(paeth(10, 20, 10), 20); // p = 20: above
        assert_eq!(paeth(20, 10, 10), 20); // p = 20: left
        assert_eq!(paeth(10, 10, 30), 10); // p = -10: left wins ties
        assert_eq!(paeth(0, 0, 0), 0);
        assert_eq!(paeth(255, 255, 0), 255);
    }

    #[test]
    fn filters() {
        let prev = [10, 20, 30, 40];
        let row = [15, 25, 33, 50];
        let mut out = [0u8; 4];
        apply(SUB, &row, &prev, 2, &mut out);
        assert_eq!(out, [15, 25, 18, 25]);
        apply(UP, &row, &prev, 2, &mut out);
        assert_eq!(out, [5, 5, 3, 10]);
        apply(AVERAGE, &row, &prev, 2, &mut out);
        assert_eq!(out, [10, 15, 33 - 22, 50 - 32]);
        apply(NONE, &row, &prev, 2, &mut out);
        assert_eq!(out, row);
    }

    #[test]
    fn undo_reverses_apply() {
        let prev = [200, 3, 90, 255, 0, 17, 64];
        let row = [1, 250, 33, 128, 7, 7, 200];
        for kind in [NONE, SUB, UP, AVERAGE, PAETH] {
            let mut out = [0u8; 7];
            apply(kind, &row, &prev, 3, &mut out);
            assert!(undo(kind, &mut out, &prev, 3));
            assert_eq!(out, row, "filter {kind}");
        }
        assert!(!undo(5, &mut [0], &[0], 1));
    }

    #[test]
    fn a_flat_row_under_a_matching_row_picks_up() {
        let (row, prev) = ([7u8; 12], [7u8; 12]);
        let (mut out, mut scratch) = (Vec::new(), [0u8; 12]);
        best(&row, &prev, 3, &mut out, &mut scratch);
        assert_eq!(out[0], UP);
        assert!(out[1..].iter().all(|&x| x == 0));
    }
}
