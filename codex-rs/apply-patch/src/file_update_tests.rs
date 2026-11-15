use super::*;
use crate::Hunk;
use crate::apply_patch;
use crate::parse_patch;
use codex_exec_server::LOCAL_FS;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use std::fs;
use tempfile::tempdir;

fn wrap_patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

#[tokio::test]
async fn test_unified_diff() {
    // Start with a file containing four lines.
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi.txt");
    fs::write(&path, "foo\nbar\nbaz\nqux\n").unwrap();
    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
 foo
-bar
+BAR
@@
 baz
-qux
+QUX"#,
        path.display()
    ));
    let patch = parse_patch(&patch).unwrap();

    let update_file_chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let diff = unified_diff_from_chunks(
        &path_uri,
        update_file_chunks,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let expected_diff = r#"@@ -1,4 +1,4 @@
 foo
-bar
+BAR
 baz
-qux
+QUX
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        original_content: "foo\nbar\nbaz\nqux\n".to_string(),
        content: "foo\nBAR\nbaz\nQUX\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_first_line_replacement() {
    // Replace the very first line of the file.
    let dir = tempdir().unwrap();
    let path = dir.path().join("first.txt");
    fs::write(&path, "foo\nbar\nbaz\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
-foo
+FOO
 bar
"#,
        path.display()
    ));

    let patch = parse_patch(&patch).unwrap();
    let chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let resolved_path = PathUri::from_host_native_path(&path).expect("absolute test path");
    let diff = unified_diff_from_chunks(
        &resolved_path,
        chunks,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let expected_diff = r#"@@ -1,2 +1,2 @@
-foo
+FOO
 bar
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        original_content: "foo\nbar\nbaz\n".to_string(),
        content: "FOO\nbar\nbaz\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_last_line_replacement() {
    // Replace the very last line of the file.
    let dir = tempdir().unwrap();
    let path = dir.path().join("last.txt");
    fs::write(&path, "foo\nbar\nbaz\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
 foo
 bar
-baz
+BAZ
"#,
        path.display()
    ));

    let patch = parse_patch(&patch).unwrap();
    let chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let resolved_path = PathUri::from_host_native_path(&path).expect("absolute test path");
    let diff = unified_diff_from_chunks(
        &resolved_path,
        chunks,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let expected_diff = r#"@@ -2,2 +2,2 @@
 bar
-baz
+BAZ
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        original_content: "foo\nbar\nbaz\n".to_string(),
        content: "foo\nbar\nBAZ\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_insert_at_eof() {
    // Insert a new line at end-of-file.
    let dir = tempdir().unwrap();
    let path = dir.path().join("insert.txt");
    fs::write(&path, "foo\nbar\nbaz\n").unwrap();

    let patch = wrap_patch(&format!(
        r#"*** Update File: {}
@@
+quux
*** End of File
"#,
        path.display()
    ));

    let patch = parse_patch(&patch).unwrap();
    let chunks = match patch.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let diff =
        unified_diff_from_chunks(&path_uri, chunks, LOCAL_FS.as_ref(), /*sandbox*/ None)
            .await
            .unwrap();
    let expected_diff = r#"@@ -3 +3,2 @@
 baz
+quux
"#;
    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        original_content: "foo\nbar\nbaz\n".to_string(),
        content: "foo\nbar\nbaz\nquux\n".to_string(),
    };
    assert_eq!(expected, diff);
}

