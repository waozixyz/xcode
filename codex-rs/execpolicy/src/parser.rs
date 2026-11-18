//! Evaluates `.rules` files.
//!
//! The Ziran parser in `native/execpolicy.zi` turns the file into a node tree
//! (top-level calls and `NAME = value` assignments over strings, lists,
//! tuples, integers, booleans, `None`, names and `+`). This module evaluates
//! that tree and calls the three rule builtins. Rules files used to run on a
//! full Starlark interpreter; the declarative subset they actually use is all
//! that is supported now, and anything else is reported with its location.

use codex_utils_absolute_path::AbsolutePathBuf;
use multimap::MultiMap;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::decision::Decision;
use crate::error::Error;
use crate::error::ErrorLocation;
use crate::error::Result;
use crate::error::TextPosition;
use crate::error::TextRange;
use crate::executable_name::executable_lookup_key;
use crate::executable_name::executable_path_lookup_key;
use crate::rule::NetworkRule;
use crate::rule::NetworkRuleProtocol;
use crate::rule::PatternToken;
use crate::rule::PrefixPattern;
use crate::rule::PrefixRule;
use crate::rule::RuleRef;
use crate::rule::validate_match_examples;
use crate::rule::validate_not_match_examples;

pub struct PolicyParser {
    builder: RefCell<PolicyBuilder>,
}

impl Default for PolicyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyParser {
    pub fn new() -> Self {
        Self {
            builder: RefCell::new(PolicyBuilder::new()),
        }
    }

    /// Parses a policy, tagging parser errors with `policy_identifier` so failures include the
    /// identifier alongside line numbers.
    pub fn parse(&mut self, policy_identifier: &str, policy_file_contents: &str) -> Result<()> {
        let pending_validation_count = self.builder.borrow().pending_example_validations.len();
        let statements = parse_rules_file(policy_identifier, policy_file_contents)?;
        let mut variables = HashMap::new();
        for statement in statements {
            match statement {
                Statement::Assign {
                    name,
                    value,
                    location,
                } => {
                    let value = evaluate(&value, &variables)
                        .map_err(|message| syntax_error(message, location))?;
                    variables.insert(name, value);
                }
                Statement::Call {
                    name,
                    arguments,
                    location,
                } => {
                    let arguments = arguments
                        .iter()
                        .map(|(keyword, value)| {
                            evaluate(value, &variables).map(|value| (keyword.clone(), value))
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|message| syntax_error(message, location.clone()))?;
                    let mut builder = self.builder.borrow_mut();
                    call_builtin(&mut builder, &name, arguments, &location)
                        .map_err(|error| syntax_error(error.to_string(), location))?;
                }
            }
        }
        self.builder
            .borrow()
            .validate_pending_examples_from(pending_validation_count)?;
        Ok(())
    }

    pub fn build(self) -> crate::policy::Policy {
        self.builder.into_inner().build()
    }
}

#[derive(Debug)]
struct PolicyBuilder {
    rules_by_program: MultiMap<String, RuleRef>,
    network_rules: Vec<NetworkRule>,
    host_executables_by_name: HashMap<String, Arc<[AbsolutePathBuf]>>,
    pending_example_validations: Vec<PendingExampleValidation>,
}

impl PolicyBuilder {
    fn new() -> Self {
        Self {
            rules_by_program: MultiMap::new(),
            network_rules: Vec::new(),
            host_executables_by_name: HashMap::new(),
            pending_example_validations: Vec::new(),
        }
    }

    fn add_rule(&mut self, rule: RuleRef) {
        self.rules_by_program
            .insert(rule.program().to_string(), rule);
    }

    fn add_network_rule(&mut self, rule: NetworkRule) {
        self.network_rules.push(rule);
    }

    fn add_host_executable(&mut self, name: String, paths: Vec<AbsolutePathBuf>) {
        self.host_executables_by_name.insert(name, paths.into());
    }

