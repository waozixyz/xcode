//! Generated cases whose digests were recorded from the original Rust renderer.

use crate::{RenderError, Span, render, render_spans};

type Text = fn(&str, usize) -> Result<String, RenderError>;
type Spans = fn(&str, usize) -> Result<Vec<Vec<Span>>, RenderError>;

/// FNV-1a over text, roles and error kinds; independent of `Debug` formatting.
fn digest(cases: &[(String, usize)], text: Text, spans: Spans) -> u64 {
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (source, width) in cases {
        match text(source, *width) {
            Ok(output) => feed(&mut hash, output.as_bytes()),
            Err(error) => feed(&mut hash, &[0xff, error as u8]),
        }
        match spans(source, *width) {
            Ok(lines) => {
                for line in lines {
                    for span in line {
                        feed(&mut hash, &[0xfe, span.role as u8]);
                        feed(&mut hash, span.text.as_bytes());
                    }
                    feed(&mut hash, &[0xfd]);
                }
            }
            Err(error) => feed(&mut hash, &[0xff, error as u8]),
        }
        feed(&mut hash, &[0xfc]);
    }
    hash
}

fn flowcharts() -> Vec<(String, usize)> {
    let labels = ["Plain", "请求", "Ship 🚀", "a;b", "&", "x<br>y", "<br>x<br>", "1 < 2", "λ", "é", "", "👍🏽", "لا", "ⴱ⵿ⴱ"];
    let mut cases = Vec::new();
    for direction in ["TD", "BT", "LR", "RL"] {
        for label in labels {
            for edge in ["-->", "---", "-.->", "-.-", "<-->", "<-.->"] {
                let source = format!("graph {direction}; A[\"{label}\"] {edge} B{{Done?}}; B --> A");
                for width in [0, 20, 100, usize::MAX] { cases.push((source.clone(), width)); }
            }
        }
    }
    cases
}

fn bounds() -> Vec<(String, usize)> {
    let mut cases = Vec::new();
    for count in [1, 16, 24, 25, 40, 80, 160, 500] {
        cases.push((format!("graph TD; A[\"{}\"]", "x<br>".repeat(count)), 100));
        cases.push((format!("graph TD; {}", "A;".repeat(count)), 100));
    }
    for space in ["\t", "\r", "\u{85}", "\u{a0}", "\u{1680}", "\u{2003}", "\u{202f}", "\u{3000}", "\u{1b}"] {
        cases.push((format!("{space}graph{space}TD; A --> B{space}"), 100));
        cases.push((format!("sequenceDiagram; A->>B: {space}hello{space}"), 100));
    }
    cases
}

fn unicode() -> Vec<(String, usize)> {
    let mut cases: Vec<_> = (0..=0x10ffff).step_by(97).chain([0x17d8, 0x1f3fb, 0x1faea])
        .filter_map(char::from_u32)
        .map(|ch| (format!("graph; A[\"x{ch}x\"]"), 100))
        .collect();
    for label in ["👍🏽", "🏽🏽", "a🏽", "☝🏽", "لا", "لآ", "ڨا", "ꓸꓼ", "ⴱ⵿ⴱ", "𖵪𖵧", "🇵🇾", "\u{17d8}"] {
        cases.push((format!("graph; A[\"{label}\"]"), 100));
    }
    cases
}

fn inline_storage() -> Vec<(String, usize)> {
    let mut cases = Vec::new();
    for count in [1, 15, 16, 17, 20, 21, 39, 40, 41] {
        let label = "名".repeat(count);
        for source in [
            format!("graph TD; A[\"{label}\"]; A[\"{label}\"]; A -->|{label}| B"),
            format!("sequenceDiagram; actor A as {label}; A->>B: {label}; Note over A,B: {label}"),
            format!("stateDiagram-v2; state \"{label}\" as A; A: {label}; A --> B: {label}"),
            format!("classDiagram\nclass A {{\n{label}\n}}\nA \"{label}\" *-- \"{label}\" B : {label}"),
            format!("erDiagram; A ||--o{{ B : {label}"),
        ] {
            for width in [0, 40, 100, usize::MAX] { cases.push((source.clone(), width)); }
        }
    }
    cases
}

#[test]
fn generated_flowcharts_and_width_boundaries() {
    assert_eq!(digest(&flowcharts(), render, render_spans), 0xc8a9_b4dd_9a48_30cf);
}

#[test]
fn source_bounds_and_multiline_labels() {
    assert_eq!(digest(&bounds(), render, render_spans), 0x5a08_27d6_5f79_8ac1);
}

#[test]
fn unicode_scalar_and_ligature_cases() {
    assert_eq!(digest(&unicode(), render, render_spans), 0xe362_6742_a890_3cac);
}

#[test]
fn labels_cross_inline_storage_in_every_family() {
    assert_eq!(digest(&inline_storage(), render, render_spans), 0xf416_c9b7_9079_2e19);
}