#[tokio::test]
async fn test_unified_diff_interleaved_changes() {
    // Original file with six lines.
    let dir = tempdir().unwrap();
    let path = dir.path().join("interleaved.txt");
    fs::write(&path, "a\nb\nc\nd\ne\nf\n").unwrap();

    // Patch replaces two separate lines and appends a new one at EOF using
    // three distinct chunks.
    let patch_body = format!(
        r#"*** Update File: {}
@@
 a
-b
+B
@@
 d
-e
+E
@@
 f
+g
*** End of File"#,
        path.display()
    );
    let patch = wrap_patch(&patch_body);

    // Extract chunks then build the unified diff.
    let parsed = parse_patch(&patch).unwrap();
    let chunks = match parsed.hunks.as_slice() {
        [Hunk::UpdateFile { chunks, .. }] => chunks,
        _ => panic!("Expected a single UpdateFile hunk"),
    };

    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let diff =
        unified_diff_from_chunks(&path_uri, chunks, LOCAL_FS.as_ref(), /*sandbox*/ None)
            .await
            .unwrap();

    let expected_diff = r#"@@ -1,6 +1,7 @@
 a
-b
+B
 c
 d
-e
+E
 f
+g
"#;

    let expected = ApplyPatchFileUpdate {
        unified_diff: expected_diff.to_string(),
        original_content: "a\nb\nc\nd\ne\nf\n".to_string(),
        content: "a\nB\nc\nd\nE\nf\ng\n".to_string(),
    };

    assert_eq!(expected, diff);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_patch(
        &patch,
        &PathUri::from_host_native_path(dir.path()).expect("absolute test path"),
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    let contents = fs::read_to_string(path).unwrap();
    assert_eq!(
        contents,
        r#"a
B
c
d
E
f
g
"#
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
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

const WORDS: &[&str] = &[
    "a",
    "b",
    "fn x()",
    "  indented",
    "\t}",
    "é",
    "数据",
    "—",
    "“q”",
    "",
];

fn random_text(rng: &mut Lcg) -> String {
    (0..rng.below(3))
        .map(|_| WORDS[rng.below(WORDS.len())])
        .collect::<Vec<_>>()
        .join(" ")
}

fn random_file(rng: &mut Lcg) -> String {
    let mut contents = String::new();
    for _ in 0..rng.below(14) {
        contents.push_str(&random_text(rng));
        contents.push_str(["\n", "\n", "\r\n", "\r"][rng.below(4)]);
    }
    if rng.below(3) == 0 {
        contents.push_str(&random_text(rng));
    }
    contents
}

fn file_lines(contents: &str) -> Vec<String> {
    contents.split(['\n', '\r']).map(str::to_string).collect()
}

fn random_chunk(rng: &mut Lcg, lines: &[String]) -> UpdateFileChunk {
    let old_lines = if rng.below(6) == 0 || lines.is_empty() {
        Vec::new()
    } else {
        let from = rng.below(lines.len());
        let count = 1 + rng.below((lines.len() - from).min(4));
        let mut old = lines[from..from + count].to_vec();
        match rng.below(5) {
            0 => old.push(String::new()),
            1 => old[0] = format!("  {} ", old[0]),
            2 => old.push(random_text(rng)),
            _ => {}
        }
        old
    };
    let mut new_lines = (0..rng.below(4))
        .map(|_| random_text(rng))
        .collect::<Vec<_>>();
    if rng.below(4) == 0 {
        new_lines.push(String::new());
    }
    let mut context_line_indices = Vec::new();
    let (mut old, mut new) = (0, 0);
    while rng.below(3) != 0 && old < old_lines.len() + 1 {
        old += rng.below(2);
        new += rng.below(2);
        context_line_indices.push((old, new));
        old += 1;
        new += 1;
    }
    UpdateFileChunk {
        change_context: (rng.below(4) == 0 && !lines.is_empty())
            .then(|| lines[rng.below(lines.len())].clone()),
        old_lines,
        new_lines,
        context_line_indices,
        is_end_of_file: rng.below(5) == 0,
    }
}

#[test]
fn ziran_update_agrees_with_the_rust_pipeline_on_generated_patches() {
    let mut rng = Lcg(0xa991_7e57);
    let mut applied = 0;
    for _ in 0..3000 {
        let contents = random_file(&mut rng);
        let lines = file_lines(&contents);
        let chunks = (0..rng.below(4))
            .map(|_| random_chunk(&mut rng, &lines))
            .collect::<Vec<_>>();
        let ziran = apply_chunks(&contents, "file.txt", &chunks);
        let rust = rust_apply_chunks(&contents, "file.txt", &chunks);
        assert_eq!(
            format!("{ziran:?}"),
            format!("{rust:?}"),
            "contents={contents:?} chunks={chunks:?}"
        );
        applied += usize::from(rust.is_ok());
    }
    assert!(applied > 1000, "only {applied} generated patches applied");
}

#[test]
fn ziran_update_agrees_on_line_endings_and_empty_files() {
    let append = UpdateFileChunk {
        change_context: None,
        old_lines: Vec::new(),
        new_lines: vec!["tail".to_string()],
        context_line_indices: Vec::new(),
        is_end_of_file: true,
    };
    for contents in ["", "\n", "x", "x\r\ny", "a\rb\rc", "\r\n\r\n", "one\ntwo\n"] {
        for chunks in [Vec::new(), vec![append.clone()]] {
            assert_eq!(
                format!("{:?}", apply_chunks(contents, "f", &chunks)),
                format!("{:?}", rust_apply_chunks(contents, "f", &chunks)),
                "contents={contents:?}"
            );
        }
    }
}

/// `cargo test -p codex-apply-patch --release --lib -- --ignored --nocapture update_bench`
#[test]
#[ignore = "benchmark"]
fn update_bench() {
    let contents = (0..50_000)
        .map(|index| format!("    let value_{index} = compute({index});\n"))
        .collect::<String>();
    let chunks = (0..20)
        .map(|chunk| {
            let at = chunk * 2_400 + 17;
            UpdateFileChunk {
                change_context: None,
                old_lines: vec![format!("    let value_{at} = compute({at});")],
                new_lines: vec![
                    format!("    let value_{at} = compute({at}) + 1;"),
                    String::new(),
                ],
                context_line_indices: Vec::new(),
                is_end_of_file: false,
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        apply_chunks(&contents, "f", &chunks).unwrap(),
        rust_apply_chunks(&contents, "f", &chunks).unwrap()
    );
    for (label, update) in [
        (
            "rust ",
            rust_apply_chunks
                as fn(&str, &str, &[UpdateFileChunk]) -> Result<String, ApplyPatchError>,
        ),
        ("ziran", apply_chunks),
    ] {
        let started = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(update(&contents, "f", &chunks).unwrap());
        }
        println!(
            "update 50,000 lines, 20 hunks {label} {:>7.2} ms",
            started.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
    }
}
