//! The rules-file language: the declarative Starlark subset `.rules` files use.

use codex_execpolicy::Decision;
use codex_execpolicy::Error;
use codex_execpolicy::Policy;
use codex_execpolicy::PolicyParser;

fn parse(source: &str) -> Result<Policy, Error> {
    let mut parser = PolicyParser::new();
    parser.parse("test.rules", source)?;
    Ok(parser.build())
}

fn decision(policy: &Policy, command: &[&str]) -> Decision {
    let command = command
        .iter()
        .map(|token| token.to_string())
        .collect::<Vec<_>>();
    policy.check(&command, &|_| Decision::Allow).decision
}

fn syntax_error(source: &str) -> (String, usize) {
    match parse(source) {
        Err(Error::Syntax { message, location }) => (message, location.range.start.line),
        other => panic!("expected a syntax error for {source:?}, got {other:?}"),
    }
}

#[test]
fn strings_decode_starlark_escapes_and_quote_forms() {
    let policy = parse(
        r#"
prefix_rule(pattern = ["a\tb", 'single', "\x41é\U0001F642\101", r"raw\n", """tri"ple""", '''x'''], decision = "forbidden")
"#,
    )
    .unwrap();
    assert_eq!(
        decision(
            &policy,
            &["a\tb", "single", "Aé🙂A", "raw\\n", "tri\"ple", "x"]
        ),
        Decision::Forbidden
    );
}

#[test]
fn assignments_names_addition_tuples_and_comments_evaluate() {
    let policy = parse(
        r#"
# Shared prefix for git commands.
GIT = ["git"]   # trailing comment
DANGEROUS = ("reset", "push")
prefix_rule(
    pattern = GIT + [list(DANGEROUS_PLACEHOLDER)] if False else GIT + ["reset"],
)
"#,
    );
    // Conditional expressions are not part of the subset.
    assert!(policy.is_err());

    let policy = parse(
        r#"
# Shared prefix for git commands.
GIT = ["git"]   # trailing comment
DANGEROUS = ["reset", "push"]
JUSTIFICATION = "rewrites " + "history"
prefix_rule(GIT + [DANGEROUS], "forbidden", justification = JUSTIFICATION,)
prefix_rule(pattern = ("ls",), decision = None)
prefix_rule(pattern = ["cat"], canonical_matching = False); prefix_rule(pattern = ["rm"], decision = "prompt")
"#,
    )
    .unwrap();
    assert_eq!(
        decision(&policy, &["git", "push", "--force"]),
        Decision::Forbidden
    );
    assert_eq!(decision(&policy, &["ls", "-l"]), Decision::Allow);
    assert_eq!(decision(&policy, &["rm", "x"]), Decision::Prompt);
}

#[test]
fn unsupported_statements_report_their_line() {
    for (source, line) in [
        (
            "prefix_rule(pattern = [\"ls\"])\n\ndef helper():\n    pass\n",
            3,
        ),
        ("for x in [1]:\n    pass\n", 1),
        ("load(\"x.star\", \"y\")\n", 1),
        ("\n\nprefix_rule(pattern = [f\"{x}\"])\n", 3),
        ("prefix_rule(pattern = [\"ls\"]\n", 2),
        ("prefix_rule(pattern = [\"unterminated])\n", 1),
        ("x = [1, 2\nprefix_rule(pattern = x)\n", 2),
        ("prefix_rule(pattern = [\"a\\q\"])\n", 1),
    ] {
        let (message, reported) = syntax_error(source);
        assert!(
            message.starts_with("Parse error: "),
            "{source:?}: {message}"
        );
        assert_eq!(reported, line, "{source:?}: {message}");
    }
}

#[test]
fn bad_calls_report_the_call_line_and_reason() {
    for (source, expected) in [
        (
            "\nunknown_rule(pattern = [\"ls\"])\n",
            "Variable `unknown_rule` not found",
        ),
        (
            "\nprefix_rule(pattern = [\"ls\"], colour = \"red\")\n",
            "extra named parameter",
        ),
        (
            "\nprefix_rule(decision = \"allow\")\n",
            "Missing parameter `pattern`",
        ),
        (
            "\nprefix_rule(pattern = \"ls\")\n",
            "expected `list`, actual `string`",
        ),
        (
            "\nprefix_rule(pattern = [\"ls\"], pattern = [\"cat\"])\n",
            "occurs both",
        ),
        (
            "\nprefix_rule(pattern = UNDEFINED)\n",
            "Variable `UNDEFINED` not found",
        ),
        (
            "\nprefix_rule(pattern = [\"ls\"] + \"x\")\n",
            "not supported for types `list` and `string`",
        ),
        (
            "\nnetwork_rule(host = \"a.com\", protocol = \"https\")\n",
            "Missing parameter `decision`",
        ),
    ] {
        let (message, line) = syntax_error(source);
        assert!(message.contains(expected), "{source:?}: {message}");
        assert_eq!(line, 2, "{source:?}");
    }
}

#[test]
fn syntax_errors_keep_the_path_line_column_shape() {
    let error = parse("\n\nprefix_rule(pattern = [\"ls\"], decision = \"nope\")\n").unwrap_err();
    let rendered = error.to_string();
    assert!(
        rendered.starts_with("test.rules:3:1: starlark error: error: "),
        "{rendered}"
    );
    assert_eq!(
        error.location().map(|location| location.range.start.line),
        Some(3)
    );
}

#[test]
fn the_example_policy_parses() {
    let source = include_str!("../examples/example.codexpolicy");
    let policy = parse(source).unwrap();
    assert_eq!(
        decision(&policy, &["git", "reset", "--hard"]),
        Decision::Forbidden
    );
}
