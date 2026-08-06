#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListEntry {
    pub(super) kind: String,
    pub(super) bytes: u64,
    pub(super) name: String,
}

impl ListEntry {
    pub(super) fn row(&self) -> String {
        format!("{}\t{}\t{}", self.kind, self.bytes, self.name)
    }

    pub(super) fn structured(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind,
            "bytes": self.bytes,
            "name": self.name,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListFailure {
    pub(super) name: Option<String>,
    pub(super) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ListCursor {
    pub(super) snapshot: u64,
    pub(super) offset: usize,
}

#[derive(Default)]
pub(super) struct ListSnapshot {
    pub(super) entries: Vec<ListEntry>,
    pub(super) failures: Vec<ListFailure>,
    pub(super) discovered: usize,
    pub(super) discovery_limit: usize,
    pub(super) discovery_limit_reached: bool,
    pub(super) cancelled: bool,
}
