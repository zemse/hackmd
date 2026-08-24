//! Human-friendly ("natural") name ordering for listings.
//!
//! Plain lexicographic order interleaves digit runs by their first character,
//! so `week0, week1, week10, week11, week2, …` is what a directory of eleven
//! weeks looks like. [`natural_cmp`] compares digit runs by value instead, so
//! the same names come out `week0, week1, week2, … week10, week11` — the order
//! Finder, Explorer and GNOME Files all use.

use std::cmp::Ordering;

/// Case-insensitive natural comparison: text compares character by character,
/// digit runs compare as numbers.
///
/// Leading zeros don't change a run's value (`week007` sorts with `week7`) but
/// do break ties, so the order is total and deterministic: equal-by-value names
/// fall back to a plain byte comparison rather than landing in arbitrary
/// positions.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    while i < av.len() && j < bv.len() {
        if av[i].is_ascii_digit() && bv[j].is_ascii_digit() {
            let (na, ni) = digit_run(&av, i);
            let (nb, nj) = digit_run(&bv, j);
            // Compare by value without parsing, so a 40-digit run in a file
            // name can't overflow: more significant digits means larger, and
            // equal widths compare digit by digit.
            let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if ord != Ordering::Equal {
                return ord;
            }
            i = ni;
            j = nj;
        } else {
            let ord = av[i]
                .to_lowercase()
                .cmp(bv[j].to_lowercase())
                .then_with(|| av[i].cmp(&bv[j]));
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
        }
    }
    // Whichever name ran out first is the prefix, so it sorts above.
    (av.len() - i).cmp(&(bv.len() - j)).then_with(|| a.cmp(b))
}

/// The digit run starting at `start`, with leading zeros stripped, plus the
/// index just past it.
fn digit_run(s: &[char], start: usize) -> (&[char], usize) {
    let mut end = start;
    while end < s.len() && s[end].is_ascii_digit() {
        end += 1;
    }
    let mut first = start;
    while first + 1 < end && s[first] == '0' {
        first += 1;
    }
    (&s[first..end], end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_order_by_value_not_by_first_digit() {
        let input = vec![
            "week10", "week2", "week0", "week11", "week1", "week9", "week20",
        ];
        assert_eq!(
            sorted(input),
            vec![
                "week0", "week1", "week2", "week9", "week10", "week11", "week20"
            ]
        );
    }

    #[test]
    fn a_prefix_sorts_above_its_extensions() {
        assert_eq!(natural_cmp("week", "week1"), Ordering::Less);
        assert_eq!(natural_cmp("notes.md", "notes-extra.md"), Ordering::Greater);
        assert_eq!(natural_cmp("week1", "week1a"), Ordering::Less);
    }

    #[test]
    fn case_is_ignored_but_still_breaks_ties() {
        assert_eq!(natural_cmp("Alpha", "beta"), Ordering::Less);
        assert_eq!(natural_cmp("alpha", "Alpha"), Ordering::Greater);
        assert_eq!(sorted(vec!["b.md", "A.md"]), vec!["A.md", "b.md"]);
    }

    #[test]
    fn leading_zeros_compare_equal_then_break_ties() {
        assert_eq!(natural_cmp("week007", "week7"), Ordering::Less);
        assert_eq!(
            sorted(vec!["v10", "v007", "v7", "v8"]),
            vec!["v007", "v7", "v8", "v10"]
        );
    }

    #[test]
    fn multiple_number_groups_compare_left_to_right() {
        assert_eq!(
            sorted(vec!["ch2-s10", "ch10-s1", "ch2-s2"]),
            vec!["ch2-s2", "ch2-s10", "ch10-s1"]
        );
    }

    #[test]
    fn huge_digit_runs_do_not_overflow() {
        let big = "f".to_string() + &"9".repeat(40);
        let bigger = "f".to_string() + &"9".repeat(41);
        assert_eq!(natural_cmp(&big, &bigger), Ordering::Less);
    }

    #[test]
    fn non_ascii_names_still_order_sensibly() {
        assert_eq!(natural_cmp("Éclair", "zebra"), Ordering::Greater);
        assert_eq!(natural_cmp("éclair", "Éclair"), Ordering::Greater);
    }
}
