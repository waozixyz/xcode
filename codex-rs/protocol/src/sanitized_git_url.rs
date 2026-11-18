use std::fmt;
use std::ops::Deref;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// A git remote URL with authentication credentials removed.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
#[ts(type = "string")]
pub struct SanitizedGitUrl(String);

impl SanitizedGitUrl {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

unsafe extern "C" {
    fn xcode_git_sanitize_url(text: *const u8, length: i64, out: *mut u8, capacity: i64) -> i64;
}

impl TryFrom<&str> for SanitizedGitUrl {
    type Error = &'static str;

    /// Removes user names (except SSH's conventional `git`) and passwords,
    /// keeping remote-helper prefixes and the original spelling otherwise.
    /// Locations Git would reject, and helper command lines (which can hide
    /// secrets), are refused. The rules live in the Ziran `git` package.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut out = vec![0u8; value.len() + 16];
        // SAFETY: `value` covers its length and `out` its capacity, which the
        // package requires to be at least the input length plus 16.
        let length = unsafe {
            xcode_git_sanitize_url(
                value.as_ptr(),
                value.len() as i64,
                out.as_mut_ptr(),
                out.len() as i64,
            )
        };
        if length < 0 {
            return Err("invalid git remote URL");
        }
        out.truncate(length as usize);
        // The result is cut from the input at ASCII delimiters.
        String::from_utf8(out)
            .map(Self)
            .map_err(|_| "invalid git remote URL")
    }
}

impl TryFrom<String> for SanitizedGitUrl {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<SanitizedGitUrl> for String {
    fn from(value: SanitizedGitUrl) -> Self {
        value.0
    }
}

impl AsRef<str> for SanitizedGitUrl {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Deref for SanitizedGitUrl {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for SanitizedGitUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn deserialize_optional_sanitized_git_url<'de, D>(
    deserializer: D,
) -> Result<Option<SanitizedGitUrl>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Malformed legacy origins must not prevent the rest of a rollout from loading.
    Ok(Option::<String>::deserialize(deserializer)?
        .and_then(|value| SanitizedGitUrl::try_from(value).ok()))
}

#[cfg(test)]
#[path = "sanitized_git_url_tests.rs"]
mod tests;
