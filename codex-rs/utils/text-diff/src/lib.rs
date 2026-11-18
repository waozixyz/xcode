//! Line diffs, unified diff text and unified-diff parsing, implemented by the
//! Ziran `diff` package (`native/diff.zi`).
//!
//! [`unified`] output matches `similar::TextDiff::from_lines(old, new).unified_diff()`
//! byte for byte; [`Patch::from_str`] accepts and rejects exactly what
//! `diffy::Patch::from_str` does.

use std::fmt;

#[cfg(test)]
mod tests;

const FIELDS: usize = 5;
const HUNK_FIELDS: usize = 8;
const LINE_FIELDS: usize = 3;
const PARSE_ERROR: i64 = -3;

unsafe extern "C" {
    fn xcode_diff_line_count(text: *const u8, length: i64) -> i64;
    fn xcode_diff_op_capacity(old_count: i64, new_count: i64) -> i64;
    fn xcode_diff_lines(
        old_text: *const u8,
        old_length: i64,
        new_text: *const u8,
        new_length: i64,
        budget: i64,
        old_starts: *mut i64,
        new_starts: *mut i64,
        old_hashes: *mut u64,
        new_hashes: *mut u64,
        forward: *mut i64,
        backward: *mut i64,
        raw_ops: *mut i64,
        ops: *mut i64,
        op_capacity: i64,
    ) -> i64;
    fn xcode_diff_unified(
        old_text: *const u8,
        new_text: *const u8,
        old_starts: *const i64,
        new_starts: *const i64,
        ops: *const i64,
        op_count: i64,
        context: i64,
        has_header: i32,
        header_old: *const u8,
        header_old_length: i64,
        header_new: *const u8,
        header_new_length: i64,
        output: *mut u8,
        output_capacity: i64,
    ) -> i64;
    fn xcode_diff_parse(
        text: *const u8,
        length: i64,
        header: *mut i64,
        hunks: *mut i64,
        hunk_capacity: i64,
        lines: *mut i64,
        line_capacity: i64,
    ) -> i64;
}

/// A unified diff of two texts, line by line; build it with [`unified`].
#[derive(Clone, Copy, Debug)]
pub struct Unified<'a> {
    old: &'a str,
    new: &'a str,
    context: usize,
    header: Option<(&'a str, &'a str)>,
    budget: u64,
}

/// The unified diff of `old` and `new` with three lines of context and no file header.
pub fn unified<'a>(old: &'a str, new: &'a str) -> Unified<'a> {
    Unified {
        old,
        new,
        context: 3,
        header: None,
        budget: 0,
    }
}

impl<'a> Unified<'a> {
    /// Lines of unchanged context around each change.
    pub fn context_radius(mut self, context: usize) -> Self {
        self.context = context;
        self
    }

    /// Writes `--- old` and `+++ new` before the first hunk.
    pub fn header(mut self, old: &'a str, new: &'a str) -> Self {
        self.header = Some((old, new));
        self
    }

    /// Bounds the search work (0, the default, is unlimited). Once it is
    /// spent, the remaining differing ranges are reported as a removal and an
    /// insertion, like `similar`'s deadline but deterministic.
    pub fn budget(mut self, budget: u64) -> Self {
        self.budget = budget;
        self
    }

    pub fn render(&self) -> String {
        let old = self.old.as_bytes();
        let new = self.new.as_bytes();
        // SAFETY: each pointer covers the length passed with it, and every
        // workspace has the size `native/diff.zi` documents.
        unsafe {
            let old_count = xcode_diff_line_count(old.as_ptr(), old.len() as i64);
            let new_count = xcode_diff_line_count(new.as_ptr(), new.len() as i64);
            let capacity = xcode_diff_op_capacity(old_count, new_count);
            let snake = 2 * ((old_count + new_count + 1) / 2 + 1) + 2;
            let mut old_starts = vec![0i64; old_count as usize + 1];
            let mut new_starts = vec![0i64; new_count as usize + 1];
            let mut old_hashes = vec![0u64; old_count as usize + 1];
            let mut new_hashes = vec![0u64; new_count as usize + 1];
            let mut forward = vec![0i64; snake as usize];
            let mut backward = vec![0i64; snake as usize];
            let mut raw = vec![0i64; capacity as usize * FIELDS];
            let mut ops = vec![0i64; capacity as usize * FIELDS];
            let count = xcode_diff_lines(
                old.as_ptr(),
                old.len() as i64,
                new.as_ptr(),
                new.len() as i64,
                self.budget.min(i64::MAX as u64) as i64,
                old_starts.as_mut_ptr(),
                new_starts.as_mut_ptr(),
                old_hashes.as_mut_ptr(),
                new_hashes.as_mut_ptr(),
                forward.as_mut_ptr(),
                backward.as_mut_ptr(),
                raw.as_mut_ptr(),
                ops.as_mut_ptr(),
                capacity,
            );
            assert!(count >= 0, "diff workspace sized by the package");
            drop(raw);
            let (has_header, header_old, header_new) = match self.header {
                Some((old, new)) => (1, old.as_bytes(), new.as_bytes()),
                None => (0, &[][..], &[][..]),
            };
            let write = |output: *mut u8, capacity: i64| {
                xcode_diff_unified(
                    old.as_ptr(),
                    new.as_ptr(),
                    old_starts.as_ptr(),
                    new_starts.as_ptr(),
                    ops.as_ptr(),
                    count,
                    self.context.min(i64::MAX as usize / 4) as i64,
                    has_header,
                    header_old.as_ptr(),
                    header_old.len() as i64,
                    header_new.as_ptr(),
                    header_new.len() as i64,
                    output,
                    capacity,
                )
            };
            let length = write(std::ptr::null_mut(), 0);
            assert!(length >= 0, "unified diff of valid ops");
            let mut output = vec![0u8; length as usize];
            write(output.as_mut_ptr(), length);
            // Whole lines of UTF-8 text plus ASCII markup.
            String::from_utf8(output).unwrap_or_else(|error| {
                String::from_utf8_lossy(error.as_bytes()).into_owned()
            })
        }
    }
}