    fn add_pending_example_validation(
        &mut self,
        rules: Vec<RuleRef>,
        matches: Vec<Vec<String>>,
        not_matches: Vec<Vec<String>>,
        location: Option<ErrorLocation>,
    ) {
        self.pending_example_validations
            .push(PendingExampleValidation {
                rules,
                matches,
                not_matches,
                location,
            });
    }

    fn validate_pending_examples_from(&self, start: usize) -> Result<()> {
        for validation in &self.pending_example_validations[start..] {
            let mut rules_by_program = MultiMap::new();
            for rule in &validation.rules {
                rules_by_program.insert(rule.program().to_string(), rule.clone());
            }

            let policy = crate::policy::Policy::from_parts(
                rules_by_program,
                Vec::new(),
                self.host_executables_by_name.clone(),
            );
            validate_not_match_examples(&policy, &validation.rules, &validation.not_matches)
                .map_err(|error| attach_validation_location(error, validation.location.clone()))?;
            validate_match_examples(&policy, &validation.rules, &validation.matches)
                .map_err(|error| attach_validation_location(error, validation.location.clone()))?;
        }

        Ok(())
    }

    fn build(self) -> crate::policy::Policy {
        crate::policy::Policy::from_parts(
            self.rules_by_program,
            self.network_rules,
            self.host_executables_by_name,
        )
    }
}

#[derive(Debug)]
struct PendingExampleValidation {
    rules: Vec<RuleRef>,
    matches: Vec<Vec<String>>,
    not_matches: Vec<Vec<String>>,
    location: Option<ErrorLocation>,
}

/// A parsed expression.
#[derive(Debug, Clone)]
enum Expression {
    String(String),
    List(Vec<Expression>),
    Tuple(Vec<Expression>),
    None,
    Bool(bool),
    Integer(i64),
    Name(String),
    Add(Box<Expression>, Box<Expression>),
}

/// An evaluated value.
#[derive(Debug, Clone)]
enum Value {
    String(String),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    None,
    Bool(bool),
    Integer(i64),
}

impl Value {
    /// The Starlark type name, as error messages always used it.
    fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::Tuple(_) => "tuple",
            Value::None => "NoneType",
            Value::Bool(_) => "bool",
            Value::Integer(_) => "int",
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(value) => Some(value),
            _ => None,
        }
    }

    fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) | Value::Tuple(items) => Some(items),
            _ => None,
        }
    }
}

enum Statement {
    Call {
        name: String,
        arguments: Vec<(Option<String>, Expression)>,
        location: ErrorLocation,
    },
    Assign {
        name: String,
        value: Expression,
        location: ErrorLocation,
    },
}

fn syntax_error(message: String, location: ErrorLocation) -> Error {
    Error::Syntax { message, location }
}

fn point_location(path: &str, line: usize, column: usize) -> ErrorLocation {
    let position = TextPosition { line, column };
    ErrorLocation {
        path: path.to_string(),
        range: TextRange {
            start: position.clone(),
            end: position,
        },
    }
}

const PARSE_ERRORS: &[(i64, &str)] = &[
    (1, "Parse error: unexpected character"),
    (2, "Parse error: unterminated string literal"),
    (3, "Parse error: invalid escape sequence"),
    (
        4,
        "Parse error: only calls such as prefix_rule(...) and NAME = value assignments are supported in rules files",
    ),
    (5, "Parse error: expected `(` or `=` after a name"),
    (6, "Parse error: expected `,` or `)`"),
    (7, "Parse error: expected `,` or `]`"),
    (8, "Parse error: expected a value"),
    (9, "Parse error: f-strings are not supported in rules files"),
    (10, "Parse error: values are nested too deeply"),
    (11, "Parse error: integer literal is too large"),
    (12, "Parse error: expected a new line after the statement"),
];

