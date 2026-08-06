use std::collections::BTreeMap;

use glob::Pattern;
use regex::{Regex, RegexBuilder};

use crate::file_query_contract_v3::SearchOptions;
use crate::{parse_bounded_usize_input, parse_input, required_input, ToolError};

const SEARCH_MAX_CONTEXT_LINES: usize = 8;

pub(super) struct ParsedSearchInput {
    pub(super) options: SearchOptions,
    pub(super) matcher: Regex,
    pub(super) path_pattern: Option<Pattern>,
    pub(super) cursor: Option<String>,
}

pub(super) fn parse_search_input(input_json: &str) -> Result<ParsedSearchInput, ToolError> {
    let input = parse_input(input_json);
    let options = SearchOptions {
        query: required_input(&input, "query")?,
        path: input
            .get("path")
            .cloned()
            .unwrap_or_else(|| ".".to_string()),
        regex: parse_bool(&input, "regex", false)?,
        case_sensitive: parse_bool(&input, "case_sensitive", true)?,
        glob: input
            .get("glob")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        context_lines: parse_bounded_usize_input(
            &input,
            "context_lines",
            0,
            0,
            SEARCH_MAX_CONTEXT_LINES,
        )?,
        max_results: parse_bounded_usize_input(&input, "max_results", 50, 1, 200)?,
    };
    let matcher = compile_matcher(&options)?;
    let path_pattern = compile_glob(options.glob.as_deref())?;
    Ok(ParsedSearchInput {
        options,
        matcher,
        path_pattern,
        cursor: input.get("cursor").cloned(),
    })
}

fn compile_matcher(options: &SearchOptions) -> Result<Regex, ToolError> {
    let pattern = if options.regex {
        options.query.clone()
    } else {
        regex::escape(&options.query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|error| typed_error("invalid_regex", format!("invalid search regex: {error}")))
}

fn compile_glob(value: Option<&str>) -> Result<Option<Pattern>, ToolError> {
    value
        .map(|value| {
            Pattern::new(value)
                .map_err(|error| typed_error("invalid_glob", format!("invalid path glob: {error}")))
        })
        .transpose()
}

fn parse_bool(
    input: &BTreeMap<String, String>,
    key: &str,
    default: bool,
) -> Result<bool, ToolError> {
    match input.get(key).map(String::as_str) {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err(typed_error(
            "invalid_input",
            format!("{key} must be true or false"),
        )),
    }
}

fn typed_error(code: &str, message: String) -> ToolError {
    ToolError {
        code: code.to_string(),
        message,
        retryable: false,
    }
}
