use std::hint::black_box;
use std::time::Instant;

type Match = Option<(Vec<usize>, i32)>;

fn rust_reference(haystack: &str, needle: &str) -> Match {
    if needle.is_empty() {
        return Some((Vec::new(), i32::MAX));
    }
    let mut lowered_chars = Vec::new();
    let mut lowered_to_original = Vec::new();
    for (original, character) in haystack.chars().enumerate() {
        for lowered in character.to_lowercase() {
            lowered_chars.push(lowered);
            lowered_to_original.push(original);
        }
    }
    let lowered_needle: Vec<char> = needle.to_lowercase().chars().collect();
    let mut indices = Vec::with_capacity(lowered_needle.len());
    let mut first = None;
    let mut last = None;
    let mut cursor = 0;
    for wanted in &lowered_needle {
        let mut found = None;
        while cursor < lowered_chars.len() {
            if lowered_chars[cursor] == *wanted {
                found = Some(cursor);
                cursor += 1;
                break;
            }
            cursor += 1;
        }
        let position = found?;
        indices.push(lowered_to_original[position]);
        first.get_or_insert(position);
        last = Some(position);
    }
    let first = first?;
    let last = last.unwrap_or(first);
    let mut score = ((last as i32 - first as i32 + 1) - lowered_needle.len() as i32).max(0);
    if first == 0 {
        score -= 100;
    }
    indices.sort_unstable();
    indices.dedup();
    Some((indices, score))
}

struct Scenario {
    name: &'static str,
    haystack: String,
    needle: &'static str,
}

fn measure(function: fn(&str, &str) -> Match, scenario: &Scenario, iterations: u64) -> (u128, u64) {
    let started = Instant::now();
    let mut checksum = 0u64;
    for _ in 0..iterations {
        let result = black_box(function(
            black_box(scenario.haystack.as_str()),
            black_box(scenario.needle),
        ));
        match result {
            Some((indices, score)) => {
                checksum = checksum.wrapping_add(score as i64 as u64);
                for index in indices {
                    checksum = checksum.rotate_left(7) ^ index as u64;
                }
            }
            None => checksum = checksum.wrapping_add(0x9e37_79b9),
        }
    }
    (started.elapsed().as_nanos(), black_box(checksum))
}

fn median(values: &mut [u128]) -> u128 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn main() {
    let iterations = std::env::var("XCODER_FUZZY_BENCH_ITERS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(200_000);
    let samples = std::env::var("XCODER_FUZZY_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(9);
    let scenarios = [
        Scenario {
            name: "project_path",
            haystack: "/home/wao/Projects/harness/harmony/src/session_browser.zi".into(),
            needle: "hsb",
        },
        Scenario {
            name: "unicode",
            haystack: "İstanbul/Σίσυφος/ПРОЕКТ/文件/🙂-session".into(),
            needle: "is",
        },
        Scenario {
            name: "long_match",
            haystack: format!("{}AlphaDeviceTask", "projects/session/runtime/".repeat(48)),
            needle: "adt",
        },
        Scenario {
            name: "long_miss",
            haystack: "projects/session/runtime/".repeat(64),
            needle: "zq",
        },
    ];

    for scenario in &scenarios {
        let before = rust_reference(&scenario.haystack, scenario.needle);
        let after = codex_utils_fuzzy_match::fuzzy_match(&scenario.haystack, scenario.needle);
        assert_eq!(before, after, "behavior mismatch for {}", scenario.name);
        for _ in 0..20_000 {
            black_box(rust_reference(&scenario.haystack, scenario.needle));
            black_box(codex_utils_fuzzy_match::fuzzy_match(
                &scenario.haystack,
                scenario.needle,
            ));
        }
    }

    println!("scenario\timplementation\tsample\titerations\telapsed_ns\tchecksum");
    let mut below_target = false;
    for (scenario_index, scenario) in scenarios.iter().enumerate() {
        let mut elapsed_samples = [Vec::new(), Vec::new()];
        for sample in 0..samples {
            let order = (sample + scenario_index as u32) % 2;
            for implementation in [order, 1 - order] {
                let (label, function): (&str, fn(&str, &str) -> Match) = if implementation == 0 {
                    ("rust_before", rust_reference)
                } else {
                    ("ziran_after", codex_utils_fuzzy_match::fuzzy_match)
                };
                let (elapsed_ns, checksum) = measure(function, scenario, iterations);
                elapsed_samples[implementation as usize].push(elapsed_ns);
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    scenario.name, label, sample, iterations, elapsed_ns, checksum
                );
            }
        }
        let rust_median = median(&mut elapsed_samples[0]);
        let ziran_median = median(&mut elapsed_samples[1]);
        let ratio = rust_median as f64 / ziran_median as f64;
        println!(
            "summary\t{}\trust_before_ns={}\tziran_after_ns={}\tratio={ratio:.4}",
            scenario.name, rust_median, ziran_median
        );
        below_target |= ratio < 1.20;
    }
    if below_target {
        eprintln!("Ziran fuzzy matching did not reach the required 1.20x median speedup");
        std::process::exit(2);
    }
}
