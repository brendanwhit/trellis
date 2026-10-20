/// Case-insensitive fuzzy match. Returns a score where lower is better.
/// Consecutive character matches get a bonus (-1 each), while gaps add the
/// text position to the score. An exact match of "abc" in "abc" scores -2.
/// Returns None if not all query characters are found in order.
pub fn fuzzy_match(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();

    if query.is_empty() {
        return Some(0);
    }

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut last_match: i32 = -2;

    for (ti, &ch) in text.iter().enumerate() {
        if qi < query.len() && ch == query[qi] {
            let ti_i32 = ti as i32;
            if ti_i32 == last_match + 1 {
                score -= 1;
            } else {
                score += ti_i32;
            }
            last_match = ti_i32;
            qi += 1;
        }
    }

    if qi < query.len() {
        None
    } else {
        Some(score)
    }
}

/// Like `fuzzy_match`, but splits `query` on whitespace into tokens and
/// requires every token to match `text` as a subsequence independently
/// (fzf-style "AND" semantics). Each token matches in order within itself;
/// order between tokens doesn't matter. Returns the sum of per-token scores,
/// or `None` if any token fails to match. Empty or all-whitespace query
/// returns `Some(0)`.
pub fn fuzzy_match_multi(query: &str, text: &str) -> Option<i32> {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.is_empty() {
        return Some(0);
    }
    let mut total = 0i32;
    for token in tokens {
        match fuzzy_match(token, text) {
            Some(s) => total += s,
            None => return None,
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
    }

    #[test]
    fn exact_match() {
        assert_eq!(fuzzy_match("abc", "abc"), Some(-2));
    }

    #[test]
    fn no_match() {
        assert_eq!(fuzzy_match("xyz", "abc"), None);
    }

    #[test]
    fn partial_match_fails() {
        assert_eq!(fuzzy_match("abcd", "abc"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_match("ABC", "abc").is_some());
    }

    #[test]
    fn gap_penalized_by_position() {
        assert_eq!(fuzzy_match("ac", "abc"), Some(2));
    }

    #[test]
    fn better_match_scores_lower() {
        let close = fuzzy_match("ab", "ab___").unwrap();
        let far = fuzzy_match("ab", "a___b").unwrap();
        assert!(close < far);
    }

    #[test]
    fn multi_tokens_must_all_match() {
        assert!(fuzzy_match_multi("feat dup", "feat/duplicate-x").is_some());
    }

    #[test]
    fn multi_returns_none_if_any_token_missing() {
        assert_eq!(fuzzy_match_multi("feat xyz", "feat/duplicate-x"), None);
    }

    #[test]
    fn multi_empty_or_whitespace_returns_zero() {
        assert_eq!(fuzzy_match_multi("", "anything"), Some(0));
        assert_eq!(fuzzy_match_multi("   ", "anything"), Some(0));
    }

    #[test]
    fn multi_single_token_equals_fuzzy_match() {
        let a = fuzzy_match("feat", "feature-branch").unwrap();
        let b = fuzzy_match_multi("feat", "feature-branch").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn multi_literal_space_in_query_no_longer_blocks_match() {
        // The bug this fix addresses: plain fuzzy_match fails on a literal
        // space character. fuzzy_match_multi splits on whitespace and succeeds.
        assert_eq!(fuzzy_match("feat dup", "feat/duplicate-x"), None);
        assert!(fuzzy_match_multi("feat dup", "feat/duplicate-x").is_some());
    }
}
