//! Bounded terminal-native Mermaid rendering, implemented as a portable Ziran module.

use std::fmt;

const MAX_SOURCE: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderError {
    Unsupported,
    Limit,
    TooWide,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "unsupported Mermaid syntax or label",
            Self::Limit => "diagram exceeds prototype limits",
            Self::TooWide => "diagram exceeds available display width",
        })
    }
}

impl std::error::Error for RenderError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Node,
    Edge,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub role: Role,
}

pub fn render(source: &str, max_width: usize) -> Result<String, RenderError> {
    let grid = native_render(source, max_width)?;
    let mut text = String::with_capacity(grid.points.count as usize * 2);
    for y in 0..grid.height as usize {
        if y != 0 {
            text.push('\n');
        }
        for point in grid.row(y) {
            if *point != 0 {
                text.push(char::from_u32(*point).ok_or(RenderError::Unsupported)?);
            }
        }
    }
    Ok(text)
}

pub fn render_spans(source: &str, max_width: usize) -> Result<Vec<Vec<Span>>, RenderError> {
    let grid = native_render(source, max_width)?;
    let mut lines = Vec::with_capacity(grid.height as usize);
    for y in 0..grid.height as usize {
        let mut line: Vec<Span> = Vec::new();
        let roles = &grid.roles()[y * grid.width as usize..];
        let row = grid.row(y);
        let mut start = 0;
        while start < row.len() {
            if row[start] == 0 {
                start += 1;
                continue;
            }
            let role = match roles[start] {
                0 => Role::Edge,
                1 => Role::Node,
                2 => Role::Text,
                _ => return Err(RenderError::Unsupported),
            };
            let mut end = start;
            let mut bytes = 0;
            while end < row.len() && (row[end] == 0 || roles[end] == roles[start]) {
                let point = row[end];
                if point != 0 {
                    bytes += if point < 0x80 {
                        1
                    } else if point < 0x800 {
                        2
                    } else if point < 0x10000 {
                        3
                    } else {
                        4
                    };
                }
                end += 1;
            }
            let mut text = String::with_capacity(bytes);
            for point in &row[start..end] {
                if *point != 0 {
                    text.push(char::from_u32(*point).ok_or(RenderError::Unsupported)?);
                }
            }
            line.push(Span { text, role });
            start = end;
        }
        lines.push(line);
    }
    Ok(lines)
}

fn native_render(source: &str, max_width: usize) -> Result<NativeGrid, RenderError> {
    if source.len() > MAX_SOURCE {
        return Err(RenderError::Limit);
    }
    let grid = unsafe {
        xcode_mermaid_grid(
            NativeSource {
                data: source.as_ptr(),
                length: source.len() as i64,
            },
            max_width.min(i64::MAX as usize) as i64,
        )
    };
    match grid.status {
        0 => {}
        2 => return Err(RenderError::Limit),
        3 => return Err(RenderError::TooWide),
        _ => return Err(RenderError::Unsupported),
    }
    let cells = i64::from(grid.width).checked_mul(i64::from(grid.height));
    if grid.width <= 0
        || grid.height <= 0
        || cells != Some(grid.points.count)
        || grid.points.count != grid.roles.count
        || grid.points.count > 64 * 1024
        || grid.points.data.is_null()
        || grid.roles.data.is_null()
    {
        return Err(RenderError::Unsupported);
    }
    Ok(grid)
}

#[repr(C)]
struct NativeSource {
    data: *const u8,
    length: i64,
}

#[repr(C)]
struct NativeVec<T> {
    data: *mut T,
    count: i64,
    capacity: i64,
}

#[repr(C)]
struct NativeGrid {
    points: NativeVec<u32>,
    roles: NativeVec<u8>,
    width: i32,
    height: i32,
    status: i32,
}

impl NativeGrid {
    fn points(&self) -> &[u32] {
        // Successful native results own initialized row-major cells until Drop.
        unsafe { std::slice::from_raw_parts(self.points.data, self.points.count as usize) }
    }

    fn roles(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.roles.data, self.roles.count as usize) }
    }

    fn row(&self, y: usize) -> &[u32] {
        let start = y * self.width as usize;
        let row = &self.points()[start..start + self.width as usize];
        let end = row
            .iter()
            .rposition(|point| {
                *point != 0 && char::from_u32(*point).is_none_or(|ch| !ch.is_whitespace())
            })
            .map_or(0, |x| x + 1);
        &row[..end]
    }
}

impl Drop for NativeGrid {
    fn drop(&mut self) {
        unsafe { xcode_mermaid_grid_free(self) };
    }
}

unsafe extern "C" {
    fn xcode_mermaid_grid(source: NativeSource, max_width: i64) -> NativeGrid;
    fn xcode_mermaid_grid_free(grid: *mut NativeGrid);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod parity_tests;
