use super::ModelError;

pub(super) fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            other if other.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

pub(super) fn extract_json_string_field_after(
    text: &str,
    marker: &str,
    field: &str,
) -> Option<String> {
    let start = if marker.is_empty() {
        0
    } else {
        text.find(marker)? + marker.len()
    };
    extract_json_string_field(&text[start..], field)
}

pub(super) fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let mut offset = 0;

    while let Some(position) = text[offset..].find(&pattern) {
        let mut index = offset + position + pattern.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        if text.as_bytes().get(index).copied() != Some(b'"') {
            offset = index.saturating_add(1);
            continue;
        }

        return parse_json_string_at(text, index).ok();
    }

    None
}

pub(super) fn extract_json_number_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let mut offset = 0;

    while let Some(position) = text[offset..].find(&pattern) {
        let mut index = offset + position + pattern.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        let start = index;
        while matches!(
            text.as_bytes().get(index).copied(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            index += 1;
        }
        if start != index {
            return Some(text[start..index].to_string());
        }
        offset = index.saturating_add(1);
    }

    None
}

pub(super) fn extract_json_bool_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let mut offset = 0;

    while let Some(position) = text[offset..].find(&pattern) {
        let mut index = offset + position + pattern.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        if text[index..].starts_with("true") {
            return Some("true".to_string());
        }
        if text[index..].starts_with("false") {
            return Some("false".to_string());
        }
        offset = index.saturating_add(1);
    }

    None
}

pub(super) fn extract_json_array_after(text: &str, marker: &str) -> Option<String> {
    let mut offset = 0;
    while let Some(position) = text[offset..].find(marker) {
        let mut index = offset + position + marker.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        if text.as_bytes().get(index).copied() != Some(b'[') {
            offset = index.saturating_add(1);
            continue;
        }

        return extract_balanced_array(text, index);
    }

    None
}

fn extract_balanced_array(text: &str, open_index: usize) -> Option<String> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text[open_index..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = open_index + offset + character.len_utf8();
                    return Some(text[open_index..end].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

pub(super) fn split_top_level_objects(array: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;

    for (index, character) in array.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(start) = start.take() {
                        objects.push(array[start..index + character.len_utf8()].to_string());
                    }
                }
            }
            _ => {}
        }
    }

    objects
}

pub(super) fn parse_number_array(array: &str) -> Result<Vec<f32>, ModelError> {
    let trimmed = array.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| ModelError::new("number array was malformed"))?;
    let mut values = Vec::new();
    for part in inner.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        values.push(
            part.parse::<f32>()
                .map_err(|error| ModelError::new(format!("invalid embedding number: {error}")))?,
        );
    }

    Ok(values)
}

fn skip_whitespace(text: &str, mut index: usize) -> usize {
    while matches!(
        text.as_bytes().get(index).copied(),
        Some(b' ' | b'\n' | b'\r' | b'\t')
    ) {
        index += 1;
    }
    index
}

fn parse_json_string_at(text: &str, quote_index: usize) -> Result<String, ModelError> {
    if text.as_bytes().get(quote_index).copied() != Some(b'"') {
        return Err(ModelError::new("json string did not start with a quote"));
    }

    let mut output = String::new();
    let mut index = quote_index + 1;

    while index < text.len() {
        let character = text[index..]
            .chars()
            .next()
            .ok_or_else(|| ModelError::new("invalid json string"))?;
        if character == '"' {
            return Ok(output);
        }
        if character != '\\' {
            output.push(character);
            index += character.len_utf8();
            continue;
        }

        index += 1;
        let escape = text[index..]
            .chars()
            .next()
            .ok_or_else(|| ModelError::new("unterminated json escape"))?;
        match escape {
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '/' => output.push('/'),
            'b' => output.push('\u{08}'),
            'f' => output.push('\u{0c}'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            'u' => {
                let start = index + 1;
                let end = start + 4;
                let hex = text
                    .get(start..end)
                    .ok_or_else(|| ModelError::new("invalid unicode escape"))?;
                let value = u32::from_str_radix(hex, 16)
                    .map_err(|error| ModelError::new(format!("invalid unicode escape: {error}")))?;
                let character = char::from_u32(value)
                    .ok_or_else(|| ModelError::new("unicode escape is not a valid scalar"))?;
                output.push(character);
                index = end;
                continue;
            }
            other => {
                return Err(ModelError::new(format!("unsupported json escape: {other}")));
            }
        }
        index += escape.len_utf8();
    }

    Err(ModelError::new("unterminated json string"))
}
