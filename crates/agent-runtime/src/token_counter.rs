//! Model token counting for context packing, compaction triggers, and budget
//! accounting.
//!
//! The default counter is a real BPE tokenizer (`cl100k_base` via
//! `tiktoken-rs`) held in a lazy process-wide singleton because initialization
//! is expensive. If the tokenizer cannot be initialized the counter degrades
//! to the legacy character heuristic instead of panicking, so callers always
//! get a usable estimate.

use std::sync::OnceLock;

/// Counts model tokens for a piece of text. Implementations must be safe to
/// call concurrently on hot paths and must never panic.
pub trait TokenCounter: Send + Sync {
    fn count_tokens(&self, text: &str) -> u64;

    /// Stable identity for diagnostics and tests.
    fn kind(&self) -> &'static str;
}

/// Default counter backed by the real `cl100k_base` BPE tokenizer. Falls back
/// to the legacy character heuristic if tokenizer initialization fails.
pub struct Cl100kTokenCounter;

impl TokenCounter for Cl100kTokenCounter {
    fn count_tokens(&self, text: &str) -> u64 {
        count_with_bpe(cl100k_bpe(), text)
    }

    fn kind(&self) -> &'static str {
        if cl100k_bpe().is_some() {
            "cl100k_base"
        } else {
            "heuristic_fallback"
        }
    }
}

/// Legacy character heuristic retained as the degradation path when the BPE
/// tokenizer is unavailable.
pub struct HeuristicTokenCounter;

impl TokenCounter for HeuristicTokenCounter {
    fn count_tokens(&self, text: &str) -> u64 {
        heuristic_text_tokens(text)
    }

    fn kind(&self) -> &'static str {
        "heuristic"
    }
}

/// Count tokens for `text` with the process-wide default counter.
pub fn count_text_tokens(text: &str) -> u64 {
    default_counter().count_tokens(text)
}

/// Identity of the active default counter (`cl100k_base`, or
/// `heuristic_fallback` when tokenizer initialization failed).
pub fn token_counter_kind() -> &'static str {
    default_counter().kind()
}

fn default_counter() -> &'static dyn TokenCounter {
    static DEFAULT: Cl100kTokenCounter = Cl100kTokenCounter;
    &DEFAULT
}

fn cl100k_bpe() -> Option<&'static tiktoken_rs::CoreBPE> {
    static BPE: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

fn count_with_bpe(bpe: Option<&tiktoken_rs::CoreBPE>, text: &str) -> u64 {
    match bpe {
        Some(bpe) => bpe.encode_ordinary(text).len() as u64,
        None => heuristic_text_tokens(text),
    }
}

pub(crate) fn heuristic_text_tokens(value: &str) -> u64 {
    if value.is_ascii() {
        return (value.len() as u64)
            .saturating_add(2)
            .checked_div(3)
            .unwrap_or_default()
            .saturating_add(u64::from(!value.is_empty()));
    }

    let mut ascii = 0_u64;
    let mut non_ascii = 0_u64;
    for character in value.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or_default()
        .saturating_add(non_ascii)
        .saturating_add(u64::from(!value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cl100k_counter_counts_real_bpe_tokens() {
        let counter = Cl100kTokenCounter;
        assert_eq!(counter.kind(), "cl100k_base");
        assert_eq!(counter.count_tokens(""), 0);
        // "hello world" is exactly two cl100k_base tokens.
        assert_eq!(counter.count_tokens("hello world"), 2);
        // BPE merges repeated characters, so a run encodes well below the old
        // len/3 heuristic.
        let repeated = "a".repeat(300);
        let real = counter.count_tokens(&repeated);
        assert!(real > 0 && real < heuristic_text_tokens(&repeated));
    }

    #[test]
    fn missing_tokenizer_degrades_to_the_heuristic() {
        let text = "some context text 上下文";
        assert_eq!(count_with_bpe(None, text), heuristic_text_tokens(text));
        assert_eq!(
            HeuristicTokenCounter.count_tokens(text),
            heuristic_text_tokens(text)
        );
    }

    #[test]
    fn default_counter_matches_cl100k_counts() {
        assert_eq!(token_counter_kind(), "cl100k_base");
        assert_eq!(
            count_text_tokens("hello world"),
            Cl100kTokenCounter.count_tokens("hello world")
        );
    }
}
