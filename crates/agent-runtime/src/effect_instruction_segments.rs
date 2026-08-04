type TextPredicate = fn(&str) -> bool;

#[derive(Clone, Copy)]
enum InstructionSeparator {
    Clause,
    Compound {
        starts_effect_action: TextPredicate,
        chinese_target_boundary: TextPredicate,
    },
}

pub(super) fn effect_clauses(value: &str) -> impl Iterator<Item = &str> {
    split_outside_literals(value, InstructionSeparator::Clause)
}

pub(super) fn compound_action_segments(
    value: &str,
    starts_effect_action: TextPredicate,
    chinese_target_boundary: TextPredicate,
) -> impl Iterator<Item = &str> {
    split_outside_literals(
        value,
        InstructionSeparator::Compound {
            starts_effect_action,
            chinese_target_boundary,
        },
    )
}

fn split_outside_literals(
    value: &str,
    separator: InstructionSeparator,
) -> impl Iterator<Item = &str> {
    let mut remaining = Some(value);
    std::iter::from_fn(move || {
        let current = remaining.take()?;
        if current.is_empty() {
            return None;
        }
        if let Some((index, width)) = next_instruction_separator(current, separator) {
            remaining = Some(&current[index + width..]);
            Some(&current[..index])
        } else {
            Some(current)
        }
    })
}

fn next_instruction_separator(
    value: &str,
    separator: InstructionSeparator,
) -> Option<(usize, usize)> {
    let mut index = 0usize;
    let mut in_fence = false;
    let mut in_backtick = false;
    let mut in_quote = false;
    let mut in_smart_quote = false;
    let mut in_single_quote = false;
    let mut in_smart_single_quote = false;
    while index < value.len() {
        let remaining = &value[index..];
        if !in_backtick
            && !in_quote
            && !in_smart_quote
            && !in_single_quote
            && !in_smart_single_quote
            && remaining.starts_with("```")
        {
            in_fence = !in_fence;
            index += 3;
            continue;
        }
        let character = remaining.chars().next()?;
        let width = character.len_utf8();
        if !in_fence {
            match character {
                '`' if !in_quote
                    && !in_smart_quote
                    && !in_single_quote
                    && !in_smart_single_quote =>
                {
                    in_backtick = !in_backtick;
                }
                '"' if !in_backtick
                    && !in_smart_quote
                    && !in_single_quote
                    && !in_smart_single_quote
                    && !ascii_character_is_escaped(value, index) =>
                {
                    in_quote = !in_quote;
                }
                '\'' if !in_backtick
                    && !in_quote
                    && !in_smart_quote
                    && !in_smart_single_quote
                    && !ascii_character_is_escaped(value, index) =>
                {
                    if in_single_quote {
                        in_single_quote = false;
                    } else if ascii_single_quote_opens(value, index) {
                        in_single_quote = true;
                    }
                }
                '“' if !in_backtick && !in_quote && !in_single_quote => {
                    in_smart_quote = true;
                }
                '”' if !in_backtick && !in_quote && !in_single_quote => {
                    in_smart_quote = false;
                }
                '‘' if !in_backtick && !in_quote && !in_smart_quote => {
                    in_smart_single_quote = true;
                }
                '’' if !in_backtick && !in_quote && !in_smart_quote => {
                    in_smart_single_quote = false;
                }
                _ => {}
            }
        }
        if !in_fence
            && !in_backtick
            && !in_quote
            && !in_smart_quote
            && !in_single_quote
            && !in_smart_single_quote
        {
            let matched = match separator {
                InstructionSeparator::Clause => {
                    matches!(character, '\n' | '。' | ';' | '；' | '!' | '！')
                        .then_some(width)
                        .or_else(|| {
                            (character == '.'
                                && remaining[width..]
                                    .chars()
                                    .next()
                                    .is_some_and(char::is_whitespace))
                            .then_some(width)
                        })
                }
                InstructionSeparator::Compound {
                    starts_effect_action,
                    chinese_target_boundary,
                } => matches!(character, ',' | '，')
                    .then_some(width)
                    .or_else(|| {
                        [" and then ", " then ", " and ", "然后"]
                            .into_iter()
                            .find(|candidate| remaining.starts_with(candidate))
                            .map(str::len)
                    })
                    .or_else(|| {
                        let candidate_width = ["并", "且"].into_iter().find_map(|candidate| {
                            remaining.strip_prefix(candidate).and_then(|after| {
                                starts_effect_action(after.trim_start()).then_some(candidate.len())
                            })
                        })?;
                        chinese_target_boundary(&value[..index]).then_some(candidate_width)
                    }),
            };
            if let Some(width) = matched {
                return Some((index, width));
            }
        }
        index += width;
    }
    None
}

fn ascii_character_is_escaped(value: &str, index: usize) -> bool {
    value.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

fn ascii_single_quote_opens(value: &str, index: usize) -> bool {
    value[..index]
        .chars()
        .next_back()
        .is_none_or(|character| !character.is_alphanumeric() && character != '_')
}
