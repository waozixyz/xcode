use std::path::PathBuf;

use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Tree;
use tree_sitter_bash::LANGUAGE as BASH;

use crate::shell_detect::ShellType;
use crate::shell_detect::detect_shell_type;

/// Parse the provided bash source using tree-sitter-bash, returning a Tree on
/// success or None if parsing failed.
pub fn try_parse_shell(shell_lc_arg: &str) -> Option<Tree> {
    let lang = BASH.into();
    let mut parser = Parser::new();
    #[expect(clippy::expect_used)]
    parser.set_language(&lang).expect("load bash grammar");
    let old_tree: Option<&Tree> = None;
    parser.parse(shell_lc_arg, old_tree)
}

/// Parse a script which may contain multiple simple commands joined only by
/// the safe logical/pipe/sequencing operators: `&&`, `||`, `;`, `|`.
///
/// Returns `Some(Vec<command_words>)` if every command is a plain word‑only
/// command and the parse tree does not contain disallowed constructs
/// (parentheses, redirections, substitutions, control flow, etc.). Otherwise
/// returns `None`.
pub fn try_parse_word_only_commands_sequence(tree: &Tree, src: &str) -> Option<Vec<Vec<String>>> {
    if tree.root_node().has_error() {
        return None;
    }

    // List of allowed (named) node kinds for a "word only commands sequence".
    // If we encounter a named node that is not in this list we reject.
    const ALLOWED_KINDS: &[&str] = &[
        // top level containers
        "program",
        "list",
        "pipeline",
        // commands & words
        "command",
        "command_name",
        "word",
        "string",
        "string_content",
        "raw_string",
        "number",
        "concatenation",
    ];
    // Allow only safe punctuation / operator tokens; anything else causes reject.
    const ALLOWED_PUNCT_TOKENS: &[&str] = &["&&", "||", ";", "|", "\"", "'"];

    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut command_nodes = Vec::new();
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if node.is_named() {
            if !ALLOWED_KINDS.contains(&kind) {
                return None;
            }
            if matches!(kind, "word" | "number") && !is_literal_word_or_number(node, src) {
                return None;
            }
            if kind == "command" {
                command_nodes.push(node);
            }
        } else {
            // Reject any punctuation / operator tokens that are not explicitly allowed.
            if kind.chars().any(|c| "&;|".contains(c)) && !ALLOWED_PUNCT_TOKENS.contains(&kind) {
                return None;
            }
            if !(ALLOWED_PUNCT_TOKENS.contains(&kind) || kind.trim().is_empty()) {
                // If it's a quote token or operator it's allowed above; we also allow whitespace tokens.
                // Any other punctuation like parentheses, braces, redirects, backticks, etc are rejected.
                return None;
            }
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    // Walk uses a stack (LIFO), so re-sort by position to restore source order.
    command_nodes.sort_by_key(Node::start_byte);

    let mut commands = Vec::new();
    for node in command_nodes {
        if let Some(words) = parse_plain_command_from_node(node, src) {
            commands.push(words);
        } else {
            return None;
        }
    }
    Some(commands)
}

/// Parses a shell script consisting only of plain commands joined by safe operators.
///
/// Runs the Ziran recognizer in `native/shell.zi`, which is at least as strict
/// as the tree-sitter walk in [`try_parse_word_only_commands_sequence`]: when
/// both accept a script they return the same words, and it never accepts a
/// script tree-sitter rejects (checked by the parity tests below).
pub fn parse_shell_script_into_commands(script: &str) -> Option<Vec<Vec<String>>> {
    // Every word byte, word and command comes from at least one script byte.
    let capacity = script.len() + 1;
    let mut bytes = vec![0u8; capacity];
    let mut word_ends = vec![0i64; capacity];
    let mut command_words = vec![0i64; capacity];
    // SAFETY: each buffer outlives the call and has the capacity passed for it.
    let commands = unsafe {
        xcoder_parse_plain_commands(
            script.as_ptr(),
            script.len() as i64,
            bytes.as_mut_ptr(),
            capacity as i64,
            word_ends.as_mut_ptr(),
            capacity as i64,
            command_words.as_mut_ptr(),
            capacity as i64,
        )
    };
    let commands = usize::try_from(commands).ok()?;
    let mut word = 0;
    let mut start = 0;
    let mut parsed = Vec::with_capacity(commands);
    for &count in &command_words[..commands] {
        let mut words = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let end = word_ends[word] as usize;
            // Words are whole UTF-8 runs of the script: quotes and separators are ASCII.
            words.push(String::from_utf8(bytes[start..end].to_vec()).ok()?);
            start = end;
            word += 1;
        }
        parsed.push(words);
    }
    Some(parsed)
}

