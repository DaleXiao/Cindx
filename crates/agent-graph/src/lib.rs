use agent_rag::{RagChunk, RagSearchResult};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphError {
    pub message: String,
}

impl GraphError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for GraphError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum GraphNodeKind {
    File,
    Symbol,
    Tool,
    Decision,
    Claim,
    Task,
}

impl GraphNodeKind {
    pub fn label(&self) -> &'static str {
        match self {
            GraphNodeKind::File => "file",
            GraphNodeKind::Symbol => "symbol",
            GraphNodeKind::Tool => "tool",
            GraphNodeKind::Decision => "decision",
            GraphNodeKind::Claim => "claim",
            GraphNodeKind::Task => "task",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "file" => Some(Self::File),
            "symbol" => Some(Self::Symbol),
            "tool" => Some(Self::Tool),
            "decision" => Some(Self::Decision),
            "claim" => Some(Self::Claim),
            "task" => Some(Self::Task),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphProvenance {
    pub source_path: String,
    pub start_line: u64,
    pub end_line: u64,
    pub extractor: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub kind: GraphNodeKind,
    pub label: String,
    pub provenance: GraphProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum GraphEdgeKind {
    Mentions,
    Defines,
    Uses,
    Decides,
    RelatedTo,
}

impl GraphEdgeKind {
    pub fn label(&self) -> &'static str {
        match self {
            GraphEdgeKind::Mentions => "mentions",
            GraphEdgeKind::Defines => "defines",
            GraphEdgeKind::Uses => "uses",
            GraphEdgeKind::Decides => "decides",
            GraphEdgeKind::RelatedTo => "related_to",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mentions" => Some(Self::Mentions),
            "defines" => Some(Self::Defines),
            "uses" => Some(Self::Uses),
            "decides" => Some(Self::Decides),
            "related_to" => Some(Self::RelatedTo),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: GraphEdgeKind,
    pub provenance: GraphProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphExtraction {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

pub trait GraphStore {
    fn upsert(&mut self, extraction: GraphExtraction) -> Result<(), GraphError>;
    fn nodes(&self) -> Vec<GraphNode>;
    fn edges(&self) -> Vec<GraphEdge>;
    fn neighbors(&self, node_id: &str, limit: usize) -> Vec<GraphNode>;
    fn nodes_by_label(&self, label: &str) -> Vec<GraphNode>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileGraphStore {
    path: PathBuf,
    nodes: BTreeMap<String, GraphNode>,
    edges: BTreeMap<String, GraphEdge>,
}

impl FileGraphStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, GraphError> {
        let path = path.into();
        let mut store = Self {
            path: path.clone(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
        };
        if path.exists() {
            store.load()?;
        }
        Ok(store)
    }

    pub fn upsert_all(
        &mut self,
        extractions: impl IntoIterator<Item = GraphExtraction>,
    ) -> Result<(), GraphError> {
        for extraction in extractions {
            self.merge(extraction);
        }
        self.save()
    }

    fn merge(&mut self, extraction: GraphExtraction) {
        for node in extraction.nodes {
            self.nodes.insert(node.id.clone(), node);
        }
        for edge in extraction.edges {
            self.edges.insert(edge.id.clone(), edge);
        }
    }

    fn load(&mut self) -> Result<(), GraphError> {
        let file = fs::File::open(&self.path)
            .map_err(|error| GraphError::new(format!("failed to read graph store: {error}")))?;
        for line in BufReader::new(file).lines() {
            let line = line
                .map_err(|error| GraphError::new(format!("failed to read graph row: {error}")))?;
            let parts = line.split('\t').collect::<Vec<_>>();
            match parts.as_slice() {
                [
                    "node",
                    id,
                    kind,
                    label,
                    source_path,
                    start_line,
                    end_line,
                    extractor,
                    observed_at_ms,
                ] => {
                    if let Some(kind) = GraphNodeKind::parse(kind) {
                        self.nodes.insert(
                            (*id).to_string(),
                            GraphNode {
                                id: (*id).to_string(),
                                kind,
                                label: decode(label)?,
                                provenance: GraphProvenance {
                                    source_path: decode(source_path)?,
                                    start_line: start_line.parse().unwrap_or(0),
                                    end_line: end_line.parse().unwrap_or(0),
                                    extractor: decode(extractor)?,
                                    observed_at_ms: observed_at_ms.parse().unwrap_or(0),
                                },
                            },
                        );
                    }
                }
                [
                    "edge",
                    id,
                    from,
                    to,
                    kind,
                    source_path,
                    start_line,
                    end_line,
                    extractor,
                    observed_at_ms,
                ] => {
                    if let Some(kind) = GraphEdgeKind::parse(kind) {
                        self.edges.insert(
                            (*id).to_string(),
                            GraphEdge {
                                id: (*id).to_string(),
                                from: (*from).to_string(),
                                to: (*to).to_string(),
                                kind,
                                provenance: GraphProvenance {
                                    source_path: decode(source_path)?,
                                    start_line: start_line.parse().unwrap_or(0),
                                    end_line: end_line.parse().unwrap_or(0),
                                    extractor: decode(extractor)?,
                                    observed_at_ms: observed_at_ms.parse().unwrap_or(0),
                                },
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn save(&self) -> Result<(), GraphError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| GraphError::new(format!("failed to create graph directory: {error}")))?;
        }
        let mut rows = Vec::new();
        for node in self.nodes.values() {
            rows.push(format!(
                "node\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                node.id,
                node.kind.label(),
                encode(&node.label),
                encode(&node.provenance.source_path),
                node.provenance.start_line,
                node.provenance.end_line,
                encode(&node.provenance.extractor),
                node.provenance.observed_at_ms
            ));
        }
        for edge in self.edges.values() {
            rows.push(format!(
                "edge\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                edge.id,
                edge.from,
                edge.to,
                edge.kind.label(),
                encode(&edge.provenance.source_path),
                edge.provenance.start_line,
                edge.provenance.end_line,
                encode(&edge.provenance.extractor),
                edge.provenance.observed_at_ms
            ));
        }
        fs::write(&self.path, rows.join("\n"))
            .map_err(|error| GraphError::new(format!("failed to save graph store: {error}")))
    }
}

impl GraphStore for FileGraphStore {
    fn upsert(&mut self, extraction: GraphExtraction) -> Result<(), GraphError> {
        self.merge(extraction);
        self.save()
    }

    fn nodes(&self) -> Vec<GraphNode> {
        self.nodes.values().cloned().collect()
    }

    fn edges(&self) -> Vec<GraphEdge> {
        self.edges.values().cloned().collect()
    }

    fn neighbors(&self, node_id: &str, limit: usize) -> Vec<GraphNode> {
        let mut ids = BTreeSet::new();
        for edge in self.edges.values() {
            if edge.from == node_id {
                ids.insert(edge.to.clone());
            }
            if edge.to == node_id {
                ids.insert(edge.from.clone());
            }
        }
        ids.into_iter()
            .filter_map(|id| self.nodes.get(&id).cloned())
            .take(limit.max(1))
            .collect()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<GraphNode> {
        let normalized = label.to_ascii_lowercase();
        self.nodes
            .values()
            .filter(|node| node.label.to_ascii_lowercase().contains(&normalized))
            .cloned()
            .collect()
    }
}

pub fn extract_graph_from_chunk(chunk: &RagChunk) -> GraphExtraction {
    let provenance = GraphProvenance {
        source_path: chunk.path.clone(),
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        extractor: "deterministic-v1".to_string(),
        observed_at_ms: current_time_millis(),
    };
    let file_node = node(GraphNodeKind::File, &chunk.path, provenance.clone());
    let mut nodes = vec![file_node.clone()];
    let mut edges = Vec::new();

    for token in graph_tokens(&chunk.text) {
        if let Some(kind) = classify_token(&token) {
            let target = node(kind.clone(), &token, provenance.clone());
            let edge_kind = match kind {
                GraphNodeKind::Tool => GraphEdgeKind::Uses,
                GraphNodeKind::Decision => GraphEdgeKind::Decides,
                GraphNodeKind::File => GraphEdgeKind::Mentions,
                GraphNodeKind::Symbol => GraphEdgeKind::Defines,
                GraphNodeKind::Claim | GraphNodeKind::Task => GraphEdgeKind::RelatedTo,
            };
            nodes.push(target.clone());
            edges.push(edge(&file_node.id, &target.id, edge_kind, provenance.clone()));
        }
    }

    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    nodes.dedup_by(|left, right| left.id == right.id);
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    edges.dedup_by(|left, right| left.id == right.id);

    GraphExtraction { nodes, edges }
}

pub fn build_graph_extraction_prompt(chunks: &[RagChunk]) -> String {
    let mut prompt = String::new();
    prompt.push_str("Extract graph facts from the local workspace sources.\n");
    prompt.push_str("Return nodes and edges with provenance. Do not invent facts.\n\n");
    for chunk in chunks {
        prompt.push_str(&format!(
            "[{}:{}-{} hash={}]\n{}\n\n",
            chunk.path, chunk.start_line, chunk.end_line, chunk.file_hash, chunk.text
        ));
    }
    prompt
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphRagSource {
    pub chunk: RagChunk,
    pub score: f32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphRagTrace {
    pub seeds: Vec<GraphRagSource>,
    pub direct: Vec<GraphRagSource>,
    pub walked: Vec<GraphRagSource>,
    pub neighbors: Vec<GraphRagSource>,
    pub selected: Vec<GraphRagSource>,
}

pub fn graph_direct_recall(
    query: &str,
    chunks: &[RagChunk],
    store: &dyn GraphStore,
    limit: usize,
) -> Vec<GraphRagSource> {
    let mut paths = BTreeSet::new();
    let normalized_query = query.to_lowercase();
    for token in graph_tokens(query) {
        for node in store.nodes_by_label(&token) {
            paths.insert(node.provenance.source_path.clone());
            if node.kind == GraphNodeKind::File {
                paths.insert(node.label.clone());
            }
        }
    }
    for node in store.nodes() {
        let label = node.label.to_lowercase();
        if label.chars().count() >= 2 && normalized_query.contains(&label) {
            paths.insert(node.provenance.source_path.clone());
            if node.kind == GraphNodeKind::File {
                paths.insert(node.label);
            }
        }
    }
    graph_sources_for_paths(paths, chunks, "graph_recall", 0.65, limit)
}

pub fn graph_walk_recall(
    seed_results: &[RagSearchResult],
    chunks: &[RagChunk],
    store: &dyn GraphStore,
    limit: usize,
) -> Vec<GraphRagSource> {
    let mut paths = BTreeSet::new();
    for seed in seed_results {
        let file_id = node_id(GraphNodeKind::File, &seed.chunk.path);
        for neighbor in store.neighbors(&file_id, 24) {
            if neighbor.kind == GraphNodeKind::File {
                paths.insert(neighbor.label);
            } else {
                for related in store.neighbors(&neighbor.id, 24) {
                    if related.kind == GraphNodeKind::File {
                        paths.insert(related.label);
                    }
                }
            }
        }
    }
    let seed_ids = seed_results
        .iter()
        .map(|result| result.chunk.id.as_str())
        .collect::<BTreeSet<_>>();
    graph_sources_for_paths(paths, chunks, "graph_walk", 0.5, limit)
        .into_iter()
        .filter(|source| !seed_ids.contains(source.chunk.id.as_str()))
        .collect()
}

pub fn graph_rag_walk(
    query: &str,
    seed_results: &[RagSearchResult],
    chunks: &[RagChunk],
    store: &dyn GraphStore,
    limit: usize,
) -> GraphRagTrace {
    let limit = limit.max(1);
    let seeds = seed_results
        .iter()
        .map(|result| GraphRagSource {
            chunk: result.chunk.clone(),
            score: result.score,
            reason: "semantic_rag".to_string(),
        })
        .collect::<Vec<_>>();
    let direct = graph_direct_recall(query, chunks, store, limit.saturating_mul(2));
    let walked = graph_walk_recall(seed_results, chunks, store, limit.saturating_mul(2));
    let mut neighbors = direct.clone();
    neighbors.extend(walked.clone());
    neighbors.sort_by(|left, right| {
        left
            .chunk
            .id
            .cmp(&right.chunk.id)
            .then_with(|| right.score.partial_cmp(&left.score).unwrap_or(std::cmp::Ordering::Equal))
    });
    neighbors.dedup_by(|left, right| left.chunk.id == right.chunk.id);

    let mut selected = seeds.clone();
    selected.extend(neighbors.clone());
    selected.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.chunk.path.cmp(&right.chunk.path))
    });
    selected.truncate(limit);

    GraphRagTrace {
        seeds,
        direct,
        walked,
        neighbors,
        selected,
    }
}

fn graph_sources_for_paths(
    paths: BTreeSet<String>,
    chunks: &[RagChunk],
    reason: &str,
    score: f32,
    limit: usize,
) -> Vec<GraphRagSource> {
    let mut sources = chunks
        .iter()
        .filter(|chunk| paths.contains(&chunk.path))
        .map(|chunk| GraphRagSource {
            chunk: chunk.clone(),
            score,
            reason: reason.to_string(),
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| {
        left
            .chunk
            .path
            .cmp(&right.chunk.path)
            .then_with(|| left.chunk.start_line.cmp(&right.chunk.start_line))
    });
    sources.truncate(limit.max(1));
    sources
}

fn node(kind: GraphNodeKind, label: &str, provenance: GraphProvenance) -> GraphNode {
    GraphNode {
        id: node_id(kind.clone(), label),
        kind,
        label: label.to_string(),
        provenance,
    }
}

fn edge(from: &str, to: &str, kind: GraphEdgeKind, provenance: GraphProvenance) -> GraphEdge {
    GraphEdge {
        id: format!("edge:{}:{}:{}", kind.label(), from, to),
        from: from.to_string(),
        to: to.to_string(),
        kind,
        provenance,
    }
}

fn node_id(kind: GraphNodeKind, label: &str) -> String {
    format!("node:{}:{}", kind.label(), label.to_ascii_lowercase())
}

fn classify_token(token: &str) -> Option<GraphNodeKind> {
    if token.contains('/') || token.ends_with(".rs") || token.ends_with(".ts") || token.ends_with(".tsx") || token.ends_with(".md") {
        return Some(GraphNodeKind::File);
    }
    if token.contains('.') && !token.starts_with('.') {
        return Some(GraphNodeKind::Tool);
    }
    if matches!(token, "decision" | "decide" | "decided" | "approved" | "denied" | "policy") {
        return Some(GraphNodeKind::Decision);
    }
    if token.chars().next().is_some_and(char::is_uppercase) || token.contains("::") {
        return Some(GraphNodeKind::Symbol);
    }
    None
}

fn graph_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | ':') {
            current.push(character);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode(value: &str) -> Result<String, GraphError> {
    if !value.len().is_multiple_of(2) {
        return Err(GraphError::new("hex value has odd length"));
    }
    let mut bytes = Vec::new();
    for chunk in value.as_bytes().chunks_exact(2) {
        let text = std::str::from_utf8(chunk)
            .map_err(|error| GraphError::new(format!("invalid hex text: {error}")))?;
        bytes.push(
            u8::from_str_radix(text, 16)
                .map_err(|error| GraphError::new(format!("invalid hex byte: {error}")))?,
        );
    }
    String::from_utf8(bytes).map_err(|error| GraphError::new(error.to_string()))
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(path: &str, text: &str) -> RagChunk {
        RagChunk {
            id: format!("chunk-{path}"),
            path: path.to_string(),
            file_hash: "hash".to_string(),
            modified_time_ms: 0,
            start_line: 1,
            end_line: 3,
            indexed_at_ms: 0,
            text: text.to_string(),
            embedding: vec![1.0, 0.0],
            embedding_provider: "test".to_string(),
            embedding_model: "test".to_string(),
            embedding_dimensions: 2,
        }
    }

    #[test]
    fn extracts_file_tool_symbol_and_decision_nodes() {
        let extraction = extract_graph_from_chunk(&chunk(
            "docs/a.md",
            "Use file.read with AgentRuntime and approved policy docs/b.md",
        ));

        assert!(extraction.nodes.iter().any(|node| node.kind == GraphNodeKind::File && node.label == "docs/a.md"));
        assert!(extraction.nodes.iter().any(|node| node.kind == GraphNodeKind::Tool && node.label == "file.read"));
        assert!(extraction.nodes.iter().any(|node| node.kind == GraphNodeKind::Symbol && node.label == "AgentRuntime"));
        assert!(extraction.nodes.iter().any(|node| node.kind == GraphNodeKind::Decision && node.label == "approved"));
        assert!(!extraction.edges.is_empty());
    }

    #[test]
    fn file_graph_store_persists_and_returns_neighbors() {
        let path = std::env::temp_dir().join(format!("agent-graph-{}.tsv", current_time_millis()));
        let source = chunk("docs/a.md", "Use file.read with docs/b.md");
        let extraction = extract_graph_from_chunk(&source);
        let mut store = FileGraphStore::open(&path).expect("store should open");
        store.upsert(extraction).expect("graph should save");

        let loaded = FileGraphStore::open(&path).expect("store should reload");
        let neighbors = loaded.neighbors(&node_id(GraphNodeKind::File, "docs/a.md"), 10);

        assert!(!loaded.nodes().is_empty());
        assert!(neighbors.iter().any(|node| node.label == "file.read" || node.label == "docs/b.md"));
    }

    #[test]
    fn file_graph_store_persists_a_batch() {
        let path = std::env::temp_dir().join(format!(
            "agent-graph-batch-{}-{}.tsv",
            std::process::id(),
            current_time_millis()
        ));
        let mut store = FileGraphStore::open(&path).expect("store should open");
        store
            .upsert_all([
                extract_graph_from_chunk(&chunk("docs/a.md", "Use file.read")),
                extract_graph_from_chunk(&chunk("docs/b.md", "Use shell.run")),
            ])
            .expect("graph batch should save");

        let loaded = FileGraphStore::open(&path).expect("store should reload");

        assert!(loaded.nodes().iter().any(|node| node.label == "docs/a.md"));
        assert!(loaded.nodes().iter().any(|node| node.label == "docs/b.md"));
    }

    #[test]
    fn graph_extraction_prompt_contains_provenance() {
        let prompt = build_graph_extraction_prompt(&[chunk("src/lib.rs", "AgentRuntime")]);

        assert!(prompt.contains("[src/lib.rs:1-3"));
        assert!(prompt.contains("Do not invent facts"));
    }

    #[test]
    fn graph_rag_walk_adds_neighbor_sources() {
        let seed = chunk("docs/a.md", "Use file.read");
        let neighbor = chunk("docs/b.md", "file.read details");
        let path = std::env::temp_dir().join(format!("agent-graph-walk-{}.tsv", current_time_millis()));
        let mut store = FileGraphStore::open(&path).expect("store should open");
        store
            .upsert(extract_graph_from_chunk(&seed))
            .expect("seed graph should save");
        store
            .upsert(extract_graph_from_chunk(&neighbor))
            .expect("neighbor graph should save");

        let trace = graph_rag_walk(
            "file.read",
            &[RagSearchResult {
                chunk: seed.clone(),
                score: 0.9,
            }],
            &[seed, neighbor],
            &store,
            4,
        );

        assert_eq!(trace.seeds.len(), 1);
        assert!(!trace.direct.is_empty());
        assert!(!trace.walked.is_empty());
        assert!(trace.neighbors.iter().any(|source| source.chunk.path == "docs/b.md"));
        assert!(trace.walked.iter().any(|source| source.reason == "graph_walk"));
    }
}
