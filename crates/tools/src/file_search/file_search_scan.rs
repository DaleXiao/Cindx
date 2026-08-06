use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

use regex::Regex;

use super::file_search_types::{CandidateFile, SearchPosition, SEARCH_FILE_SCAN_MAX_BYTES};
use crate::file_query_contract_v3::{
    invalid_search_cursor, SearchContextLine, SearchCoverage, SearchMatch,
};
use crate::ToolError;

const SEARCH_MATCH_PREVIEW_CHARS: usize = 512;
const SEARCH_CANCEL_SAMPLE_LINES: usize = 1_024;

pub(super) fn planned_read_bytes(
    candidate: &CandidateFile,
    position: SearchPosition,
    context_lines: usize,
) -> u64 {
    let scan_end = candidate.size.min(SEARCH_FILE_SCAN_MAX_BYTES);
    let read_start = if context_lines == 0 {
        position.byte as u64
    } else {
        0
    };
    scan_end.saturating_sub(read_start.min(scan_end))
}

pub(super) fn read_candidate(
    candidate: &CandidateFile,
    position: SearchPosition,
    context_lines: usize,
    coverage: &mut SearchCoverage,
) -> Result<Option<CandidateContents>, ToolError> {
    let Ok(mut file) = fs::File::open(&candidate.path) else {
        coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
        return Ok(None);
    };
    let scan_end = candidate.size.min(SEARCH_FILE_SCAN_MAX_BYTES);
    let start = if context_lines == 0 {
        position
    } else {
        SearchPosition::default()
    };
    if start.byte as u64 > scan_end {
        return Err(invalid_search_cursor(
            "cursor byte offset is outside the searchable file prefix",
        ));
    }
    if file.seek(SeekFrom::Start(start.byte as u64)).is_err() {
        coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
        return Ok(None);
    }
    let limit = scan_end.saturating_sub(start.byte as u64);
    let mut bytes = Vec::with_capacity(limit.min(256 * 1024) as usize);
    if file.take(limit).read_to_end(&mut bytes).is_err() {
        coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
        return Ok(None);
    }
    coverage.scanned_files = coverage.scanned_files.saturating_add(1);
    coverage.scanned_bytes = coverage.scanned_bytes.saturating_add(bytes.len() as u64);
    let truncated = candidate.size > SEARCH_FILE_SCAN_MAX_BYTES;
    if truncated {
        coverage.truncated_files = coverage.truncated_files.saturating_add(1);
    }
    match utf8_search_prefix(bytes, truncated) {
        Some(text) => Ok(Some(CandidateContents { text, start })),
        None => {
            coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
            Ok(None)
        }
    }
}

pub(super) fn search_contents(
    candidate: &CandidateFile,
    contents: &CandidateContents,
    matcher: &Regex,
    matches: &mut Vec<SearchMatch>,
    request: SearchScanRequest<'_>,
) -> bool {
    let SearchScanRequest {
        position,
        context_lines,
        remaining_results,
        should_cancel,
    } = request;
    if remaining_results == 0 {
        return false;
    }
    let mut before = VecDeque::with_capacity(context_lines);
    let mut pending: VecDeque<SearchMatch> =
        VecDeque::with_capacity(context_lines.min(remaining_results));
    let mut accepted = 0usize;
    for (visited, line) in search_lines(contents).enumerate() {
        if visited > 0 && visited % SEARCH_CANCEL_SAMPLE_LINES == 0 && should_cancel() {
            return true;
        }
        if context_lines > 0 {
            let next_context = context_line(&line);
            for pending_match in &mut pending {
                pending_match.after.push(next_context.clone());
            }
            while pending
                .front()
                .is_some_and(|pending_match| pending_match.after.len() >= context_lines)
            {
                matches.push(
                    pending
                        .pop_front()
                        .expect("a completed pending match must exist"),
                );
            }
        }

        if accepted < remaining_results
            && line.byte_offset >= position.byte
            && matcher.is_match(line.text)
        {
            let found = SearchMatch {
                path: candidate.relative.clone(),
                line: line.number,
                byte_offset: line.byte_offset,
                text: preview(line.text),
                before: before.iter().map(context_line).collect(),
                after: Vec::with_capacity(context_lines),
            };
            accepted += 1;
            if context_lines == 0 {
                matches.push(found);
            } else {
                pending.push_back(found);
            }
        }

        if context_lines > 0 {
            before.push_back(line);
            if before.len() > context_lines {
                before.pop_front();
            }
        }
        if accepted >= remaining_results && pending.is_empty() {
            return false;
        }
    }
    matches.extend(pending);
    false
}

pub(super) struct SearchScanRequest<'a> {
    pub(super) position: SearchPosition,
    pub(super) context_lines: usize,
    pub(super) remaining_results: usize,
    pub(super) should_cancel: &'a dyn Fn() -> bool,
}

pub(super) struct CandidateContents {
    text: String,
    start: SearchPosition,
}

struct SearchLine<'a> {
    byte_offset: usize,
    number: usize,
    text: &'a str,
}

fn utf8_search_prefix(bytes: Vec<u8>, truncated: bool) -> Option<String> {
    match String::from_utf8(bytes) {
        Ok(contents) => Some(contents),
        Err(error) if truncated && error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_up_to);
            String::from_utf8(bytes).ok()
        }
        Err(_) => None,
    }
}

fn search_lines(contents: &CandidateContents) -> impl Iterator<Item = SearchLine<'_>> {
    let mut byte_offset = contents.start.byte;
    let mut number = contents.start.line;
    contents.text.split_inclusive('\n').map(move |segment| {
        let text = segment
            .strip_suffix('\n')
            .map_or(segment, |without_newline| {
                without_newline
                    .strip_suffix('\r')
                    .unwrap_or(without_newline)
            });
        let line = SearchLine {
            byte_offset,
            number,
            text,
        };
        byte_offset = byte_offset.saturating_add(segment.len());
        number = number.saturating_add(1);
        line
    })
}

fn context_line(line: &SearchLine<'_>) -> SearchContextLine {
    SearchContextLine {
        line: line.number,
        text: preview(line.text),
    }
}

fn preview(value: &str) -> String {
    value
        .trim()
        .chars()
        .take(SEARCH_MATCH_PREVIEW_CHARS)
        .collect()
}
