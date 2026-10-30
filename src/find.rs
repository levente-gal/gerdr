//! Finding a query in text, as the diff's search does: every place it is,
//! one after another without overlapping. A query all in lower case finds
//! any case; one with an upper case letter, only that case. A whole word
//! (vim's `*`) is found only as a word of its own, in its own case.
//!
//! A caret goes on by words as vim's `w`, `b` and `e` do, along a line:
//! a word is a run of word characters, or of other ones but blanks.

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

/// Whether `c` is part of a word for a whole-word search: a letter, a
/// digit or `_`, as vim's `*` takes them.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word at char `at` of `text`, by char index; off a word, the next
/// one after it on the line, as vim's `*` takes it. None past the last.
pub fn word_at(text: &str, at: usize) -> Option<Range<usize>> {
    let cs: Vec<char> = text.chars().collect();
    let first = (at..cs.len()).find(|&i| is_word(cs[i]))?;
    let start = if first == at {
        (0..at)
            .rev()
            .find(|&i| !is_word(cs[i]))
            .map_or(0, |i| i + 1)
    } else {
        first
    };
    let end = (start..cs.len())
        .find(|&i| !is_word(cs[i]))
        .unwrap_or(cs.len());
    Some(start..end)
}

/// Where `word` is in `text` as a word of its own, not part of a longer
/// one, by char index, in its own case only; none for an empty word.
pub fn find_words(text: &str, word: &str) -> Vec<Range<usize>> {
    let word: Vec<char> = word.chars().collect();
    let cs: Vec<char> = text.chars().collect();
    let n = word.len();
    let mut found = Vec::new();
    if n == 0 {
        return found;
    }
    let apart = |i: Option<usize>| i.and_then(|i| cs.get(i)).is_none_or(|&c| !is_word(c));
    let mut at = 0;
    while at + n <= cs.len() {
        if cs[at..at + n] == word[..] && apart(at.checked_sub(1)) && apart(Some(at + n)) {
            found.push(at..at + n);
            at += n;
        } else {
            at += 1;
        }
    }
    found
}

/// `find_all`, or for a `whole` word, `find_words`.
pub fn find(text: &str, query: &str, whole: bool) -> Vec<Range<usize>> {
    if whole {
        find_words(text, query)
    } else {
        find_all(text, query)
    }
}

/// What a character is to the caret's word moves: a blank (0), part of a
/// word (1), or of a run of other characters (2).
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if is_word(c) {
        1
    } else {
        2
    }
}

/// The start of the next word after char `at` of `text`, as vim's `w`
/// goes; none on the line.
pub fn next_word(text: &str, at: usize) -> Option<usize> {
    let cs: Vec<char> = text.chars().collect();
    let here = class(*cs.get(at)?);
    let past = (at..cs.len()).find(|&i| class(cs[i]) != here)?;
    (past..cs.len()).find(|&i| class(cs[i]) != 0)
}

/// The start of the word before char `at` of `text` (or of the one it
/// is in), as vim's `b` goes; none on the line.
pub fn prev_word(text: &str, at: usize) -> Option<usize> {
    let cs: Vec<char> = text.chars().collect();
    let last = (0..at.min(cs.len())).rev().find(|&i| class(cs[i]) != 0)?;
    let here = class(cs[last]);
    let start = (0..last).rev().find(|&i| class(cs[i]) != here);
    Some(start.map_or(0, |i| i + 1))
}

/// The last char of the word at or after char `from` of `text`, as vim's
/// `e` goes from the char before; none on the line.
pub fn word_end(text: &str, from: usize) -> Option<usize> {
    let cs: Vec<char> = text.chars().collect();
    let first = (from..cs.len()).find(|&i| class(cs[i]) != 0)?;
    let here = class(cs[first]);
    let past = (first..cs.len()).find(|&i| class(cs[i]) != here);
    Some(past.unwrap_or(cs.len()) - 1)
}

/// The first char of `text` but a blank, as vim's `^` goes; 0 for none.
pub fn first_char(text: &str) -> usize {
    text.chars().position(|c| !c.is_whitespace()).unwrap_or(0)
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

    #[test]
    fn a_word_is_the_one_at_a_place_or_the_next_after_it() {
        let text = "let foo_bar = baz(1);";
        assert_eq!(word_at(text, 5), Some(4..11));
        assert_eq!(word_at(text, 4), Some(4..11));
        // Off a word, the next one on.
        assert_eq!(word_at(text, 11), Some(14..17));
        assert_eq!(word_at(text, 0), Some(0..3));
        assert_eq!(word_at(text, 19), None);
        assert_eq!(word_at("", 0), None);
    }

    #[test]
    fn a_whole_word_is_found_only_on_its_own_in_its_case() {
        let text = "id idle id_x (id) Id xid id";
        assert_eq!(find_words(text, "id"), vec![0..2, 14..16, 25..27]);
        assert!(find_words(text, "").is_empty());
        assert_eq!(find(text, "id", false).len(), 7);
    }

    #[test]
    fn the_caret_goes_by_words_and_runs_of_other_chars() {
        let text = "  let foo = bar(1);";
        assert_eq!(next_word(text, 0), Some(2));
        assert_eq!(next_word(text, 2), Some(6));
        assert_eq!(next_word(text, 6), Some(10));
        assert_eq!(next_word(text, 14), Some(15));
        assert_eq!(next_word(text, 15), Some(16));
        assert_eq!(next_word(text, 17), None);
        assert_eq!(prev_word(text, 16), Some(15));
        assert_eq!(prev_word(text, 15), Some(12));
        assert_eq!(prev_word(text, 13), Some(12));
        assert_eq!(prev_word(text, 6), Some(2));
        assert_eq!(prev_word(text, 2), None);
        assert_eq!(word_end(text, 1), Some(4));
        assert_eq!(word_end(text, 5), Some(8));
        assert_eq!(word_end(text, 18), Some(18));
        assert_eq!(word_end(text, 19), None);
        assert_eq!(first_char(text), 2);
        assert_eq!(first_char("   "), 0);
    }
}
