use agent_core::Event;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(crate) fn first_metadata_value<'a>(event: &'a Event, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| event.metadata.get(*key).map(String::as_str))
}

pub(crate) fn sanitize_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(character);
    }
    output
}

pub(crate) fn normalize_memory_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

pub(crate) fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn requirement_evidence_sha256(
    schema: &str,
    project_id: &str,
    session_id: &str,
    event_id: &str,
    source_sha256: &str,
    quote_start_byte: u64,
    quote_end_byte: u64,
    quote: &str,
) -> String {
    let mut hasher = Sha256::new();
    for field in [
        schema.as_bytes(),
        project_id.as_bytes(),
        session_id.as_bytes(),
        event_id.as_bytes(),
        source_sha256.as_bytes(),
        &quote_start_byte.to_be_bytes(),
        &quote_end_byte.to_be_bytes(),
        quote.as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    format!("{:x}", hasher.finalize())
}

pub(crate) fn memory_terms(value: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for raw in value.split(|character: char| {
        character.is_whitespace() || (!character.is_alphanumeric() && character != '_')
    }) {
        let token = raw.trim().to_lowercase();
        if token.is_empty() || is_memory_stopword(&token) {
            continue;
        }
        terms.insert(token.clone());
        let chars = token.chars().collect::<Vec<_>>();
        if chars.iter().any(|character| !character.is_ascii()) {
            for pair in chars.windows(2) {
                terms.insert(pair.iter().collect());
            }
        } else if token.len() > 3 && token.ends_with('s') && !token.ends_with("ss") {
            terms.insert(token.trim_end_matches('s').to_string());
        }
    }
    terms
}

fn is_memory_stopword(token: &str) -> bool {
    token.is_ascii()
        && matches!(
            token,
            "a" | "an"
                | "and"
                | "are"
                | "as"
                | "at"
                | "be"
                | "by"
                | "for"
                | "from"
                | "in"
                | "is"
                | "it"
                | "of"
                | "on"
                | "or"
                | "that"
                | "the"
                | "this"
                | "to"
                | "was"
                | "with"
                | "you"
                | "your"
        )
}
