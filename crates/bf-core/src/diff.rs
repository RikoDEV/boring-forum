//! Word-level diff for the edit history view. Classic LCS over words; the words the
//! two texts share at both ends are set aside first, and the work is capped so a huge post
//! cannot cost much.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<'a> {
    Same(&'a str),
    Added(&'a str),
    Removed(&'a str),
}

/// Splits text into words and the whitespace between them, keeping every character.
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut ws = None;
    for (i, c) in s.char_indices() {
        let w = c.is_whitespace();
        if ws.is_some_and(|k| k != w) {
            out.push(&s[start..i]);
            start = i;
        }
        ws = Some(w);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// The most cells the LCS table may have (4 bytes each), counted after the words the two texts
/// share at both ends are set aside: a small edit to a long post costs almost nothing.
const MAX_CELLS: usize = 1_000_000;

/// The part of `text` that the tokens `t` (consecutive slices of it) cover.
fn span<'a>(text: &'a str, t: &[&'a str]) -> &'a str {
    match (t.first(), t.last()) {
        (Some(f), Some(l)) => {
            let start = f.as_ptr() as usize - text.as_ptr() as usize;
            &text[start..l.as_ptr() as usize - text.as_ptr() as usize + l.len()]
        }
        _ => "",
    }
}

/// Diffs `old` → `new`. When the part that differs is still above `MAX_CELLS` it falls back to
/// "all removed, all added".
pub fn words<'a>(old: &'a str, new: &'a str) -> Vec<Op<'a>> {
    let (a, b) = (tokens(old), tokens(new));
    // A shared start never changes the result. A shared end can change which of several equal
    // diffs is picked, so it is only set aside when the table would otherwise be too big.
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let too_big = (a.len() - head).saturating_mul(b.len() - head) > MAX_CELLS;
    let tail = if too_big {
        a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count()
    } else {
        0
    };
    let (ma, mb) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out: Vec<Op<'a>> = a[..head].iter().map(|t| Op::Same(t)).collect();
    if ma.len().saturating_mul(mb.len()) > MAX_CELLS {
        let changed = [Op::Removed(span(old, ma)), Op::Added(span(new, mb))];
        out.extend(changed.into_iter().filter(|o| !matches!(o, Op::Removed("") | Op::Added(""))));
    } else {
        out.extend(lcs_diff(ma, mb));
    }
    out.extend(a[a.len() - tail..].iter().map(|t| Op::Same(t)));
    out
}

/// Classic LCS over tokens, in one flat table.
fn lcs_diff<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Op<'a>> {
    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    let mut lcs = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * w + j] =
                if a[i] == b[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Op::Same(a[i]));
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
            out.push(Op::Removed(a[i]));
            i += 1;
        } else {
            out.push(Op::Added(b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|t| Op::Removed(t)));
    out.extend(b[j..].iter().map(|t| Op::Added(t)));
    out
}

/// HTML for a diff: `<del>`/`<ins>` around changed words, everything escaped.
pub fn html(old: &str, new: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    words(old, new)
        .into_iter()
        .map(|op| match op {
            Op::Same(t) => esc(t),
            Op::Added(t) => format!("<ins>{}</ins>", esc(t)),
            Op::Removed(t) => format!("<del>{}</del>", esc(t)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_changed_words_only() {
        assert_eq!(html("the quick fox", "the slow fox"), "the <del>quick</del><ins>slow</ins> fox");
        assert_eq!(html("a", "a b"), "a<ins> </ins><ins>b</ins>");
    }

    #[test]
    fn escapes_html() {
        assert_eq!(html("<b>", "<i>"), "<del>&lt;b&gt;</del><ins>&lt;i&gt;</ins>");
    }

    /// The original table-per-call version, as the reference for small inputs.
    fn reference<'a>(old: &'a str, new: &'a str) -> Vec<Op<'a>> {
        let (a, b) = (tokens(old), tokens(new));
        let (n, m) = (a.len(), b.len());
        let mut lcs = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
            }
        }
        let (mut i, mut j, mut out) = (0, 0, Vec::new());
        while i < n && j < m {
            if a[i] == b[j] {
                out.push(Op::Same(a[i]));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                out.push(Op::Removed(a[i]));
                i += 1;
            } else {
                out.push(Op::Added(b[j]));
                j += 1;
            }
        }
        out.extend(a[i..].iter().map(|t| Op::Removed(t)));
        out.extend(b[j..].iter().map(|t| Op::Added(t)));
        out
    }

    #[test]
    fn output_matches_the_plain_algorithm() {
        // A small deterministic generator over a tiny vocabulary, so repeats and ties are common.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let vocab = ["a", "b", "c", " ", "  ", "dd"];
        for _ in 0..3000 {
            let (la, lb) = (next() % 14, next() % 14);
            let mut make = |len: u64| (0..len).map(|_| vocab[(next() % 6) as usize]).collect::<String>();
            let (old, new) = (make(la), make(lb));
            assert_eq!(words(&old, &new), reference(&old, &new), "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn a_small_edit_in_a_big_post_is_cheap_and_unrelated_huge_texts_fall_back() {
        let big: String = (0..5000).map(|i| format!("w{i} ")).collect();
        let edited = big.replacen("w2500", "changed", 1);
        let d = words(&big, &edited);
        assert!(d.contains(&Op::Added("changed")) && d.contains(&Op::Removed("w2500")));
        let other: String = (0..5000).map(|i| format!("x{i} ")).collect();
        let d = words(&big, &other);
        assert!(d.len() <= 3, "no word-level diff of unrelated texts");
        assert!(matches!(d[0], Op::Removed(t) if t.starts_with("w0 ")));
        assert!(matches!(d[1], Op::Added(t) if t.starts_with("x0 ")));
    }
}