/// Parses `contents` with the Ziran rules parser.
fn parse_rules_file(path: &str, contents: &str) -> Result<Vec<Statement>> {
    // Every node and decoded byte comes from at least one source byte.
    let capacity = contents.len() + 1;
    let mut nodes = vec![0i64; capacity * NODE_FIELDS];
    let mut bytes = vec![0u8; capacity];
    let mut error = [0i64; 3];
    // SAFETY: the buffers outlive the call and have the capacities passed.
    let count = unsafe {
        xcode_parse_rules(
            contents.as_ptr(),
            contents.len() as i64,
            nodes.as_mut_ptr(),
            capacity as i64,
            bytes.as_mut_ptr(),
            capacity as i64,
            error.as_mut_ptr(),
        )
    };
    if count < 0 {
        let [code, line, column] = error;
        let message = PARSE_ERRORS
            .iter()
            .find(|(known, _)| *known == code)
            .map_or(
                "Parse error: the rules file could not be parsed",
                |(_, message)| message,
            );
        return Err(syntax_error(
            message.to_string(),
            point_location(path, line.max(1) as usize, column.max(1) as usize),
        ));
    }
    nodes.truncate(count as usize * NODE_FIELDS);
    let mut reader = NodeReader {
        nodes: &nodes,
        bytes: &bytes,
        at: 0,
        path,
    };
    let mut statements = Vec::new();
    while reader.at < count as usize {
        statements.push(reader.statement()?);
    }
    Ok(statements)
}

const NODE_FIELDS: usize = 5;

struct NodeReader<'a> {
    nodes: &'a [i64],
    bytes: &'a [u8],
    at: usize,
    path: &'a str,
}

impl NodeReader<'_> {
    fn next(&mut self) -> Result<[i64; NODE_FIELDS]> {
        let record = self
            .nodes
            .get(self.at * NODE_FIELDS..(self.at + 1) * NODE_FIELDS)
            .ok_or_else(|| Error::InvalidRule("truncated rules parse".to_string()))?;
        self.at += 1;
        let mut fields = [0; NODE_FIELDS];
        fields.copy_from_slice(record);
        Ok(fields)
    }

    fn text(&self, offset: i64, length: i64) -> Result<String> {
        let range = offset as usize..(offset + length) as usize;
        let bytes = self
            .bytes
            .get(range)
            .ok_or_else(|| Error::InvalidRule("truncated rules parse".to_string()))?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::InvalidRule("rules strings must be valid UTF-8".to_string()))
    }

    fn location(&mut self) -> Result<ErrorLocation> {
        let [_, line, column, end_line, end_column] = self.next()?;
        let mut location = point_location(self.path, line as usize, column as usize);
        if end_line > 0 {
            location.range.end = TextPosition {
                line: end_line as usize,
                column: end_column as usize,
            };
        }
        Ok(location)
    }

    fn statement(&mut self) -> Result<Statement> {
        let [kind, name, name_length, argument_count, _] = self.next()?;
        let name = self.text(name, name_length)?;
        let location = self.location()?;
        match kind {
            10 => {
                let mut arguments = Vec::new();
                for _ in 0..argument_count {
                    let [kind, keyword, keyword_length, _, _] = self.next()?;
                    let keyword = (kind == 11)
                        .then(|| self.text(keyword, keyword_length))
                        .transpose()?;
                    arguments.push((keyword, self.expression()?));
                }
                Ok(Statement::Call {
                    name,
                    arguments,
                    location,
                })
            }
            14 => Ok(Statement::Assign {
                name,
                value: self.expression()?,
                location,
            }),
            _ => Err(Error::InvalidRule(
                "unexpected rules parse node".to_string(),
            )),
        }
    }

    fn expression(&mut self) -> Result<Expression> {
        let [kind, a, b, _, _] = self.next()?;
        Ok(match kind {
            1 => Expression::String(self.text(a, b)?),
            2 | 3 => {
                let items = (0..a)
                    .map(|_| self.expression())
                    .collect::<Result<Vec<_>>>()?;
                if kind == 2 {
                    Expression::List(items)
                } else {
                    Expression::Tuple(items)
                }
            }
            4 => Expression::None,
            5 => Expression::Bool(true),
            6 => Expression::Bool(false),
            7 => Expression::Integer(a),
            8 => Expression::Name(self.text(a, b)?),
            9 => {
                let left = self.expression()?;
                let right = self.expression()?;
                Expression::Add(Box::new(left), Box::new(right))
            }
            _ => {
                return Err(Error::InvalidRule(
                    "unexpected rules parse node".to_string(),
                ));
            }
        })
    }
}

