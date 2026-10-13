//! Fuzzy matching for the switcher: the query's characters must appear in
//! the text in order, not necessarily together. Matches that keep the
//! characters together, or start them at words, score higher.

/// Score for each character matched.
const MATCH: i32 = 16;
/// Extra for a character starting a word: after a separator, or an upper
/// case letter after a lower case one.
const WORD_START: i32 = 10;
/// Extra for the text's first character.
const FIRST: i32 = 6;
/// Extra for a character right after the previous one matched.
const CONSECUTIVE: i32 = 8;
/// What a gap between two matched characters costs: a start, and a bit more
/// for each character it skips after the first.
const GAP_START: i32 = 3;
const GAP_EXTEND: i32 = 1;

const NONE: i32 = i32::MIN / 2;

/// How well `query` matches `text`, and where: the positions (in chars) of
/// the matched characters in `text`. `None` when it doesn't match at all.
/// Whitespace in the query is ignored; it matches either case, unless it has
/// an upper case letter.
pub fn score(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let exact = query.iter().any(|c| c.is_uppercase());
    let fold = |c: char| {
        if exact {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let text: Vec<char> = text.chars().collect();
    let folded: Vec<char> = text.iter().map(|c| fold(*c)).collect();
    let (m, n) = (query.len(), text.len());
    if m > n {
        return None;
    }
    let bonus: Vec<i32> = (0..n)
        .map(|j| match j.checked_sub(1).map(|k| text[k]) {
            None => WORD_START + FIRST,
            Some(prev) if !prev.is_alphanumeric() => WORD_START,
            Some(prev) if prev.is_lowercase() && text[j].is_uppercase() => WORD_START,
            _ => 0,
        })
        .collect();

    // best[i][j]: the best score for query[..=i] with query[i] on text[j];
    // from[i][j]: where query[i - 1] is then.
    let mut best = vec![vec![NONE; n]; m];
    let mut from = vec![vec![0; n]; m];
    for i in 0..m {
        // The best place for query[i - 1] before j - 1, with the gap it leaves.
        let (mut gapped, mut gapped_from) = (NONE, 0);
        for j in i..n {
            if i > 0 && j >= 2 {
                gapped -= GAP_EXTEND;
                if best[i - 1][j - 2] - GAP_START > gapped {
                    (gapped, gapped_from) = (best[i - 1][j - 2] - GAP_START, j - 2);
                }
            }
            if folded[j] != query[i] {
                continue;
            }
            let here = MATCH + bonus[j];
            if i == 0 {
                best[0][j] = here;
                continue;
            }
            let next_to = best[i - 1][j - 1] + CONSECUTIVE;
            let (before, k) = if next_to >= gapped {
                (next_to, j - 1)
            } else {
                (gapped, gapped_from)
            };
            if before > NONE / 2 {
                best[i][j] = here + before;
                from[i][j] = k;
            }
        }
    }

    // The first of the best ends.
    let (mut j, score) = (m - 1..n)
        .map(|j| (j, best[m - 1][j]))
        .rev()
        .max_by_key(|(_, s)| *s)?;
    if score <= NONE / 2 {
        return None;
    }
    let mut hits = vec![0; m];
    for i in (0..m).rev() {
        hits[i] = j;
        j = from[i][j];
    }
    Some((score, hits))
}

#[cfg(test)]
mod tests {
    use super::score;

    fn hits(query: &str, text: &str) -> Option<Vec<usize>> {
        score(query, text).map(|(_, hits)| hits)
    }

    #[test]
    fn matches_in_order() {
        assert_eq!(hits("", "gerdr"), Some(vec![]));
        assert_eq!(hits("gdr", "gerdr"), Some(vec![0, 3, 4]));
        assert_eq!(hits("rg", "gerdr"), None);
        assert_eq!(hits("gerdrs", "gerdr"), None);
        assert_eq!(hits("g r", "gerdr"), hits("gr", "gerdr"));
    }

    #[test]
    fn smart_case() {
        assert!(score("g", "Gerdr").is_some());
        assert!(score("G", "gerdr").is_none());
        assert!(score("G", "Gerdr").is_some());
    }

    #[test]
    fn prefers_word_starts_and_runs() {
        assert_eq!(hits("api", "rapid services-api"), Some(vec![15, 16, 17]));
        assert_eq!(hits("sa", "services-api"), Some(vec![0, 9]));
        assert_eq!(hits("fb", "FooBar"), Some(vec![0, 3]));
        let s = |query, text| score(query, text).unwrap().0;
        assert!(s("ger", "gerdr") > s("ger", "gxexr"));
        assert!(s("ab", "a_b") > s("ab", "xaxb"));
    }
}
