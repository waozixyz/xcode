use super::*;
use pretty_assertions::assert_eq;

/// Deterministic xorshift, so failures reproduce without a seed file.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

fn edited_pair(rng: &mut Rng) -> (String, String) {
    let count = [0, 1, 2, 5, 12, 40, 150][rng.below(7)];
    let alphabet = [2, 5, 30, 1000][rng.below(4)];
    let endings: &[&str] = [&["\n"][..], &["\n", "\r\n"], &["\n", "\r", "\r\n"]][rng.below(3)];
    let line = |rng: &mut Rng, tag: &str| {
        format!("{tag}{} é{}", rng.below(alphabet), endings[rng.below(endings.len())])
    };
    let old: Vec<String> = (0..count).map(|_| line(rng, "l")).collect();
    let mut new = old.clone();
    for _ in 0..rng.below(8) {
        let at = rng.below(new.len() + 1);
        match rng.below(4) {
            0 => new.insert(at, line(rng, "n")),
            1 if !new.is_empty() => {
                new.remove(at.min(new.len() - 1));
            }
            2 if !new.is_empty() => {
                let at = at.min(new.len() - 1);
                new[at] = line(rng, "r");
            }
            3 if !new.is_empty() => {
                let moved = new.remove(rng.below(new.len()));
                new.insert(at.min(new.len()), moved);
            }
            _ => {}
        }
    }
    let mut old = old.concat();
    let mut new = new.concat();
    if rng.below(5) == 0 {
        old.truncate(old.trim_end_matches(['\r', '\n']).len());
    }
    if rng.below(5) == 0 {
        new.truncate(new.trim_end_matches(['\r', '\n']).len());
    }
    (old, new)
}

fn similar_unified(old: &str, new: &str, context: usize) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(context)
        .header("a/file", "b/file")
        .to_string()
}

#[test]
fn unified_matches_similar_on_generated_edits() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for round in 0..3000 {
        let (old, new) = edited_pair(&mut rng);
        let context = [0, 1, 2, 3, 5][rng.below(5)];
        let expected = similar_unified(&old, &new, context);
        let actual = unified(&old, &new)
            .context_radius(context)
            .header("a/file", "b/file")
            .render();
        assert_eq!(actual, expected, "round {round}: {old:?} -> {new:?} context {context}");
    }
}

#[test]
fn unified_handles_edge_inputs() {
    for (old, new) in [
        ("", ""),
        ("", "x"),
        ("x", ""),
        ("a\nb\nc\n", "a\nb\nc\n"),
        ("a\r\nb", "a\nb"),
        ("\n\n\n", "\n"),
        ("a\rb\rc", "a\rc\r"),
    ] {
        for context in [0, 3] {
            assert_eq!(
                unified(old, new).context_radius(context).header("a/file", "b/file").render(),
                similar_unified(old, new, context),
                "{old:?} -> {new:?}"
            );
        }
    }
    assert_eq!(unified("a\n", "b\n").render(), "@@ -1 +1 @@\n-a\n+b\n");
}

#[test]
fn large_file_matches_similar() {
    let old: String = (0..20_000).map(|i| format!("line {}\n", i % 997)).collect();
    let new: String = old
        .lines()
        .enumerate()
        .filter(|(i, _)| i % 101 != 7)
        .map(|(i, line)| if i % 333 == 5 { format!("{line} changed\n") } else { format!("{line}\n") })
        .collect();
    assert_eq!(
        unified(&old, &new).header("a/file", "b/file").render(),
        similar_unified(&old, &new, 3)
    );
}

#[test]
fn budget_still_produces_a_valid_patch() {
    let old: String = (0..2_000).map(|i| format!("{}\n", i * 7 % 13)).collect();
    let new: String = (0..2_000).map(|i| format!("{}\n", i * 11 % 17)).collect();
    let patch = unified(&old, &new).context_radius(100_000).budget(1).render();
    let parsed = Patch::from_str(&patch).expect("budgeted diff parses");
    let mut rebuilt = String::new();
    for hunk in parsed.hunks() {
        for line in hunk.lines() {
            if let Line::Insert(text) | Line::Context(text) = line {
                rebuilt.push_str(text);
            }
        }
    }
    assert_eq!(rebuilt, new);
}

fn diffy_view(text: &str) -> Option<Vec<(usize, usize, usize, usize, Vec<(char, String)>)>> {
    let patch = diffy::Patch::from_str(text).ok()?;
    Some(
        patch
            .hunks()
            .iter()
            .map(|hunk| {
                let lines = hunk
                    .lines()
                    .iter()
                    .map(|line| match line {
                        diffy::Line::Context(text) => (' ', text.to_string()),
                        diffy::Line::Delete(text) => ('-', text.to_string()),
                        diffy::Line::Insert(text) => ('+', text.to_string()),
                    })
                    .collect();
                (
                    hunk.old_range().start(),
                    hunk.old_range().len(),
                    hunk.new_range().start(),
                    hunk.new_range().len(),
                    lines,
                )
            })
            .collect(),
    )
}

fn native_view(text: &str) -> Option<Vec<(usize, usize, usize, usize, Vec<(char, String)>)>> {
    let patch = Patch::from_str(text).ok()?;
    Some(
        patch
            .hunks()
            .iter()
            .map(|hunk| {
                let lines = hunk
                    .lines()
                    .iter()
                    .map(|line| match line {
                        Line::Context(text) => (' ', text.to_string()),
                        Line::Delete(text) => ('-', text.to_string()),
                        Line::Insert(text) => ('+', text.to_string()),
                    })
                    .collect();
                (
                    hunk.old_range().start(),
                    hunk.old_range().len(),
                    hunk.new_range().start(),
                    hunk.new_range().len(),
                    lines,
                )
            })
            .collect(),
    )
}

#[test]
fn parse_matches_diffy_on_generated_and_damaged_patches() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for round in 0..2000 {
        let (old, new) = edited_pair(&mut rng);
        let mut patch = similar_unified(&old, &new, rng.below(4));
        if rng.below(3) == 0 {
            patch.insert_str(0, "diff --git a/file b/file\nindex 0000000..1111111 100644\n");
        }
        // Damage some patches: drop, duplicate or alter one line.
        if rng.below(2) == 0 && !patch.is_empty() {
            let mut lines: Vec<&str> = patch.split_inclusive('\n').collect();
            let at = rng.below(lines.len());
            let replacement: &str = ["", "@@ -1 +1 @@\n", "x\n", "\\ No newline at end of file\n", " \n"]
                [rng.below(5)];
            match rng.below(3) {
                0 => {
                    lines.remove(at);
                }
                1 => lines.insert(at, lines[at]),
                _ => lines[at] = replacement,
            }
            patch = lines.concat();
        }
        assert_eq!(native_view(&patch), diffy_view(&patch), "round {round}: {patch:?}");
    }
}

#[test]
fn parse_keeps_names_and_function_context() {
    let text = "--- \"a\\tb\"\t2026\n+++ b/file\n@@ -1 +1 @@ fn main\n-x\n+y\n";
    let patch = Patch::from_str(text).expect("parse");
    assert_eq!(patch.original(), Some("\"a\\tb\""));
    assert_eq!(patch.modified(), Some("b/file"));
    assert_eq!(patch.hunks()[0].function_context(), Some("fn main\n"));
    assert!(Patch::from_str("--- a\n--- b\n").is_err());
    assert!(diffy::Patch::from_str("--- a\n--- b\n").is_err());
}
