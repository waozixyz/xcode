/// Simple case-insensitive subsequence matcher used for fuzzy filtering.
///
/// Returns the indices (character positions) of the matched characters in the
/// ORIGINAL `haystack` string and a score where smaller is better.
///
/// Unicode correctness: we perform the match on a lowercased copy of the
/// haystack and needle but maintain a mapping from each character in the
/// lowercased haystack back to the original character index in `haystack`.
/// This ensures the returned indices can be safely used with
/// `str::chars().enumerate()` consumers for highlighting, even when
/// lowercasing expands certain characters (e.g., İ → i̇).
pub fn fuzzy_match(haystack: &str, needle: &str) -> Option<(Vec<usize>, i32)> {
    if needle.is_empty() {
        return Some((Vec::new(), i32::MAX));
    }

    let mut scratch = vec![0u32; needle.len().saturating_mul(3)];
    let mut indices = vec![0usize; needle.len()];
    let mut score = 0i32;
    let count = unsafe {
        xcode_fuzzy_match(
            haystack.as_ptr(),
            haystack.len() as i64,
            needle.as_ptr(),
            needle.len() as i64,
            scratch.as_mut_ptr(),
            scratch.len() as i64,
            indices.as_mut_ptr().cast::<i64>(),
            indices.len() as i64,
            &mut score,
        )
    };
    if count < 0 {
        return None;
    }
    indices.truncate(count as usize);
    indices.sort_unstable();
    indices.dedup();
    Some((indices, score))
}

unsafe extern "C" {
    fn xcode_fuzzy_match(
        haystack: *const u8,
        haystack_length: i64,
        needle: *const u8,
        needle_length: i64,
        scratch: *mut u32,
        scratch_capacity: i64,
        indices: *mut i64,
        index_capacity: i64,
        score: *mut i32,
    ) -> i64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_basic_indices() {
        let (idx, score) = match fuzzy_match("hello", "hl") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 2]);
        // 'h' at 0, 'l' at 2 -> window 1; start-of-string bonus applies (-100)
        assert_eq!(score, -99);
    }

    #[test]
    fn unicode_dotted_i_istanbul_highlighting() {
        let (idx, score) = match fuzzy_match("İstanbul", "is") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 1]);
        // Matches at lowered positions 0 and 2 -> window 1; start-of-string bonus applies
        assert_eq!(score, -99);
    }

    #[test]
    fn unicode_german_sharp_s_casefold() {
        assert!(fuzzy_match("straße", "strasse").is_none());
    }

    #[test]
    fn prefer_contiguous_match_over_spread() {
        let (_idx_a, score_a) = match fuzzy_match("abc", "abc") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        let (_idx_b, score_b) = match fuzzy_match("a-b-c", "abc") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        // Contiguous window -> 0; start-of-string bonus -> -100
        assert_eq!(score_a, -100);
        // Spread over 5 chars for 3-letter needle -> window 2; with bonus -> -98
        assert_eq!(score_b, -98);
        assert!(score_a < score_b);
    }

    #[test]
    fn start_of_string_bonus_applies() {
        let (_idx_a, score_a) = match fuzzy_match("file_name", "file") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        let (_idx_b, score_b) = match fuzzy_match("my_file_name", "file") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        // Start-of-string contiguous -> window 0; bonus -> -100
        assert_eq!(score_a, -100);
        // Non-prefix contiguous -> window 0; no bonus -> 0
        assert_eq!(score_b, 0);
        assert!(score_a < score_b);
    }

    #[test]
    fn empty_needle_matches_with_max_score_and_no_indices() {
        let (idx, score) = match fuzzy_match("anything", "") {
            Some(v) => v,
            None => panic!("empty needle should match"),
        };
        assert!(idx.is_empty());
        assert_eq!(score, i32::MAX);
    }

    #[test]
    fn case_insensitive_matching_basic() {
        let (idx, score) = match fuzzy_match("FooBar", "foO") {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0, 1, 2]);
        // Contiguous prefix match (case-insensitive) -> window 0 with bonus
        assert_eq!(score, -100);
    }

    #[test]
    fn indices_are_deduped_for_multichar_lowercase_expansion() {
        let needle = "\u{0069}\u{0307}"; // "i" + combining dot above
        let (idx, score) = match fuzzy_match("İ", needle) {
            Some(v) => v,
            None => panic!("expected a match"),
        };
        assert_eq!(idx, vec![0]);
        // Lowercasing 'İ' expands to two chars; contiguous prefix -> window 0 with bonus
        assert_eq!(score, -100);
    }
}
