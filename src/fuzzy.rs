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
    // A longer query strewn over the text in bits that start no word
    // ("restore" over "Turn off terminal config reload") is no match:
    // it would list rows that have nothing to do with it.
    let loose = (0..m)
        .filter(|&i| (i == 0 || hits[i - 1] + 1 != hits[i]) && bonus[hits[i]] == 0)
        .count();
    if m >= LOOSE_FROM && loose > (m / 4).max(1) {
        return None;
    }
    Some((score, hits))
}

/// How long a query is before its bits must mostly start words (see
/// `score`).
const LOOSE_FROM: usize = 4;

/// An item `rank` kept: how well the query matched it and where (see
/// `score`).
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked<T> {
    pub item: T,
    pub score: i32,
    pub hits: Vec<usize>,
}

/// The `items` that `query` matches (all of them for an empty query), the
/// best match first; among equal matches the one with the higher `weight`
/// (its frecency), and among those the earlier one. `text` is what the
/// query is matched against.
pub fn rank<T>(
    query: &str,
    items: impl IntoIterator<Item = T>,
    text: impl Fn(&T) -> String,
    weight: impl Fn(&T) -> f32,
) -> Vec<Ranked<T>> {
    let mut ranked: Vec<(f32, Ranked<T>)> = items
        .into_iter()
        .filter_map(|item| {
            let (score, hits) = score(query, &text(&item))?;
            let weight = weight(&item);
            Some((weight, Ranked { item, score, hits }))
        })
        .collect();
    // A stable sort: equal scores and weights keep the items' order.
    ranked.sort_by(|(wa, a), (wb, b)| {
        b.score
            .cmp(&a.score)
            .then_with(|| wb.partial_cmp(wa).unwrap_or(std::cmp::Ordering::Equal))
    });
    ranked.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::{rank, score};

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
    fn a_long_query_strewn_over_the_text_is_no_match() {
        assert_eq!(hits("restore", "xrxexsxtxoxrxe"), None);
        assert!(score("restore", "Restore the worktrees of Bulk").is_some());
        // Words and their starts still are, and short queries as before.
        assert!(score("nfeat", "New feature").is_some());
        assert!(score("gerdr", "the gerdr log").is_some());
        assert!(score("gdr", "gxdxr").is_some());
        // Nor over a path's many words (Find and switch's workspace rows).
        for (label, name) in [
            ("Dark theme for reports", "dark-theme-for-reports"),
            ("Export invoices to JSON", "export-invoices-to-json"),
        ] {
            let row = format!(
                "{label} /home/u/.cache/fwshots/s/root/shop/.features/{name} \u{b7} feature/{name}"
            );
            assert_eq!(hits("restore", &row), None, "{row}");
        }
        assert!(score("restore", "Restore \u{201c}Abandon probe\u{201d} history").is_some());
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

    #[test]
    fn ranks_by_match_then_weight_then_order() {
        let items = [
            ("gerdr", 1.0),
            ("herdr", 5.0),
            ("gxexr", 0.0),
            ("gerdr-old", 1.0),
        ];
        fn names<'a>(ranked: Vec<super::Ranked<&'a (&'a str, f32)>>) -> Vec<&'a str> {
            ranked.into_iter().map(|r| r.item.0).collect()
        }
        let by = |query| rank(query, items.iter(), |i| i.0.into(), |i| i.1);
        // Nothing typed: the weight orders, ties keep their order.
        assert_eq!(names(by("")), ["herdr", "gerdr", "gerdr-old", "gxexr"]);
        // "ger" matches the two gerdrs equally well: the weight can't split
        // them, so the earlier stays first; the gapped match comes last.
        let ranked = by("ger");
        assert_eq!(
            ranked.iter().map(|r| r.item.0).collect::<Vec<_>>(),
            ["gerdr", "gerdr-old", "gxexr"]
        );
        assert_eq!(ranked[0].hits, [0, 1, 2]);
        assert!(ranked[0].score > ranked[2].score);
        // An equal match with more weight goes first.
        let ranked = rank("erdr", items.iter(), |i| i.0.into(), |i| i.1);
        assert_eq!(names(ranked), ["herdr", "gerdr", "gerdr-old"]);
    }
}