unsafe extern "C" {
    fn xcoder_parse_plain_commands(
        script: *const u8,
        script_length: i64,
        bytes: *mut u8,
        byte_capacity: i64,
        word_ends: *mut i64,
        word_capacity: i64,
        command_words: *mut i64,
        command_capacity: i64,
    ) -> i64;
}

pub fn extract_bash_command(command: &[String]) -> Option<(&str, &str)> {
    let [shell, flag, script] = command else {
        return None;
    };
    if !matches!(flag.as_str(), "-lc" | "-c")
        || !matches!(
            detect_shell_type(PathBuf::from(shell)),
            Some(ShellType::Zsh) | Some(ShellType::Bash) | Some(ShellType::Sh)
        )
    {
        return None;
    }
    Some((shell, script))
}

/// Returns the sequence of plain commands within a `bash -lc "..."` or
/// `zsh -lc "..."` invocation when the script only contains word-only commands
/// joined by safe operators.
pub fn parse_shell_lc_plain_commands(command: &[String]) -> Option<Vec<Vec<String>>> {
    let (_, script) = extract_bash_command(command)?;
    parse_shell_script_into_commands(script)
}

/// Extracts the literal portions of command invocations from a shell script.
///
/// Unlike [`parse_shell_lc_plain_commands`], this accepts complex shell syntax
/// and returns the statically known words from every command node in a valid
/// syntax tree. Dynamic words and redirections are omitted. This is suitable
/// for identifying dangerous literal commands, but must not be used to prove
/// that a command is safe.
pub(crate) fn parse_shell_lc_literal_commands(command: &[String]) -> Option<Vec<Vec<String>>> {
    let (_, script) = extract_bash_command(command)?;
    let tree = try_parse_shell(script)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }

    let mut commands = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "command"
            && let Some(command) = parse_literal_command_from_node(node, script)
        {
            commands.push(command);
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }

    Some(commands)
}

