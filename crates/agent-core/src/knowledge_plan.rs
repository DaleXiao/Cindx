use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRetrievalChannel {
    Semantic,
    FileSearch,
    GraphDirect,
    GraphWalk,
}

impl WorkspaceRetrievalChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Semantic => "semantic_rag",
            Self::FileSearch => "file_search",
            Self::GraphDirect => "graph_recall",
            Self::GraphWalk => "graph_walk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecallPolicy {
    None,
    Relevant,
    Comprehensive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRetrievalPlan {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub channels: BTreeSet<WorkspaceRetrievalChannel>,
    #[serde(default = "default_retrieval_limit")]
    pub max_results: usize,
}

fn default_retrieval_limit() -> usize {
    8
}

impl WorkspaceRetrievalPlan {
    pub fn none() -> Self {
        Self {
            query: String::new(),
            channels: BTreeSet::new(),
            max_results: default_retrieval_limit(),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.channels.is_empty()
    }

    pub fn mode_label(&self) -> String {
        if self.channels.is_empty() {
            return "none".to_string();
        }
        self.channels
            .iter()
            .copied()
            .map(WorkspaceRetrievalChannel::label)
            .collect::<Vec<_>>()
            .join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecallPlan {
    pub policy: MemoryRecallPolicy,
    #[serde(default)]
    pub query: String,
}

impl MemoryRecallPlan {
    pub fn none() -> Self {
        Self {
            policy: MemoryRecallPolicy::None,
            query: String::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.policy != MemoryRecallPolicy::None
    }
}
