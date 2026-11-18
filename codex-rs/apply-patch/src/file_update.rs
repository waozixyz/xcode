use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::ReadFileOptions;
use codex_utils_path_uri::PathUri;

use crate::ApplyPatchError;
use crate::IoError;
use crate::UpdateFileChunk;
use crate::seek_sequence;
use crate::text_file::Replacement;
use crate::text_file::SourceFile;

#[cfg(test)]
#[path = "file_update_tests.rs"]
mod tests;

pub(crate) struct AppliedPatch {
    pub(crate) original_contents: String,
    pub(crate) new_contents: String,
}

/// Return *only* the new file contents (joined into a single `String`) after
/// applying the chunks to the file at `path`.
pub(crate) async fn derive_new_contents_from_chunks(
    path: &PathUri,
    chunks: &[UpdateFileChunk],
    fs: &dyn ExecutorFileSystem,
    follow_symlinks: bool,
    sandbox: Option<&FileSystemSandboxContext>,
) -> std::result::Result<AppliedPatch, ApplyPatchError> {
    let original_contents = fs
        .read_file_text(path, ReadFileOptions { follow_symlinks }, sandbox)
        .await
        .map_err(|err| {
            ApplyPatchError::IoError(IoError {
                context: format!(
                    "Failed to read file to update {}",
                    path.inferred_native_path_string()
                ),
                source: err,
            })
        })?;

    let path_text = path.inferred_native_path_string();
    let new_contents = apply_chunks(&original_contents, &path_text, chunks)?;
    Ok(AppliedPatch {
        original_contents,
        new_contents,
    })
}