unsafe extern "C" {
    fn xcode_parse_rules(
        source: *const u8,
        source_length: i64,
        nodes: *mut i64,
        node_capacity: i64,
        bytes: *mut u8,
        byte_capacity: i64,
        error: *mut i64,
    ) -> i64;
}

fn evaluate(
    expression: &Expression,
    variables: &HashMap<String, Value>,
) -> std::result::Result<Value, String> {
    Ok(match expression {
        Expression::String(value) => Value::String(value.clone()),
        Expression::List(items) => Value::List(
            items
                .iter()
                .map(|item| evaluate(item, variables))
                .collect::<std::result::Result<_, _>>()?,
        ),
        Expression::Tuple(items) => Value::Tuple(
            items
                .iter()
                .map(|item| evaluate(item, variables))
                .collect::<std::result::Result<_, _>>()?,
        ),
        Expression::None => Value::None,
        Expression::Bool(value) => Value::Bool(*value),
        Expression::Integer(value) => Value::Integer(*value),
        Expression::Name(name) => variables
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Variable `{name}` not found"))?,
        Expression::Add(left, right) => {
            match (evaluate(left, variables)?, evaluate(right, variables)?) {
                (Value::String(left), Value::String(right)) => Value::String(left + &right),
                (Value::List(mut left), Value::List(right)) => {
                    left.extend(right);
                    Value::List(left)
                }
                (Value::Tuple(mut left), Value::Tuple(right)) => {
                    left.extend(right);
                    Value::Tuple(left)
                }
                (Value::Integer(left), Value::Integer(right)) => Value::Integer(
                    left.checked_add(right)
                        .ok_or_else(|| "Integer overflow".to_string())?,
                ),
                (left, right) => {
                    return Err(format!(
                        "Operation `+` not supported for types `{}` and `{}`",
                        left.type_name(),
                        right.type_name()
                    ));
                }
            }
        }
    })
}

/// Binds call arguments to a builtin's parameters as Starlark did: positional
/// arguments in order, then keywords; a parameter passed `None` counts as
/// omitted.
fn bind_arguments(
    function: &str,
    parameters: &[(&str, bool)],
    arguments: Vec<(Option<String>, Value)>,
) -> Result<Vec<Option<Value>>> {
    let mut bound: Vec<Option<Value>> = vec![None; parameters.len()];
    let mut seen = vec![false; parameters.len()];
    let mut position = 0;
    for (keyword, value) in arguments {
        let index = match keyword {
            None => {
                if position >= parameters.len() {
                    return Err(Error::InvalidRule(format!(
                        "Too many positional arguments for call to {function}"
                    )));
                }
                position += 1;
                position - 1
            }
            Some(keyword) => parameters
                .iter()
                .position(|(name, _)| *name == keyword)
                .ok_or_else(|| {
                    Error::InvalidRule(format!(
                        "Found `{keyword}` extra named parameter for call to {function}"
                    ))
                })?,
        };
        if seen[index] {
            return Err(Error::InvalidRule(format!(
                "Parameter `{}` occurs both explicitly and in **kwargs",
                parameters[index].0
            )));
        }
        seen[index] = true;
        bound[index] = (!matches!(value, Value::None)).then_some(value);
    }
    for ((name, required), value) in parameters.iter().zip(&bound) {
        if *required && value.is_none() {
            return Err(Error::InvalidRule(format!(
                "Missing parameter `{name}` for call to {function}"
            )));
        }
    }
    Ok(bound)
}

