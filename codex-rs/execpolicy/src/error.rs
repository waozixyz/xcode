use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorLocation {
    pub path: String,
    pub range: TextRange,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid decision: {0}")]
    InvalidDecision(String),
    #[error("invalid pattern element: {0}")]
    InvalidPattern(String),
    #[error("invalid example: {0}")]
    InvalidExample(String),
    #[error("invalid rule: {0}")]
    InvalidRule(String),
    #[error(
        "expected every example to match at least one rule. rules: {rules:?}; unmatched examples: \
         {examples:?}"
    )]
    ExampleDidNotMatch {
        rules: Vec<String>,
        examples: Vec<String>,
        location: Option<ErrorLocation>,
    },
    #[error("expected example to not match rule `{rule}`: {example}")]
    ExampleDidMatch {
        rule: String,
        example: String,
        location: Option<ErrorLocation>,
    },
    /// A rules file that does not parse or evaluate, at the statement or
    /// position where it failed. The text keeps the `path:line:column:
    /// starlark error:` shape that callers parse for the location.
    #[error(
        "{}:{}:{}: starlark error: error: {message}",
        location.path,
        location.range.start.line,
        location.range.start.column
    )]
    Syntax {
        message: String,
        location: ErrorLocation,
    },
}

impl Error {
    pub fn with_location(self, location: ErrorLocation) -> Self {
        match self {
            Error::ExampleDidNotMatch {
                rules,
                examples,
                location: None,
            } => Error::ExampleDidNotMatch {
                rules,
                examples,
                location: Some(location),
            },
            Error::ExampleDidMatch {
                rule,
                example,
                location: None,
            } => Error::ExampleDidMatch {
                rule,
                example,
                location: Some(location),
            },
            other => other,
        }
    }

    pub fn location(&self) -> Option<ErrorLocation> {
        match self {
            Error::ExampleDidNotMatch { location, .. }
            | Error::ExampleDidMatch { location, .. } => location.clone(),
            Error::Syntax { location, .. } => Some(location.clone()),
            _ => None,
        }
    }
}