/// Compute a list of replacements needed to transform `original_lines` into the
/// new lines, given the patch `chunks`. Each replacement is returned as
/// `(start_index, old_len, new_lines)`.
/// The patched contents of `original` (displayed as `path` in errors).
///
/// Runs in Ziran (`native/apply_patch.zi`): lines are offsets into `original`
/// and the result is written once, instead of copying every line into a
/// `String` several times.
pub(crate) fn apply_chunks(
    original: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<String, ApplyPatchError> {
    let mut strings: Vec<&str> = Vec::new();
    let mut records: Vec<i64> = Vec::with_capacity(chunks.len() * 8);
    let mut pairs: Vec<i64> = Vec::new();
    let mut replacement_capacity = 0usize;
    let mut inserted = 0usize;
    for chunk in chunks {
        let context = match &chunk.change_context {
            Some(line) => {
                strings.push(line);
                strings.len() as i64 - 1
            }
            None => -1,
        };
        let old_first = strings.len();
        strings.extend(chunk.old_lines.iter().map(String::as_str));
        let new_first = strings.len();
        strings.extend(chunk.new_lines.iter().map(String::as_str));
        inserted += chunk
            .new_lines
            .iter()
            .map(|line| line.len() + 2)
            .sum::<usize>();
        let pair_first = pairs.len() / 2;
        for &(old, new) in &chunk.context_line_indices {
            pairs.extend([old as i64, new as i64]);
        }
        replacement_capacity += chunk.context_line_indices.len() + 1;
        records.extend([
            context,
            old_first as i64,
            chunk.old_lines.len() as i64,
            new_first as i64,
            chunk.new_lines.len() as i64,
            pair_first as i64,
            chunk.context_line_indices.len() as i64,
            i64::from(chunk.is_end_of_file),
        ]);
    }
    let too_large = |length: usize| length > i32::MAX as usize;
    if too_large(original.len()) || strings.iter().any(|line| too_large(line.len())) {
        return rust_apply_chunks(original, path, chunks);
    }
    let line_capacity = original
        .bytes()
        .filter(|byte| matches!(byte, b'\n' | b'\r'))
        .count()
        + 1;
    let mut line_data = vec![std::ptr::null::<u8>(); line_capacity];
    let mut line_lengths = vec![0i64; line_capacity];
    let mut line_endings = vec![0i32; line_capacity];
    let string_data = strings.iter().map(|line| line.as_ptr()).collect::<Vec<_>>();
    let string_lengths = strings
        .iter()
        .map(|line| line.len() as i64)
        .collect::<Vec<_>>();
    let mut replacements = vec![0i64; replacement_capacity * 4];
    // Kept lines never grow; each inserted line and a final line that had no
    // ending gain at most two ending bytes.
    let output_capacity = original.len() + inserted + 2;
    let mut output = Vec::<u8>::with_capacity(output_capacity);
    let mut failed_chunk = -1i64;
    // SAFETY: every table outlives the call and has the length passed for it;
    // Ziran writes at most `output_capacity` bytes into `output`.
    let written = unsafe {
        xcode_apply_chunks(
            original.as_ptr(),
            original.len() as i64,
            line_data.as_mut_ptr(),
            line_lengths.as_mut_ptr(),
            line_endings.as_mut_ptr(),
            line_capacity as i64,
            string_data.as_ptr(),
            string_lengths.as_ptr(),
            strings.len() as i64,
            records.as_ptr(),
            chunks.len() as i64,
            pairs.as_ptr(),
            replacements.as_mut_ptr(),
            replacement_capacity as i64,
            output.as_mut_ptr(),
            output_capacity as i64,
            &mut failed_chunk,
        )
    };
    let failed = || &chunks[usize::try_from(failed_chunk).unwrap_or_default()];
    match written {
        written if written >= 0 => {
            // SAFETY: Ziran initialised exactly `written` bytes.
            unsafe { output.set_len(written as usize) };
            // Whole lines of valid UTF-8 joined by ASCII endings stay valid.
            String::from_utf8(output).or_else(|_| rust_apply_chunks(original, path, chunks))
        }
        -1 => Err(ApplyPatchError::ComputeReplacements(format!(
            "Failed to find context '{}' in {path}",
            failed().change_context.as_deref().unwrap_or_default()
        ))),
        -2 => Err(ApplyPatchError::ComputeReplacements(format!(
            "Failed to find expected lines in {}:\n{}",
            path,
            failed().old_lines.join("\n"),
        ))),
        _ => rust_apply_chunks(original, path, chunks),
    }
}

unsafe extern "C" {
    fn xcode_apply_chunks(
        contents: *const u8,
        contents_length: i64,
        line_data: *mut *const u8,
        line_lengths: *mut i64,
        line_endings: *mut i32,
        line_capacity: i64,
        strings: *const *const u8,
        string_lengths: *const i64,
        string_count: i64,
        chunks: *const i64,
        chunk_count: i64,
        pairs: *const i64,
        replacements: *mut i64,
        replacement_capacity: i64,
        output: *mut u8,
        output_capacity: i64,
        failed_chunk: *mut i64,
    ) -> i64;
}

/// The original Rust pipeline: the fallback when Ziran refuses its input (text
/// above 2 GiB) and the oracle its parity tests compare against.
pub(crate) fn rust_apply_chunks(
    original: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<String, ApplyPatchError> {
    let mut source_file = SourceFile::parse(original);
    let original_lines = source_file.line_texts();
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    source_file.apply_replacements(&replacements);
    Ok(source_file.into_contents())
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<Vec<Replacement>, ApplyPatchError> {
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // If a chunk has a `change_context`, we use seek_sequence to find it, then
        // adjust our `line_index` to continue from there.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                /*eof*/ false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{ctx_line}' in {path}"
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            replacements.push((original_lines.len(), 0, chunk.new_lines.clone()));
            continue;
        }

        // Otherwise, try to match the existing lines in the file with the old lines
        // from the chunk. If found, schedule that region for replacement.
        // Attempt to locate the `old_lines` verbatim within the file.  In many
        // real‑world diffs the last element of `old_lines` is an *empty* string
        // representing the terminating newline of the region being replaced.
        // This sentinel is not present in `original_lines` because `SourceFile`
        // stores the terminator on the preceding line rather than as an extra
        // trailing element. If a direct search fails and the pattern ends with
        // an empty string, retry without that final element so modifications
        // touching the end‑of‑file can be located reliably.

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found =
            seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            // Retry without the trailing empty line which represents the final
            // newline in the file.
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }

            found = seek_sequence::seek_sequence(
                original_lines,
                pattern,
                line_index,
                chunk.is_end_of_file,
            );
        }

        if let Some(start_idx) = found {
            // Context lines occur in both sides of a patch chunk. Keep those
            // original lines in place so their exact contents and terminators
            // survive, especially when the file has mixed line endings.
            let mut old_start = 0;
            let mut new_start = 0;
            for &(old_context, new_context) in &chunk.context_line_indices {
                // A trailing empty context line can be removed from `pattern`
                // and `new_slice` above when it represents the final newline.
                if old_context >= pattern.len() || new_context >= new_slice.len() {
                    break;
                }
                if old_start != old_context || new_start != new_context {
                    replacements.push((
                        start_idx + old_start,
                        old_context - old_start,
                        new_slice[new_start..new_context].to_vec(),
                    ));
                }
                old_start = old_context + 1;
                new_start = new_context + 1;
            }
            if old_start != pattern.len() || new_start != new_slice.len() {
                replacements.push((
                    start_idx + old_start,
                    pattern.len() - old_start,
                    new_slice[new_start..].to_vec(),
                ));
            }
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path,
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);

    Ok(replacements)
}

/// Intended result of a file update for apply_patch.
#[derive(Debug, Eq, PartialEq)]
pub struct ApplyPatchFileUpdate {
    pub(crate) unified_diff: String,
    pub(crate) original_content: String,
    pub(crate) content: String,
}

pub async fn unified_diff_from_chunks(
    path: &PathUri,
    chunks: &[UpdateFileChunk],
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    unified_diff_from_chunks_with_context(path, chunks, /*context*/ 1, fs, sandbox).await
}

pub async fn unified_diff_from_chunks_with_context(
    path: &PathUri,
    chunks: &[UpdateFileChunk],
    context: usize,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    let AppliedPatch {
        original_contents,
        new_contents,
    } = derive_new_contents_from_chunks(path, chunks, fs, /*follow_symlinks*/ true, sandbox)
        .await?;
    let unified_diff = codex_utils_text_diff::unified(&original_contents, &new_contents)
        .context_radius(context)
        .render();
    Ok(ApplyPatchFileUpdate {
        unified_diff,
        original_content: original_contents,
        content: new_contents,
    })
}
