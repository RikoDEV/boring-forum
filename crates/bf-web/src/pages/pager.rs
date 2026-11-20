//! The page numbers a pager shows.

/// The numbers to show for page `current` of `total`: the first, the last and two either side of the
/// current page, as `(number, address, is the current one)`. Where numbers are skipped there is a gap,
/// `(0, "", false)`, unless only one is skipped, which is shown instead: "1 … 3" would hide a single page
/// behind a mark as wide as the number.
pub fn page_window(total: i64, current: i64, url: impl Fn(i64) -> String) -> Vec<(i64, String, bool)> {
    let total = total.max(1);
    let mut shown: Vec<i64> = (1..=total).filter(|n| *n == 1 || *n == total || (n - current).abs() <= 2).collect();
    // A single skipped page is shown, not hidden.
    let mut filled = Vec::with_capacity(shown.len() + 2);
    for n in shown.drain(..) {
        if let Some(prev) = filled.last().copied().filter(|p| n - p == 2) {
            filled.push(prev + 1);
        }
        filled.push(n);
    }
    let mut out = Vec::with_capacity(filled.len() + 2);
    let mut last = 0;
    for n in filled {
        if last != 0 && n - last > 1 {
            out.push((0, String::new(), false));
        }
        out.push((n, url(n), n == current));
        last = n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(total: i64, current: i64) -> String {
        page_window(total, current, |n| format!("/p/{n}"))
            .into_iter()
            .map(|(n, _, cur)| match (n, cur) {
                (0, _) => "…".to_owned(),
                (n, true) => format!("[{n}]"),
                (n, false) => n.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_window_marks_what_is_skipped() {
        assert_eq!(shape(27, 27), "1 … 25 26 [27]");
        assert_eq!(shape(27, 14), "1 … 12 13 [14] 15 16 … 27");
        assert_eq!(shape(27, 1), "[1] 2 3 … 27");
        assert_eq!(shape(5, 3), "1 2 [3] 4 5");
        assert_eq!(shape(11, 6), "1 … 4 5 [6] 7 8 … 11");
    }

    #[test]
    fn one_skipped_page_is_shown_not_hidden() {
        assert_eq!(shape(9, 5), "1 2 3 4 [5] 6 7 8 9");
        assert_eq!(shape(10, 6), "1 … 4 5 [6] 7 8 9 10");
    }

    #[test]
    fn a_single_page_is_just_itself() {
        assert_eq!(shape(1, 1), "[1]");
        assert_eq!(shape(0, 1), "[1]");
    }
}
