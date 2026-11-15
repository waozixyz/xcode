/// Attempt to find the sequence of `pattern` lines within `lines` beginning at or after `start`.
/// Returns the starting index of the match or `None` if not found. Matches are attempted with
/// decreasing strictness: exact match, then ignoring trailing whitespace, then ignoring leading
/// and trailing whitespace. When `eof` is true, we first try starting at the end-of-file (so that
/// patterns intended to match file endings are applied at the end), and fall back to searching
/// from `start` if needed.
///
/// Special cases handled defensively:
///  • Empty `pattern` → returns `Some(start)` (no-op match)
///  • `pattern.len() > lines.len()` → returns `None` (cannot match, avoids
///    out‑of‑bounds panic that occurred pre‑2025‑04‑12)
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    // The Ziran matcher borrows each line's bytes; only the pointer and length
    // tables are built here.
    let pointers =
        |strings: &[String]| strings.iter().map(|line| line.as_ptr()).collect::<Vec<_>>();
    let lengths = |strings: &[String]| {
        strings
            .iter()
            .map(|line| line.len() as i64)
            .collect::<Vec<_>>()
    };
    let (line_data, line_lengths) = (pointers(lines), lengths(lines));
    let (pattern_data, pattern_lengths) = (pointers(pattern), lengths(pattern));
    if line_lengths
        .iter()
        .chain(&pattern_lengths)
        .any(|length| *length > i64::from(i32::MAX))
    {
        return rust_seek_sequence(lines, pattern, start, eof);
    }
    // SAFETY: every pointer and length describes a live `String` for the call.
    let found = unsafe {
        xcoder_seek_sequence(
            line_data.as_ptr(),
            line_lengths.as_ptr(),
            lines.len() as i64,
            pattern_data.as_ptr(),
            pattern_lengths.as_ptr(),
            pattern.len() as i64,
            start as i64,
            i32::from(eof),
        )
    };
    usize::try_from(found).ok()
}

unsafe extern "C" {
    fn xcoder_seek_sequence(
        lines: *const *const u8,
        line_lengths: *const i64,
        line_count: i64,
        pattern: *const *const u8,
        pattern_lengths: *const i64,
        pattern_count: i64,
        start: i64,
        eof: i32,
    ) -> i64;
}