fn parse_plain_command_from_node(cmd: tree_sitter::Node, src: &str) -> Option<Vec<String>> {
    if cmd.kind() != "command" {
        return None;
    }
    let mut words = Vec::new();
    let mut cursor = cmd.walk();
    for child in cmd.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let word_node = child.named_child(0)?;
                if word_node.kind() != "word" {
                    return None;
                }
                words.push(word_node.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "word" | "number" => {
                words.push(child.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "string" => {
                let parsed = parse_double_quoted_string(child, src)?;
                words.push(parsed);
            }
            "raw_string" => {
                let parsed = parse_raw_string(child, src)?;
                words.push(parsed);
            }
            "concatenation" => {
                // Handle concatenated arguments like -g"*.py"
                let mut concatenated = String::new();
                let mut concat_cursor = child.walk();
                for part in child.named_children(&mut concat_cursor) {
                    match part.kind() {
                        "word" | "number" => {
                            concatenated
                                .push_str(part.utf8_text(src.as_bytes()).ok()?.to_owned().as_str());
                        }
                        "string" => {
                            let parsed = parse_double_quoted_string(part, src)?;
                            concatenated.push_str(&parsed);
                        }
                        "raw_string" => {
                            let parsed = parse_raw_string(part, src)?;
                            concatenated.push_str(&parsed);
                        }
                        _ => return None,
                    }
                }
                if concatenated.is_empty() {
                    return None;
                }
                words.push(concatenated);
            }
            _ => return None,
        }
    }
    Some(words)
}

fn parse_literal_command_from_node(cmd: Node<'_>, src: &str) -> Option<Vec<String>> {
    if cmd.kind() != "command" {
        return None;
    }

    let mut words = Vec::new();
    let mut found_command_name = false;
    let mut cursor = cmd.walk();
    for child in cmd.named_children(&mut cursor) {
        if child.kind() == "command_name" {
            let command_name = parse_literal_shell_word(child.named_child(0)?, src)?;
            words.push(command_name);
            found_command_name = true;
        } else if found_command_name && let Some(word) = parse_literal_shell_word(child, src) {
            words.push(word);
        }
    }

    found_command_name.then_some(words)
}

fn parse_literal_shell_word(node: Node<'_>, src: &str) -> Option<String> {
    match node.kind() {
        "word" | "number" if is_literal_word_or_number(node, src) => {
            Some(node.utf8_text(src.as_bytes()).ok()?.to_owned())
        }
        "string" => parse_double_quoted_string(node, src),
        "raw_string" => parse_raw_string(node, src),
        "concatenation" => {
            let mut concatenated = String::new();
            let mut cursor = node.walk();
            for part in node.named_children(&mut cursor) {
                concatenated.push_str(&parse_literal_shell_word(part, src)?);
            }
            (!concatenated.is_empty()).then_some(concatenated)
        }
        _ => None,
    }
}

fn is_literal_word_or_number(node: Node<'_>, src: &str) -> bool {
    if !matches!(node.kind(), "word" | "number") {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next().is_none()
        && node.utf8_text(src.as_bytes()).is_ok_and(|word| {
            // A tree-sitter word can still undergo shell expansion or escape
            // removal. Do not use its source spelling as proof of runtime argv.
            // Include Zsh's equals expansion and extended glob syntax because
            // this parser is also used for Zsh commands.
            !word.starts_with('=')
                && !word.contains(['{', '}', '*', '?', '[', ']', '\\', '~', '^', '#', '$', '`'])
        })
}

fn parse_double_quoted_string(node: Node, src: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }

    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        if part.kind() != "string_content" {
            return None;
        }
    }
    let raw = node.utf8_text(src.as_bytes()).ok()?;
    let stripped = raw
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))?;
    // Double quotes suppress globbing and brace expansion, but not escape
    // removal. Only accept contents whose source spelling is already literal.
    if stripped
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'\\' && matches!(pair[1], b'$' | b'`' | b'"' | b'\\' | b'\n'))
    {
        return None;
    }
    Some(stripped.to_owned())
}

