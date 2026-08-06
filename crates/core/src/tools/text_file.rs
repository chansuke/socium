use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;

pub(crate) fn apply_view_range(
    content: &str,
    view_range: Option<&Value>,
) -> Result<String, String> {
    let Some(view_range) = view_range else {
        return Ok(content.to_string());
    };
    let range = view_range
        .as_array()
        .ok_or("text_editor view_range must be an array: [start_line, end_line]")?;
    if range.len() != 2 {
        return Err("text_editor view_range must contain exactly two integers".to_string());
    }
    let start = range[0]
        .as_i64()
        .ok_or("text_editor view_range start must be an integer")?;
    let end = range[1]
        .as_i64()
        .ok_or("text_editor view_range end must be an integer")?;
    if start < 1 {
        return Err("text_editor view_range start must be >= 1".to_string());
    }
    if end < -1 || end == 0 {
        return Err("text_editor view_range end must be -1 or a positive integer".to_string());
    }
    if end != -1 && end < start {
        return Err("text_editor view_range end must be -1 or >= start".to_string());
    }

    let line_count = count_lines(content) as i64;
    if line_count == 0 {
        if start == 1 {
            return Ok(String::new());
        }
        return Err("text_editor view_range start is out of range for an empty file".to_string());
    }
    if start > line_count {
        return Err(format!(
            "text_editor view_range start {} is out of range (file has {} lines)",
            start, line_count
        ));
    }
    let last = if end == -1 {
        line_count
    } else {
        std::cmp::min(end, line_count)
    };
    let start_line = start as usize;
    let end_line = last as usize;

    let start_offset = line_start_offset(content, start_line).ok_or_else(|| {
        format!(
            "text_editor view_range start {} is out of range (file has {} lines)",
            start, line_count
        )
    })?;
    let end_offset = if end_line as i64 == line_count {
        content.len()
    } else {
        line_start_offset(content, end_line + 1).ok_or_else(|| {
            format!(
                "text_editor view_range end {} is out of range (file has {} lines)",
                end_line, line_count
            )
        })?
    };

    Ok(content[start_offset..end_offset].to_string())
}

pub(crate) fn truncate_by_characters(content: &str, max_characters: usize) -> (String, bool) {
    if max_characters == 0 {
        return (String::new(), !content.is_empty());
    }

    let mut end = 0usize;
    for (count, (index, ch)) in content.char_indices().enumerate() {
        if count == max_characters {
            return (content[..end].to_string(), true);
        }
        end = index + ch.len_utf8();
    }
    (content.to_string(), false)
}

pub(crate) fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

pub(crate) fn byte_index_after_line(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    let mut seen = 0usize;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            seen += 1;
            if seen == line {
                return Some(index + 1);
            }
        }
    }
    if line == count_lines(content) {
        return Some(content.len());
    }
    None
}

pub(crate) fn line_start_offset(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }

    let mut current_line = 1usize;
    for (index, byte) in content.as_bytes().iter().enumerate() {
        if *byte == b'\n' {
            current_line += 1;
            if current_line == line {
                return Some(index + 1);
            }
        }
    }
    None
}

pub(crate) fn read_utf8_file_with_limit(
    path: &Path,
    display_path: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("Failed to open file '{}': {e}", display_path))?;
    let mut bytes = Vec::new();
    let mut limited = Read::by_ref(&mut file).take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read file '{}': {e}", display_path))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "File too large: '{}' exceeds {} bytes",
            display_path, max_bytes
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("File '{}' is not valid UTF-8: {e}", display_path))
}