fn expect_str<'v>(function: &str, parameter: &str, value: &'v Value) -> Result<&'v str> {
    value.as_str().ok_or_else(|| {
        Error::InvalidRule(format!(
            "Type of parameter `{parameter}` doesn't match, expected `str`, actual `{}` (in call to {function})",
            value.type_name()
        ))
    })
}

fn expect_list<'v>(function: &str, parameter: &str, value: &'v Value) -> Result<&'v [Value]> {
    value.as_list().ok_or_else(|| {
        Error::InvalidRule(format!(
            "Type of parameter `{parameter}` doesn't match, expected `list`, actual `{}` (in call to {function})",
            value.type_name()
        ))
    })
}

fn call_builtin(
    builder: &mut PolicyBuilder,
    function: &str,
    arguments: Vec<(Option<String>, Value)>,
    location: &ErrorLocation,
) -> Result<()> {
    match function {
        "prefix_rule" => {
            let [
                pattern,
                decision,
                matches,
                not_matches,
                justification,
                canonical_matching,
            ] = <[Option<Value>; 6]>::try_from(bind_arguments(
                function,
                &[
                    ("pattern", true),
                    ("decision", false),
                    ("match", false),
                    ("not_match", false),
                    ("justification", false),
                    ("canonical_matching", false),
                ],
                arguments,
            )?)
            .unwrap_or_else(|_| unreachable!("six parameters"));
            let pattern = expect_list(
                function,
                "pattern",
                pattern.as_ref().unwrap_or(&Value::None),
            )?;
            let decision = decision
                .as_ref()
                .map(|value| expect_str(function, "decision", value))
                .transpose()?;
            let justification = justification
                .as_ref()
                .map(|value| expect_str(function, "justification", value))
                .transpose()?;
            let canonical_matching = match canonical_matching {
                Some(Value::Bool(value)) => value,
                Some(other) => {
                    return Err(Error::InvalidRule(format!(
                        "Type of parameter `canonical_matching` doesn't match, expected `bool`, actual `{}` (in call to {function})",
                        other.type_name()
                    )));
                }
                None => false,
            };
            let matches = matches
                .as_ref()
                .map(|value| expect_list(function, "match", value).and_then(parse_examples))
                .transpose()?
                .unwrap_or_default();
            let not_matches = not_matches
                .as_ref()
                .map(|value| expect_list(function, "not_match", value).and_then(parse_examples))
                .transpose()?
                .unwrap_or_default();
            prefix_rule(
                builder,
                pattern,
                decision,
                matches,
                not_matches,
                justification,
                canonical_matching,
                location,
            )
        }
        "network_rule" => {
            let [host, protocol, decision, justification] =
                <[Option<Value>; 4]>::try_from(bind_arguments(
                    function,
                    &[
                        ("host", true),
                        ("protocol", true),
                        ("decision", true),
                        ("justification", false),
                    ],
                    arguments,
                )?)
                .unwrap_or_else(|_| unreachable!("four parameters"));
            let required = |name: &str, value: &Option<Value>| {
                expect_str(function, name, value.as_ref().unwrap_or(&Value::None))
                    .map(str::to_string)
            };
            let host = required("host", &host)?;
            let protocol = required("protocol", &protocol)?;
            let decision = required("decision", &decision)?;
            let justification = justification
                .as_ref()
                .map(|value| expect_str(function, "justification", value))
                .transpose()?;
            network_rule(builder, &host, &protocol, &decision, justification)
        }
        "host_executable" => {
            let [name, paths] = <[Option<Value>; 2]>::try_from(bind_arguments(
                function,
                &[("name", true), ("paths", true)],
                arguments,
            )?)
            .unwrap_or_else(|_| unreachable!("two parameters"));
            let name = expect_str(function, "name", name.as_ref().unwrap_or(&Value::None))?;
            let paths = expect_list(function, "paths", paths.as_ref().unwrap_or(&Value::None))?;
            host_executable(builder, name, paths)
        }
        other => Err(Error::InvalidRule(format!(
            "Variable `{other}` not found; rules files can call prefix_rule, network_rule and host_executable"
        ))),
    }
}