fn parse_raw_string(node: Node, src: &str) -> Option<String> {
    if node.kind() != "raw_string" {
        return None;
    }

    let raw_string = node.utf8_text(src.as_bytes()).ok()?;
    let stripped = raw_string
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''));
    stripped.map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parse_seq(src: &str) -> Option<Vec<Vec<String>>> {
        let parsed = parse_shell_script_into_commands(src);
        assert_parity(src, &parsed);
        parsed
    }

    /// The tree-sitter walk that plain-command parsing used before Ziran.
    fn tree_sitter_plain(src: &str) -> Option<Vec<Vec<String>>> {
        let tree = try_parse_shell(src)?;
        try_parse_word_only_commands_sequence(&tree, src)
    }

    /// The Ziran recognizer may reject more than tree-sitter, never less, and
    /// never returns different words. Returns whether only Ziran rejected.
    fn assert_parity(src: &str, ziran: &Option<Vec<Vec<String>>>) -> bool {
        if let Some(problem) = parity_problem(src, ziran) {
            panic!("{problem}");
        }
        ziran.is_none() && tree_sitter_plain(src).is_some()
    }

    fn parity_problem(src: &str, ziran: &Option<Vec<Vec<String>>>) -> Option<String> {
        let tree = || try_parse_shell(src).map(|tree| tree.root_node().to_sexp());
        match (ziran, tree_sitter_plain(src)) {
            (Some(ziran), Some(tree_sitter)) if *ziran != tree_sitter => Some(format!(
                "different words for {src:?}: Ziran {ziran:?}, tree-sitter {tree_sitter:?}"
            )),
            (Some(ziran), None) => Some(format!(
                "Ziran accepted {src:?} as {ziran:?}; tree-sitter rejects it: {:?}",
                tree()
            )),
            _ => None,
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, bound: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) % bound as u64) as usize
        }
    }

    const PIECES: &[&str] = &[
        "ls",
        "git",
        "status",
        "-l",
        "--opt=val",
        "a=b",
        "x+=1",
        "10",
        "-1",
        "./a.out",
        "path/to",
        "'single quoted'",
        "'it''s'",
        "\"plain\"",
        "\"a b\"",
        "\"$HOME\"",
        "\"\\n\"",
        "\"a\\\"b\"",
        "\"`id`\"",
        "$HOME",
        "${x}",
        "$(id)",
        "`id`",
        "*.rs",
        "?",
        "[ab]",
        "{a,b}",
        "~",
        "~/x",
        "\\x",
        "#comment",
        "!",
        "hi!",
        "=x",
        "if",
        "then",
        "fi",
        "for",
        "do",
        "done",
        "time",
        "{",
        "}",
        "[[",
        "]]",
        "-g\"*.py\"",
        "a'b'c",
        "é",
        "数据",
        "%",
        "@",
        "+",
        ",",
        ":",
        ".",
        "^",
        "a^b",
    ];
    const JOINS: &[&str] = &[
        " ", " ", " ", "  ", "\t", " && ", " || ", "; ", ";", " | ", "\n", " & ", " > out",
        " < in", " 2>&1", " |& ", " ;; ", "(", ")", " &&\n", "\\\n", "\r\n", "",
    ];

    // Pieces and joins a plain script is made of, drawn twice as often so that
    // both the accepting and the rejecting paths are exercised.
    const SAFE_PIECES: &[&str] = &[
        "ls",
        "git",
        "status",
        "-l",
        "--opt=val",
        "a=b",
        "10",
        "-1",
        "./a.out",
        "path/to",
        "'single quoted'",
        "\"plain\"",
        "\"a b\"",
        "-g\"*.py\"",
        "a'b'c",
        "é",
        "数据",
        "%",
        "@",
        "+",
        ",",
        ":",
        ".",
        "-",
        "--",
    ];
    const SAFE_JOINS: &[&str] = &[
        " ", " ", " ", "\t", " && ", " || ", "; ", " | ", "\n", " &&\n",
    ];

    fn random_script(rng: &mut Lcg) -> String {
        let mut script = String::new();
        for _ in 0..1 + rng.below(8) {
            let (pieces, joins) = if rng.below(3) == 0 {
                (PIECES, JOINS)
            } else {
                (SAFE_PIECES, SAFE_JOINS)
            };
            script.push_str(pieces[rng.below(pieces.len())]);
            script.push_str(joins[rng.below(joins.len())]);
        }
        script
    }

    #[test]
    fn ziran_plain_commands_are_never_looser_than_tree_sitter() {
        let mut rng = Lcg(0x5afe_5be1);
        let (mut accepted, mut stricter) = (0, 0);
        let mut problems = std::collections::BTreeSet::new();
        for _ in 0..50_000 {
            let script = random_script(&mut rng);
            let ziran = parse_shell_script_into_commands(&script);
            if let Some(problem) = parity_problem(&script, &ziran) {
                problems.insert(problem);
                continue;
            }
            stricter += usize::from(ziran.is_none() && tree_sitter_plain(&script).is_some());
            accepted += usize::from(ziran.is_some());
        }
        let shown = problems.iter().take(15).cloned().collect::<Vec<_>>();
        assert!(
            problems.is_empty(),
            "{} parity problems:\n{}",
            problems.len(),
            shown.join("\n")
        );
        assert!(
            accepted > 3_000,
            "only {accepted} generated scripts were plain"
        );
        println!("accepted {accepted}, rejected only by Ziran {stricter}");
    }

    /// `XCODER_SHELL_CORPUS=commands.jsonl cargo test -p codex-shell-command
    /// --lib -- --ignored --nocapture shell_corpus`: one JSON string per line.
    #[test]
    #[ignore = "needs a local corpus of real commands"]
    fn shell_corpus() {
        let path = std::env::var("XCODER_SHELL_CORPUS").expect("XCODER_SHELL_CORPUS");
        let corpus = std::fs::read_to_string(path).expect("read corpus");
        let (mut total, mut accepted, mut stricter) = (0, 0, 0);
        for line in corpus.lines() {
            let Ok(script) = serde_json::from_str::<String>(line) else {
                continue;
            };
            total += 1;
            let ziran = parse_shell_script_into_commands(&script);
            if assert_parity(&script, &ziran) {
                stricter += 1;
                println!("rejected only by Ziran: {script:?}");
            }
            accepted += usize::from(ziran.is_some());
        }
        println!("{total} scripts, {accepted} plain, {stricter} rejected only by Ziran");
    }

    #[test]
    fn accepts_single_simple_command() {
        let cmds = parse_seq("ls -1").unwrap();
        assert_eq!(cmds, vec![vec!["ls".to_string(), "-1".to_string()]]);
    }

    #[test]
    fn accepts_multiple_commands_with_allowed_operators() {
        let src = "ls && pwd; echo 'hi there' | wc -l";
        let cmds = parse_seq(src).unwrap();
        let expected: Vec<Vec<String>> = vec![
            vec!["ls".to_string()],
            vec!["pwd".to_string()],
            vec!["echo".to_string(), "hi there".to_string()],
            vec!["wc".to_string(), "-l".to_string()],
        ];
        assert_eq!(cmds, expected);
    }

    #[test]
    fn extracts_double_and_single_quoted_strings() {
        let cmds = parse_seq("echo \"hello world\"").unwrap();
        assert_eq!(
            cmds,
            vec![vec!["echo".to_string(), "hello world".to_string()]]
        );

        let cmds2 = parse_seq("echo 'hi there'").unwrap();
        assert_eq!(
            cmds2,
            vec![vec!["echo".to_string(), "hi there".to_string()]]
        );
    }

    #[test]
    fn accepts_double_quoted_strings_with_newlines() {
        let cmds = parse_seq("git commit -m \"line1\nline2\"").unwrap();
        assert_eq!(
            cmds,
            vec![vec![
                "git".to_string(),
                "commit".to_string(),
                "-m".to_string(),
                "line1\nline2".to_string(),
            ]]
        );
    }

    #[test]
    fn accepts_mixed_quote_concatenation() {
        assert_eq!(
            parse_seq(r#"echo "/usr"'/'"local"/bin"#).unwrap(),
            vec![vec!["echo".to_string(), "/usr/local/bin".to_string()]]
        );
        assert_eq!(
            parse_seq(r#"echo '/usr'"/"'local'/bin"#).unwrap(),
            vec![vec!["echo".to_string(), "/usr/local/bin".to_string()]]
        );
    }

    #[test]
    fn rejects_double_quoted_strings_with_expansions() {
        assert!(parse_seq(r#"echo "hi ${USER}""#).is_none());
        assert!(parse_seq(r#"echo "$HOME""#).is_none());
    }

    #[test]
    fn accepts_numbers_as_words() {
        let cmds = parse_seq("echo 123 456").unwrap();
        assert_eq!(
            cmds,
            vec![vec![
                "echo".to_string(),
                "123".to_string(),
                "456".to_string()
            ]]
        );
    }

    #[test]
    fn rejects_parentheses_and_subshells() {
        assert!(parse_seq("(ls)").is_none());
        assert!(parse_seq("ls || (pwd && echo hi)").is_none());
    }

    #[test]
    fn rejects_redirections_and_unsupported_operators() {
        assert!(parse_seq("ls > out.txt").is_none());
        assert!(parse_seq("echo hi & echo bye").is_none());
    }

    #[test]
    fn rejects_command_and_process_substitutions_and_expansions() {
        assert!(parse_seq("echo $(pwd)").is_none());
        assert!(parse_seq("echo `pwd`").is_none());
        assert!(parse_seq("echo $HOME").is_none());
        assert!(parse_seq("echo \"hi $USER\"").is_none());
    }

    #[test]
    fn rejects_runtime_expansion_in_plain_words() {
        for script in [
            "find . -{delete,print}",
            "rg --pre{=,=sh} pattern payload.sh",
            "find . -del*",
            "find . -delet?",
            "find . -delet[e]",
            r"find . -de\lete",
            "echo ~",
            "echo ~HOME",
            "echo HEAD~1",
            "echo HEAD^",
            "echo file~",
            "echo =sh",
            "echo foo^bar",
            "echo foo#bar",
            "l* -l",
        ] {
            for shell in ["bash", "zsh"] {
                for flag in ["-c", "-lc"] {
                    let command = [shell, flag, script].map(str::to_owned);
                    assert_eq!(parse_shell_lc_plain_commands(&command), None, "{command:?}");
                }
            }
        }
    }

    #[test]
    fn preserves_quoted_literals() {
        for (script, expected) in [
            (r#"rg -g"*.py" pattern"#, vec!["rg", "-g*.py", "pattern"]),
            (r#"echo "\n""#, vec!["echo", r"\n"]),
            (
                r#"echo "~HOME" 'HEAD~1' "HEAD^" 'foo#bar' "=sh" 'file~'"#,
                vec![
                    "echo", "~HOME", "HEAD~1", "HEAD^", "foo#bar", "=sh", "file~",
                ],
            ),
            (
                r#"echo -"{a,b}" '*?[]~^#=\\'"#,
                vec!["echo", "-{a,b}", r"*?[]~^#=\\"],
            ),
        ] {
            let expected = vec![expected.into_iter().map(str::to_owned).collect::<Vec<_>>()];
            assert_eq!(
                parse_shell_script_into_commands(script),
                Some(expected),
                "{script:?}"
            );
        }
    }

    #[test]
    fn rejects_double_quoted_escapes() {
        for script in [
            r#"echo "\$HOME\`\"\\\n""#,
            "find . \"-de\\\nlete\"",
            r#"echo "\\""#,
        ] {
            assert_eq!(parse_seq(script), None, "{script:?}");
            for shell in ["bash", "zsh"] {
                let command = [shell, "-lc", script].map(str::to_owned);
                assert_eq!(parse_shell_lc_plain_commands(&command), None, "{command:?}");
            }
        }
    }

    #[test]
    fn rejects_variable_assignment_prefix() {
        assert!(parse_seq("FOO=bar ls").is_none());
    }

    #[test]
    fn rejects_trailing_operator_parse_error() {
        assert!(parse_seq("ls &&").is_none());
    }

    #[test]
    fn rejects_empty_command_position_with_leading_operator() {
        assert!(parse_seq("&& ls").is_none());
    }

    #[test]
    fn rejects_empty_command_position_with_double_separator() {
        assert!(parse_seq("ls ;; pwd").is_none());
    }

    #[test]
    fn rejects_empty_command_position_with_empty_pipeline_segment() {
        assert!(parse_seq("ls | | wc").is_none());
    }

    #[test]
    fn parse_zsh_lc_plain_commands() {
        let command = vec!["zsh".to_string(), "-lc".to_string(), "ls".to_string()];
        let parsed = parse_shell_lc_plain_commands(&command).unwrap();
        assert_eq!(parsed, vec![vec!["ls".to_string()]]);
    }

    #[test]
    fn accepts_concatenated_flag_and_value() {
        // Test case: -g"*.py" (flag directly concatenated with quoted value)
        let cmds = parse_seq("rg -n \"foo\" -g\"*.py\"").unwrap();
        assert_eq!(
            cmds,
            vec![vec![
                "rg".to_string(),
                "-n".to_string(),
                "foo".to_string(),
                "-g*.py".to_string(),
            ]]
        );
    }

    #[test]
    fn accepts_concatenated_flag_with_single_quotes() {
        let cmds = parse_seq("grep -n 'pattern' -g'*.txt'").unwrap();
        assert_eq!(
            cmds,
            vec![vec![
                "grep".to_string(),
                "-n".to_string(),
                "pattern".to_string(),
                "-g*.txt".to_string(),
            ]]
        );
    }

    #[test]
    fn rejects_concatenation_with_variable_substitution() {
        // Environment variables in concatenated strings should be rejected
        assert!(parse_seq("rg -g\"$VAR\" pattern").is_none());
        assert!(parse_seq("rg -g\"${VAR}\" pattern").is_none());
    }

    #[test]
    fn rejects_concatenation_with_command_substitution() {
        // Command substitution in concatenated strings should be rejected
        assert!(parse_seq("rg -g\"$(pwd)\" pattern").is_none());
        assert!(parse_seq("rg -g\"$(echo '*.py')\" pattern").is_none());
    }
}
