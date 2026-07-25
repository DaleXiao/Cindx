use agent_core::Event;
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
