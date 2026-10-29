//! Finding a query in text, as the diff's search does: every place it is,
//! one after another without overlapping. A query all in lower case finds
//! any case; one with an upper case letter, only that case.

use std::ops::Range;

/// Whether `query` finds text of any case.
fn any_case(query: &str) -> bool {
    !query.chars().any(char::is_uppercase)
}

/// `c` as it is compared: in lower case when `fold`.
fn fold(c: char, fold: bool) -> char {
    if fold {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

/// Where `query` is in `text`, by char index, first to last; none for an
/// empty query.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    let folds = any_case(query);
    let query: Vec<char> = query.chars().map(|c| fold(c, folds)).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let text: Vec<char> = text.chars().map(|c| fold(c, folds)).collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at + query.len() <= text.len() {
        if text[at..at + query.len()] == query[..] {
            found.push(at..at + query.len());
            at += query.len();
        } else {
            at += 1;
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn lower_case_finds_any_case() {
        assert_eq!(find_all("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
    }

    #[test]
    fn upper_case_finds_only_its_case() {
        assert_eq!(find_all("Foo foo FOO", "Foo"), vec![0..3]);
        assert_eq!(find_all("Foo foo FOO", "FOO"), vec![8..11]);
    }

    #[test]
    fn places_are_by_char_and_do_not_overlap() {
        assert_eq!(find_all("é→aaa", "aa"), vec![2..4]);
        assert_eq!(find_all("aaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(find_all("ÄÖ äö", "äö"), vec![0..2, 3..5]);
    }

    #[test]
    fn an_empty_query_or_a_longer_one_finds_nothing() {
        assert!(find_all("text", "").is_empty());
        assert!(find_all("ab", "abc").is_empty());
        assert!(find_all("", "a").is_empty());
    }
}
