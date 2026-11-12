//! Sequence alignment: which element of an edited sequence corresponds to which original.

/// Largest table the longest-common-subsequence step may fill; longer middles count as fully
/// replaced, which costs formatting but never correctness.
const MAX_CELLS: usize = 4_000_000;

/// For each element of `b`, the index of the element of `a` it corresponds to.
///
/// Elements equal under `eq` are matched along a longest common subsequence. Between two such
/// anchors, the remaining elements are paired by position where `pair` allows it: an edited
/// paragraph keeps the identity of the original at the same place.
pub(crate) fn align<A, B>(a: &[A], b: &[B], eq: impl Fn(&A, &B) -> bool, pair: impl Fn(&A, &B) -> bool) -> Vec<Option<usize>> {
    let mut out = vec![None; b.len()];
    let anchors = lcs(a, b, &eq);
    let (mut ai, mut bi) = (0, 0);
    for &(ma, mb) in anchors.iter().chain(std::iter::once(&(a.len(), b.len()))) {
        for k in 0..(ma - ai).min(mb - bi) {
            if pair(&a[ai + k], &b[bi + k]) {
                out[bi + k] = Some(ai + k);
            }
        }
        if mb < b.len() {
            out[mb] = Some(ma);
        }
        (ai, bi) = (ma + 1, mb + 1);
    }
    out
}

/// Matched index pairs of a longest common subsequence, in order.
fn lcs<A, B>(a: &[A], b: &[B], eq: &impl Fn(&A, &B) -> bool) -> Vec<(usize, usize)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| eq(x, y)).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| eq(x, y)).count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut pairs: Vec<(usize, usize)> = (0..prefix).map(|i| (i, i)).collect();
    let (n, m) = (am.len(), bm.len());
    if n > 0 && m > 0 && (n + 1) * (m + 1) <= MAX_CELLS {
        // table[i][j]: LCS length of am[i..] and bm[j..]
        let width = m + 1;
        let mut table = vec![0u32; (n + 1) * width];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i * width + j] = if eq(&am[i], &bm[j]) {
                    table[(i + 1) * width + j + 1] + 1
                } else {
                    table[(i + 1) * width + j].max(table[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if eq(&am[i], &bm[j]) {
                pairs.push((prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    pairs.extend((0..suffix).map(|k| (a.len() - suffix + k, b.len() - suffix + k)));
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_equal_elements_and_pairs_edited_ones() {
        let a = ["intro", "q1", "", "", "q2"];
        let b = ["intro", "q1", "answer", "", "extra", "q2"];
        let mapped = align(&a, &b, |x, y| x == y, |_, _| true);
        // "answer" takes the place of the first blank line, the second blank line matches,
        // and "extra" is new.
        assert_eq!(mapped, vec![Some(0), Some(1), Some(2), Some(3), None, Some(4)]);
    }

    #[test]
    fn character_alignment_keeps_unchanged_text() {
        let a: Vec<char> = "Hola mundo".chars().collect();
        let b: Vec<char> = "Hola, nuevo mundo".chars().collect();
        let mapped = align(&a, &b, |x, y| x == y, |_, _| false);
        let kept: String = b.iter().zip(&mapped).filter(|(_, m)| m.is_some()).map(|(c, _)| *c).collect();
        assert_eq!(kept, "Hola mundo");
    }
}