fn parse_pattern(pattern: &[Value]) -> Result<Vec<PatternToken>> {
    let tokens: Vec<PatternToken> = pattern
        .iter()
        .map(parse_pattern_token)
        .collect::<Result<_>>()?;
    if tokens.is_empty() {
        Err(Error::InvalidPattern("pattern cannot be empty".to_string()))
    } else {
        Ok(tokens)
    }
}

fn parse_pattern_token(value: &Value) -> Result<PatternToken> {
    if let Some(s) = value.as_str() {
        Ok(PatternToken::Single(s.to_string()))
    } else if let Value::List(list) = value {
        let tokens: Vec<String> = list
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| {
                        Error::InvalidPattern(format!(
                            "pattern alternative must be a string (got {})",
                            value.type_name()
                        ))
                    })
                    .map(str::to_string)
            })
            .collect::<Result<_>>()?;

        match tokens.as_slice() {
            [] => Err(Error::InvalidPattern(
                "pattern alternatives cannot be empty".to_string(),
            )),
            [single] => Ok(PatternToken::Single(single.clone())),
            _ => Ok(PatternToken::Alts(tokens)),
        }
    } else {
        Err(Error::InvalidPattern(format!(
            "pattern element must be a string or list of strings (got {})",
            value.type_name()
        )))
    }
}

fn parse_examples(examples: &[Value]) -> Result<Vec<Vec<String>>> {
    examples.iter().map(parse_example).collect()
}

fn parse_literal_absolute_path(raw: &str) -> Result<AbsolutePathBuf> {
    if !Path::new(raw).is_absolute() {
        return Err(Error::InvalidRule(format!(
            "host_executable paths must be absolute (got {raw})"
        )));
    }

    AbsolutePathBuf::try_from(raw.to_string())
        .map_err(|error| Error::InvalidRule(format!("invalid absolute path `{raw}`: {error}")))
}

fn validate_host_executable_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidRule(
            "host_executable name cannot be empty".to_string(),
        ));
    }

    let path = Path::new(name);
    if path.components().count() != 1
        || path.file_name().and_then(|value| value.to_str()) != Some(name)
    {
        return Err(Error::InvalidRule(format!(
            "host_executable name must be a bare executable name (got {name})"
        )));
    }

    Ok(())
}

fn parse_network_rule_decision(raw: &str) -> Result<Decision> {
    match raw {
        "deny" => Ok(Decision::Forbidden),
        other => Decision::parse(other),
    }
}

fn attach_validation_location(error: Error, location: Option<ErrorLocation>) -> Error {
    match location {
        Some(location) => error.with_location(location),
        None => error,
    }
}

fn parse_example(value: &Value) -> Result<Vec<String>> {
    if let Some(raw) = value.as_str() {
        parse_string_example(raw)
    } else if let Value::List(list) = value {
        parse_list_example(list)
    } else {
        Err(Error::InvalidExample(format!(
            "example must be a string or list of strings (got {})",
            value.type_name()
        )))
    }
}

fn parse_string_example(raw: &str) -> Result<Vec<String>> {
    let tokens = shlex::split(raw).ok_or_else(|| {
        Error::InvalidExample("example string has invalid shell syntax".to_string())
    })?;

    if tokens.is_empty() {
        Err(Error::InvalidExample(
            "example cannot be an empty string".to_string(),
        ))
    } else {
        Ok(tokens)
    }
}

