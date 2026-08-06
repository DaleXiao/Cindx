use std::path::PathBuf;

pub(super) const SEARCH_FILE_SCAN_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct CandidateFile {
    pub(super) path: PathBuf,
    pub(super) relative: String,
    pub(super) size: u64,
    pub(super) modified_nanos: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SearchPosition {
    pub(super) line: usize,
    pub(super) byte: usize,
}

impl Default for SearchPosition {
    fn default() -> Self {
        Self { line: 1, byte: 0 }
    }
}