/// The original Rust matcher: the fallback for lines above 2 GiB, which the
/// Ziran matcher's 32-bit offsets cannot address, and the oracle its parity
/// tests compare against.
fn rust_seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }

    // When the pattern is longer than the available input there is no possible
    // match. Early‑return to avoid the out‑of‑bounds slice that would occur in
    // the search loops below (previously caused a panic when
    // `pattern.len() > lines.len()`).
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        let eof_start = lines.len() - pattern.len();
        eof_start.max(start)
    } else {
        start
    };
    // Exact match first.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    // Then rstrip match.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim_end() != pat.trim_end() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }
    // Finally, trim both sides to allow more lenience.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim() != pat.trim() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    // ------------------------------------------------------------------
    // Final, most permissive pass – attempt to match after *normalising*
    // common Unicode punctuation to their ASCII equivalents so that diffs
    // authored with plain ASCII characters can still be applied to source
    // files that contain typographic dashes / quotes, etc.  This mirrors the
    // fuzzy behaviour of `git apply` which ignores minor byte-level
    // differences when locating context lines.
    // ------------------------------------------------------------------

    fn normalise(s: &str) -> String {
        s.trim()
            .chars()
            .map(|c| match c {
                // Various dash / hyphen code-points → ASCII '-'
                '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
                | '\u{2212}' => '-',
                // Fancy single quotes → '\''
                '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
                // Fancy double quotes → '"'
                '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
                // Non-breaking space and other odd spaces → normal space
                '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
                | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
                | '\u{3000}' => ' ',
                other => other,
            })
            .collect::<String>()
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if normalise(&lines[i + p_idx]) != normalise(pat) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::rust_seek_sequence;
    use super::seek_sequence;
    use std::string::ToString;

    fn to_vec(strings: &[&str]) -> Vec<String> {
        strings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn test_exact_match_finds_sequence() {
        let lines = to_vec(&["foo", "bar", "baz"]);
        let pattern = to_vec(&["bar", "baz"]);
        assert_eq!(
            seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ false,),
            Some(1)
        );
    }

    #[test]
    fn test_rstrip_match_ignores_trailing_whitespace() {
        let lines = to_vec(&["foo   ", "bar\t\t"]);
        // Pattern omits trailing whitespace.
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(
            seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ false,),
            Some(0)
        );
    }

    #[test]
    fn test_trim_match_ignores_leading_and_trailing_whitespace() {
        let lines = to_vec(&["    foo   ", "   bar\t"]);
        // Pattern omits any additional whitespace.
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(
            seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ false,),
            Some(0)
        );
    }

    #[test]
    fn test_pattern_longer_than_input_returns_none() {
        let lines = to_vec(&["just one line"]);
        let pattern = to_vec(&["too", "many", "lines"]);
        // Should not panic – must return None when pattern cannot possibly fit.
        assert_eq!(
            seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ false,),
            None
        );
    }

    /// Deterministic pseudo-random source, so failures reproduce exactly.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    // ASCII, multi-byte text, every Unicode White_Space class Rust trims, and
    // the typographic punctuation the final pass normalises.
    const PIECES: &[&str] = &[
        "fn", "let", "x", "=", "-", "'", "\"", "{", "}", "é", "数据", "🙂", " ", "\t", "\u{0b}",
        "\u{0c}", "\r", "\u{85}", "\u{a0}", "\u{1680}", "\u{2002}", "\u{2009}", "\u{200a}",
        "\u{2028}", "\u{2029}", "\u{202f}", "\u{205f}", "\u{3000}", "\u{2010}", "\u{2013}",
        "\u{2014}", "\u{2212}", "\u{2018}", "\u{2019}", "\u{201c}", "\u{201d}", "\u{200b}",
        "\u{feff}",
    ];

    fn random_line(rng: &mut Lcg) -> String {
        (0..rng.below(7))
            .map(|_| PIECES[rng.below(PIECES.len())])
            .collect()
    }

    // A pattern line that should match the original only under some pass.
    fn mutate(rng: &mut Lcg, line: &str) -> String {
        match rng.below(7) {
            0 => line.to_string(),
            1 => format!("{line}  \t"),
            2 => format!("\u{3000} {line}\u{a0}"),
            3 => line.trim().to_string(),
            4 => line.replace('-', "\u{2014}").replace('\'', "\u{2019}"),
            5 => line.replace('\u{2013}', "-").replace('\u{201c}', "\""),
            _ => random_line(rng),
        }
    }

    fn assert_parity(lines: &[String], pattern: &[String], start: usize, eof: bool) -> bool {
        let expected = rust_seek_sequence(lines, pattern, start, eof);
        assert_eq!(
            seek_sequence(lines, pattern, start, eof),
            expected,
            "lines={lines:?} pattern={pattern:?} start={start} eof={eof}"
        );
        expected.is_some()
    }

    #[test]
    fn ziran_matcher_agrees_with_the_rust_matcher_on_generated_cases() {
        let mut rng = Lcg(0x5eed_a991_c0de);
        let mut found = 0;
        for _ in 0..4000 {
            let lines = (0..rng.below(12))
                .map(|_| random_line(&mut rng))
                .collect::<Vec<_>>();
            let pattern = if !lines.is_empty() && rng.below(4) != 0 {
                let from = rng.below(lines.len());
                let count = 1 + rng.below(lines.len() - from);
                lines[from..from + count]
                    .iter()
                    .map(|line| mutate(&mut rng, line))
                    .collect()
            } else {
                (0..rng.below(4))
                    .map(|_| random_line(&mut rng))
                    .collect::<Vec<_>>()
            };
            let start = rng.below(lines.len() + 2);
            let eof = rng.below(2) == 1;
            found += usize::from(assert_parity(&lines, &pattern, start, eof));
        }
        assert!(found > 1000, "only {found} generated cases matched");
    }

    #[test]
    fn ziran_matcher_agrees_on_edge_cases() {
        let empty: Vec<String> = Vec::new();
        let blank = to_vec(&["", "", ""]);
        let unicode = to_vec(&[
            "\u{3000}café\u{a0}",
            "  — “quoted” ‘text’ −1  ",
            "\u{85}x\u{2028}",
        ]);
        for (lines, pattern) in [
            (&empty, &empty),
            (&empty, &to_vec(&["a"])),
            (&blank, &to_vec(&[""])),
            (&blank, &to_vec(&["", "", "", ""])),
            (&unicode, &to_vec(&["café", "- \"quoted\" 'text' -1"])),
            (&unicode, &to_vec(&["x"])),
            (&unicode, &to_vec(&["\u{200b}x"])),
        ] {
            for start in 0..=lines.len() + 1 {
                for eof in [false, true] {
                    let _ = assert_parity(lines, pattern, start, eof);
                }
            }
        }
    }

    #[test]
    fn ziran_matcher_agrees_on_a_large_file() {
        let mut rng = Lcg(42);
        let lines = (0..20_000)
            .map(|index| format!("    line {index} {}", random_line(&mut rng)))
            .collect::<Vec<_>>();
        for at in [0, 1, 9_999, 19_990] {
            let pattern = lines[at..at + 5]
                .iter()
                .map(|line| line.trim().replace('-', "\u{2013}"))
                .collect::<Vec<_>>();
            for start in [0, at, at + 1] {
                for eof in [false, true] {
                    let found = assert_parity(&lines, &pattern, start, eof);
                    // With eof the search begins at the last possible window.
                    assert_eq!(
                        found,
                        !eof && start <= at,
                        "lines {at} from {start}, eof {eof}"
                    );
                }
            }
        }
        assert!(!assert_parity(&lines, &to_vec(&["absent"]), 0, false));
    }

    /// `cargo test -p codex-apply-patch --release -- --ignored --nocapture seek_bench`
    #[test]
    #[ignore = "benchmark"]
    fn seek_bench() {
        let mut rng = Lcg(7);
        let lines = (0..50_000)
            .map(|index| {
                format!(
                    "    let value_{index} = compute(\"{}\");",
                    random_line(&mut rng)
                )
            })
            .collect::<Vec<_>>();
        let tail = |at: usize| lines[at..at + 4].to_vec();
        let cases = [
            ("exact, end of file", tail(49_990), false),
            (
                "trimmed, middle",
                tail(25_000).iter().map(|l| l.trim().to_string()).collect(),
                false,
            ),
            (
                "punctuation, end",
                tail(49_000)
                    .iter()
                    .map(|l| l.trim().replace('"', "\u{201c}"))
                    .collect(),
                false,
            ),
            ("absent", to_vec(&["no such line", "anywhere"]), false),
        ];
        for (name, pattern, eof) in cases {
            for (label, matcher) in [
                (
                    "rust ",
                    rust_seek_sequence as fn(&[String], &[String], usize, bool) -> Option<usize>,
                ),
                ("ziran", seek_sequence),
            ] {
                let started = std::time::Instant::now();
                let mut result = None;
                for _ in 0..20 {
                    result = std::hint::black_box(matcher(&lines, &pattern, 0, eof));
                }
                println!(
                    "{name:20} {label} {:>8.2} ms/search -> {result:?}",
                    started.elapsed().as_secs_f64() * 1000.0 / 20.0
                );
            }
        }
    }
}