fn parse_list_example(list: &[Value]) -> Result<Vec<String>> {
    let tokens: Vec<String> = list
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| {
                    Error::InvalidExample(format!(
                        "example tokens must be strings (got {})",
                        value.type_name()
                    ))
                })
                .map(str::to_string)
        })
        .collect::<Result<_>>()?;

    if tokens.is_empty() {
        Err(Error::InvalidExample(
            "example cannot be an empty list".to_string(),
        ))
    } else {
        Ok(tokens)
    }
}

#[expect(clippy::too_many_arguments)]
fn prefix_rule(
    builder: &mut PolicyBuilder,
    pattern: &[Value],
    decision: Option<&str>,
    matches: Vec<Vec<String>>,
    not_matches: Vec<Vec<String>>,
    justification: Option<&str>,
    canonical_matching: bool,
    location: &ErrorLocation,
) -> Result<()> {
    let decision = match decision {
        Some(raw) => Decision::parse(raw)?,
        None => Decision::Allow,
    };

    let justification = match justification {
        Some(raw) if raw.trim().is_empty() => {
            return Err(Error::InvalidRule(
                "justification cannot be empty".to_string(),
            ));
        }
        Some(raw) => Some(raw.to_string()),
        None => None,
    };

    let pattern_tokens = parse_pattern(pattern)?;

    let (first_token, remaining_tokens) = pattern_tokens
        .split_first()
        .ok_or_else(|| Error::InvalidPattern("pattern cannot be empty".to_string()))?;

    let rest: Arc<[PatternToken]> = remaining_tokens.to_vec().into();

    let rules: Vec<RuleRef> = first_token
        .alternatives()
        .iter()
        .map(|head| {
            Arc::new(PrefixRule {
                pattern: PrefixPattern {
                    canonical_matching,
                    first: Arc::from(head.as_str()),
                    rest: rest.clone(),
                },
                decision,
                justification: justification.clone(),
            }) as RuleRef
        })
        .collect();

    builder.add_pending_example_validation(
        rules.clone(),
        matches,
        not_matches,
        Some(location.clone()),
    );
    rules.into_iter().for_each(|rule| builder.add_rule(rule));
    Ok(())
}

fn network_rule(
    builder: &mut PolicyBuilder,
    host: &str,
    protocol: &str,
    decision: &str,
    justification: Option<&str>,
) -> Result<()> {
    let protocol = NetworkRuleProtocol::parse(protocol)?;
    let decision = parse_network_rule_decision(decision)?;
    let justification = match justification {
        Some(raw) if raw.trim().is_empty() => {
            return Err(Error::InvalidRule(
                "justification cannot be empty".to_string(),
            ));
        }
        Some(raw) => Some(raw.to_string()),
        None => None,
    };

    builder.add_network_rule(NetworkRule {
        host: crate::rule::normalize_network_rule_host(host)?,
        protocol,
        decision,
        justification,
    });
    Ok(())
}

fn host_executable(builder: &mut PolicyBuilder, name: &str, paths: &[Value]) -> Result<()> {
    validate_host_executable_name(name)?;

    let mut parsed_paths = Vec::new();
    for value in paths {
        let raw = value.as_str().ok_or_else(|| {
            Error::InvalidRule(format!(
                "host_executable paths must be strings (got {})",
                value.type_name()
            ))
        })?;
        let path = parse_literal_absolute_path(raw)?;
        let Some(path_name) = executable_path_lookup_key(path.as_path()) else {
            return Err(Error::InvalidRule(format!(
                "host_executable path `{raw}` must have basename `{name}`"
            )));
        };
        if path_name != executable_lookup_key(name) {
            return Err(Error::InvalidRule(format!(
                "host_executable path `{raw}` must have basename `{name}`"
            )));
        }
        if !parsed_paths.iter().any(|existing| existing == &path) {
            parsed_paths.push(path);
        }
    }

    builder.add_host_executable(executable_lookup_key(name), parsed_paths);
    Ok(())
}