impl fmt::Display for Unified<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

/// A hunk's line range: 1-based `start` (0 for an empty range at the top) and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HunkRange {
    start: usize,
    len: usize,
}

impl HunkRange {
    pub fn start(&self) -> usize {
        self.start
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// One hunk line; the text keeps its newline unless the patch marked it missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    Context(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk<'a> {
    old_range: HunkRange,
    new_range: HunkRange,
    function_context: Option<&'a str>,
    lines: Vec<Line<'a>>,
}

impl<'a> Hunk<'a> {
    pub fn old_range(&self) -> HunkRange {
        self.old_range
    }

    pub fn new_range(&self) -> HunkRange {
        self.new_range
    }

    /// The text after the closing `@@ `, with its newline.
    pub fn function_context(&self) -> Option<&'a str> {
        self.function_context
    }

    pub fn lines(&self) -> &[Line<'a>] {
        &self.lines
    }
}

/// A parsed unified diff. Filenames are kept as written (quoted names keep
/// their quotes and escapes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch<'a> {
    original: Option<&'a str>,
    modified: Option<&'a str>,
    hunks: Vec<Hunk<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsePatchError;

impl fmt::Display for ParsePatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("error parsing patch")
    }
}

impl std::error::Error for ParsePatchError {}

impl<'a> Patch<'a> {
    /// Parses with the rules of `diffy::Patch::from_str`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &'a str) -> Result<Self, ParsePatchError> {
        let bytes = text.as_bytes();
        let line_capacity = bytes.iter().filter(|byte| **byte == b'\n').count() + 1;
        let mut header = [0i64; 6];
        let mut hunks = vec![0i64; line_capacity * HUNK_FIELDS];
        let mut lines = vec![0i64; line_capacity * LINE_FIELDS];
        // SAFETY: the buffers hold `line_capacity` records each, the most a
        // text with this many lines can produce.
        let count = unsafe {
            xcode_diff_parse(
                bytes.as_ptr(),
                bytes.len() as i64,
                header.as_mut_ptr(),
                hunks.as_mut_ptr(),
                line_capacity as i64,
                lines.as_mut_ptr(),
                line_capacity as i64,
            )
        };
        if count == PARSE_ERROR {
            return Err(ParsePatchError);
        }
        assert!(count >= 0, "parse buffers sized by line count");
        // Offsets fall on line and ASCII-marker boundaries of valid UTF-8.
        let slice = |start: i64, end: i64| &text[start as usize..end as usize];
        let name = |start: i64, end: i64| (start >= 0).then(|| slice(start, end));
        let records = hunks[..count as usize * HUNK_FIELDS]
            .chunks_exact(HUNK_FIELDS)
            .map(|hunk| {
                let first = hunk[4] as usize;
                let lines = lines[first * LINE_FIELDS..(first + hunk[5] as usize) * LINE_FIELDS]
                    .chunks_exact(LINE_FIELDS)
                    .map(|line| {
                        let text = slice(line[1], line[2]);
                        match line[0] {
                            1 => Line::Delete(text),
                            2 => Line::Insert(text),
                            _ => Line::Context(text),
                        }
                    })
                    .collect();
                Hunk {
                    old_range: HunkRange {
                        start: hunk[0] as usize,
                        len: hunk[1] as usize,
                    },
                    new_range: HunkRange {
                        start: hunk[2] as usize,
                        len: hunk[3] as usize,
                    },
                    function_context: name(hunk[6], hunk[7]),
                    lines,
                }
            })
            .collect();
        Ok(Self {
            original: name(header[0], header[1]),
            modified: name(header[3], header[4]),
            hunks: records,
        })
    }

    pub fn original(&self) -> Option<&'a str> {
        self.original
    }

    pub fn modified(&self) -> Option<&'a str> {
        self.modified
    }

    pub fn hunks(&self) -> &[Hunk<'a>] {
        &self.hunks
    }
}
