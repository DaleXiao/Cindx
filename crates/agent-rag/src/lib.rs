use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const EMBEDDING_DIMS: usize = 64;
const DEFAULT_MAX_FILE_BYTES: u64 = 512 * 1024;
const DEFAULT_MAX_FILES: usize = 10_000;
const DEFAULT_CHUNK_LINES: usize = 80;
const DEFAULT_CHUNK_OVERLAP: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagError {
    pub message: String,
}

impl RagError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RagError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RagError {}

#[derive(Debug, Clone, PartialEq)]
pub struct RagChunk {
    pub id: String,
    pub path: String,
    pub file_hash: String,
    pub modified_time_ms: u64,
    pub start_line: u64,
    pub end_line: u64,
    pub indexed_at_ms: u64,
    pub text: String,
    pub embedding: Vec<f32>,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagSearchResult {
    pub chunk: RagChunk,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagIndexStats {
    pub files_indexed: usize,
    pub chunks_indexed: usize,
    pub indexed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOptions {
    pub max_file_bytes: u64,
    pub max_files: usize,
    pub chunk_lines: usize,
    pub chunk_overlap: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_files: DEFAULT_MAX_FILES,
            chunk_lines: DEFAULT_CHUNK_LINES,
            chunk_overlap: DEFAULT_CHUNK_OVERLAP,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagIndex {
    pub chunks: Vec<RagChunk>,
    pub stats: RagIndexStats,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingBatch {
    pub provider: String,
    pub model: String,
    pub vectors: Vec<Vec<f32>>,
}

pub trait RagEmbedder {
    fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, RagError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct LanceDbRecord {
    pub id: String,
    pub path: String,
    pub file_hash: String,
    pub modified_time_ms: u64,
    pub start_line: u64,
    pub end_line: u64,
    pub indexed_at_ms: u64,
    pub text: String,
    pub vector: Vec<f32>,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
}

pub trait RagAdapter {
    fn replace_all(&mut self, index: RagIndex) -> Result<RagIndexStats, RagError>;

    fn search(&self, query: &str, limit: usize) -> Result<Vec<RagSearchResult>, RagError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRagAdapter {
    path: PathBuf,
    index: RagIndex,
}

impl FileRagAdapter {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, RagError> {
        let path = path.into();
        let index = if path.exists() {
            load_index(&path)?
        } else {
            empty_index()
        };

        Ok(Self { path, index })
    }

    pub fn stats(&self) -> &RagIndexStats {
        &self.index.stats
    }

    pub fn chunks(&self) -> &[RagChunk] {
        &self.index.chunks
    }

    pub fn embedding_profile(&self) -> Option<(&str, &str, usize)> {
        self.index.chunks.first().map(|chunk| {
            (
                chunk.embedding_provider.as_str(),
                chunk.embedding_model.as_str(),
                chunk.embedding_dimensions,
            )
        })
    }
}

impl RagAdapter for FileRagAdapter {
    fn replace_all(&mut self, index: RagIndex) -> Result<RagIndexStats, RagError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| RagError::new(format!("failed to create RAG directory: {error}")))?;
        }
        save_index(&self.path, &index)?;
        self.index = index;

        Ok(self.index.stats.clone())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<RagSearchResult>, RagError> {
        Ok(search_chunks(&self.index.chunks, query, limit))
    }
}

pub fn index_workspace(
    workspace_root: impl AsRef<Path>,
    options: IndexOptions,
) -> Result<RagIndex, RagError> {
    let workspace_root = workspace_root.as_ref();
    let indexed_at_ms = current_time_millis();
    let mut chunks = Vec::new();
    let mut files_indexed = 0;

    collect_chunks(
        workspace_root,
        workspace_root,
        &options,
        indexed_at_ms,
        &mut chunks,
        &mut files_indexed,
    )?;

    let stats = RagIndexStats {
        files_indexed,
        chunks_indexed: chunks.len(),
        indexed_at_ms,
    };

    Ok(RagIndex { chunks, stats })
}

pub fn index_workspace_with_embedder(
    workspace_root: impl AsRef<Path>,
    options: IndexOptions,
    embedder: &mut dyn RagEmbedder,
) -> Result<RagIndex, RagError> {
    let mut index = index_workspace(workspace_root, options)?;
    let texts = index
        .chunks
        .iter()
        .map(|chunk| chunk.text.clone())
        .collect::<Vec<_>>();
    if texts.is_empty() {
        return Ok(index);
    }

    let batch = embedder.embed_texts(&texts)?;
    if batch.vectors.len() != index.chunks.len() {
        return Err(RagError::new(format!(
            "embedding count mismatch: got {}, expected {}",
            batch.vectors.len(),
            index.chunks.len()
        )));
    }

    let provider = batch.provider;
    let model = batch.model;
    for (chunk, vector) in index.chunks.iter_mut().zip(batch.vectors) {
        if vector.is_empty() {
            return Err(RagError::new("embedding vector was empty"));
        }
        chunk.embedding_dimensions = vector.len();
        chunk.embedding = vector;
        chunk.embedding_provider = provider.clone();
        chunk.embedding_model = model.clone();
    }

    Ok(index)
}

pub fn search_chunks(chunks: &[RagChunk], query: &str, limit: usize) -> Vec<RagSearchResult> {
    search_chunks_with_embedding(chunks, query, &local_query_embedding(query), limit)
}

pub fn local_query_embedding(text: &str) -> Vec<f32> {
    embed_text(text)
}

pub fn search_chunks_semantic(
    chunks: &[RagChunk],
    query_embedding: &[f32],
    limit: usize,
) -> Vec<RagSearchResult> {
    let limit = limit.max(1).min(50);
    let mut results = chunks
        .iter()
        .filter(|chunk| chunk.embedding_dimensions == query_embedding.len())
        .cloned()
        .map(|chunk| RagSearchResult {
            score: cosine_similarity(query_embedding, &chunk.embedding),
            chunk,
        })
        .filter(|result| result.score > 0.0)
        .collect::<Vec<_>>();
    sort_and_truncate_results(&mut results, limit);
    results
}

pub fn search_chunks_literal(
    chunks: &[RagChunk],
    query: &str,
    limit: usize,
) -> Vec<RagSearchResult> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Vec::new();
    }
    let query_tokens = token_counts(&normalized_query);
    let mut results = chunks
        .iter()
        .cloned()
        .filter_map(|chunk| {
            let normalized_text = chunk.text.to_lowercase();
            let exact_matches = normalized_text.matches(&normalized_query).count();
            let lexical_score = lexical_overlap(&query_tokens, &token_counts(&normalized_text));
            let score = if exact_matches > 0 {
                1.0 + (exact_matches.min(8) as f32 * 0.05)
            } else {
                lexical_score
            };
            (score > 0.0).then_some(RagSearchResult { chunk, score })
        })
        .collect::<Vec<_>>();
    sort_and_truncate_results(&mut results, limit.max(1).min(50));
    results
}

pub fn search_chunks_with_embedding(
    chunks: &[RagChunk],
    query: &str,
    query_embedding: &[f32],
    limit: usize,
) -> Vec<RagSearchResult> {
    let limit = limit.max(1).min(50);
    let query_tokens = token_counts(query);
    let mut results = chunks
        .iter()
        .cloned()
        .map(|chunk| {
            let vector_score = if query_embedding.len() == chunk.embedding_dimensions {
                cosine_similarity(query_embedding, &chunk.embedding)
            } else {
                0.0
            };
            let lexical_score = lexical_overlap(&query_tokens, &token_counts(&chunk.text));
            RagSearchResult {
                chunk,
                score: (vector_score * 0.72) + (lexical_score * 0.28),
            }
        })
        .filter(|result| result.score > 0.0)
        .collect::<Vec<_>>();

    sort_and_truncate_results(&mut results, limit);
    results
}

fn sort_and_truncate_results(results: &mut Vec<RagSearchResult>, limit: usize) {
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.chunk.path.cmp(&right.chunk.path))
            .then_with(|| left.chunk.start_line.cmp(&right.chunk.start_line))
    });
    results.truncate(limit);
}

pub fn lancedb_records(index: &RagIndex) -> Vec<LanceDbRecord> {
    index
        .chunks
        .iter()
        .map(|chunk| LanceDbRecord {
            id: chunk.id.clone(),
            path: chunk.path.clone(),
            file_hash: chunk.file_hash.clone(),
            modified_time_ms: chunk.modified_time_ms,
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            indexed_at_ms: chunk.indexed_at_ms,
            text: chunk.text.clone(),
            vector: chunk.embedding.clone(),
            embedding_provider: chunk.embedding_provider.clone(),
            embedding_model: chunk.embedding_model.clone(),
            embedding_dimensions: chunk.embedding_dimensions,
        })
        .collect()
}

pub fn export_lancedb_records_jsonl(
    index: &RagIndex,
    path: impl AsRef<Path>,
) -> Result<usize, RagError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| RagError::new(format!("failed to create LanceDB export directory: {error}")))?;
    }

    let rows = lancedb_records(index)
        .into_iter()
        .map(|record| lancedb_record_json(&record))
        .collect::<Vec<_>>();
    fs::write(path, rows.join("\n"))
        .map_err(|error| RagError::new(format!("failed to export LanceDB records: {error}")))?;

    Ok(rows.len())
}

fn lancedb_record_json(record: &LanceDbRecord) -> String {
    let vector = record
        .vector
        .iter()
        .map(|value| format!("{value:.6}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"id\":\"{}\",\"path\":\"{}\",\"file_hash\":\"{}\",\"modified_time_ms\":{},\"start_line\":{},\"end_line\":{},\"indexed_at_ms\":{},\"text\":\"{}\",\"vector\":[{}],\"embedding_provider\":\"{}\",\"embedding_model\":\"{}\",\"embedding_dimensions\":{}}}",
        json_escape(&record.id),
        json_escape(&record.path),
        json_escape(&record.file_hash),
        record.modified_time_ms,
        record.start_line,
        record.end_line,
        record.indexed_at_ms,
        json_escape(&record.text),
        vector,
        json_escape(&record.embedding_provider),
        json_escape(&record.embedding_model),
        record.embedding_dimensions
    )
}

pub fn build_grounded_answer_prompt(question: &str, results: &[RagSearchResult]) -> String {
    let mut prompt = String::new();
    prompt.push_str("Answer the question using only the provided local workspace sources.\n");
    prompt.push_str("Cite sources inline as [path:start-end]. If the sources are insufficient, say what is missing.\n\n");
    prompt.push_str("Question:\n");
    prompt.push_str(question);
    prompt.push_str("\n\nSources:\n");

    for result in results {
        prompt.push_str(&format!(
            "[{}:{}-{} score={:.3} hash={}]\n{}\n\n",
            result.chunk.path,
            result.chunk.start_line,
            result.chunk.end_line,
            result.score,
            result.chunk.file_hash,
            result.chunk.text
        ));
    }

    prompt
}

fn collect_chunks(
    workspace_root: &Path,
    current: &Path,
    options: &IndexOptions,
    indexed_at_ms: u64,
    chunks: &mut Vec<RagChunk>,
    files_indexed: &mut usize,
) -> Result<(), RagError> {
    if *files_indexed >= options.max_files.max(1) {
        return Ok(());
    }
    let metadata = match fs::metadata(current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(error) => return Err(RagError::new(format!("failed to stat path: {error}"))),
    };
    if metadata.is_file() {
        index_file(workspace_root, current, &metadata, options, indexed_at_ms, chunks, files_indexed)?;
        return Ok(());
    }

    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(error) => return Err(RagError::new(format!("failed to read directory: {error}"))),
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        if *files_indexed >= options.max_files.max(1) {
            break;
        }
        let path = entry.path();
        let name = entry.file_name();
        if should_skip_workspace_entry(workspace_root, current, &name.to_string_lossy()) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_chunks(workspace_root, &path, options, indexed_at_ms, chunks, files_indexed)?;
        } else if metadata.is_file() {
            index_file(workspace_root, &path, &metadata, options, indexed_at_ms, chunks, files_indexed)?;
        }
    }

    Ok(())
}

fn index_file(
    workspace_root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
    options: &IndexOptions,
    indexed_at_ms: u64,
    chunks: &mut Vec<RagChunk>,
    files_indexed: &mut usize,
) -> Result<(), RagError> {
    if *files_indexed >= options.max_files.max(1) {
        return Ok(());
    }
    if metadata.len() > options.max_file_bytes {
        return Ok(());
    }

    let relative = relative_workspace_path(workspace_root, path)?;
    if is_probably_binary_path(&relative) {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    if content.trim().is_empty() {
        return Ok(());
    }

    let file_hash = stable_hash_hex(content.as_bytes());
    let modified_time_ms = metadata
        .modified()
        .ok()
        .and_then(system_time_millis)
        .unwrap_or(0);
    let lines = content.lines().collect::<Vec<_>>();
    let chunk_lines = options.chunk_lines.max(8);
    let overlap = options.chunk_overlap.min(chunk_lines.saturating_sub(1));
    let mut start = 0;

    while start < lines.len() {
        let end = (start + chunk_lines).min(lines.len());
        let text = lines[start..end].join("\n");
        let start_line = start as u64 + 1;
        let end_line = end as u64;
        chunks.push(RagChunk {
            id: stable_hash_hex(format!("{relative}:{start_line}:{end_line}:{file_hash}").as_bytes()),
            path: relative.clone(),
            file_hash: file_hash.clone(),
            modified_time_ms,
            start_line,
            end_line,
            indexed_at_ms,
            embedding: embed_text(&text),
            embedding_provider: "local".to_string(),
            embedding_model: format!("local-hash-{EMBEDDING_DIMS}"),
            embedding_dimensions: EMBEDDING_DIMS,
            text,
        });
        if end == lines.len() {
            break;
        }
        start = end.saturating_sub(overlap);
    }

    *files_indexed += 1;
    Ok(())
}

fn relative_workspace_path(workspace_root: &Path, path: &Path) -> Result<String, RagError> {
    let relative = path
        .strip_prefix(workspace_root)
        .map_err(|_| RagError::new("path is outside workspace"))?;
    for component in relative.components() {
        if matches!(component, Component::ParentDir | Component::Prefix(_) | Component::RootDir) {
            return Err(RagError::new("path escapes workspace"));
        }
    }
    Ok(relative.to_string_lossy().to_string())
}

fn should_skip_path(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let is_env_file = (lower == ".env" || lower.starts_with(".env."))
        && !lower.ends_with(".example")
        && !lower.ends_with(".sample")
        && !lower.ends_with(".template");
    is_env_file
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || matches!(
        lower.as_str(),
        ".git"
            | ".cindx"
            | "target"
            | "node_modules"
            | "dist"
            | ".ds_store"
            | "cargo.lock"
            | "package-lock.json"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "id_rsa"
            | "id_ed25519"
        )
}

fn should_skip_workspace_entry(workspace_root: &Path, current: &Path, name: &str) -> bool {
    if should_skip_path(name) {
        return true;
    }
    let is_home_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .is_some_and(|home| home == workspace_root);
    current == workspace_root
        && is_home_root
        && matches!(
            name.to_ascii_lowercase().as_str(),
            "library"
                | "pictures"
                | "movies"
                | "music"
                | "applications"
                | ".trash"
                | ".cargo"
                | ".rustup"
                | ".npm"
                | ".bun"
                | ".codex"
                | ".cache"
                | ".local"
        )
}

fn is_probably_binary_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".icns", ".ico", ".pdf", ".zip", ".gz",
        ".tar", ".sqlite", ".sqlite3", ".db", ".app", ".dylib", ".so", ".a",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn embed_text(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0; EMBEDDING_DIMS];
    let counts = token_counts(text);
    for (token, count) in counts {
        let index = stable_hash(token.as_bytes()) as usize % EMBEDDING_DIMS;
        vector[index] += (count as f32).sqrt();
    }
    normalize_vector(&mut vector);
    vector
}

fn token_counts(text: &str) -> BTreeMap<String, usize> {
    let mut tokens = BTreeMap::new();
    let mut current = String::new();

    for character in text.chars() {
        if character.is_alphanumeric() || character == '_' {
            for lower in character.to_lowercase() {
                current.push(lower);
            }
        } else if !current.is_empty() {
            *tokens.entry(std::mem::take(&mut current)).or_insert(0) += 1;
        }
    }
    if !current.is_empty() {
        *tokens.entry(current).or_insert(0) += 1;
    }

    tokens
}

fn normalize_vector(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let dot = left
        .iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }

    (dot / (left_norm * right_norm)).max(0.0)
}

fn lexical_overlap(query: &BTreeMap<String, usize>, chunk: &BTreeMap<String, usize>) -> f32 {
    if query.is_empty() {
        return 0.0;
    }

    let hits = query.keys().filter(|token| chunk.contains_key(*token)).count();
    hits as f32 / query.len() as f32
}

fn empty_index() -> RagIndex {
    RagIndex {
        chunks: Vec::new(),
        stats: RagIndexStats {
            files_indexed: 0,
            chunks_indexed: 0,
            indexed_at_ms: 0,
        },
    }
}

fn save_index(path: &Path, index: &RagIndex) -> Result<(), RagError> {
    let mut rows = Vec::new();
    rows.push(format!(
        "stats\t{}\t{}\t{}",
        index.stats.files_indexed, index.stats.chunks_indexed, index.stats.indexed_at_ms
    ));
    for chunk in &index.chunks {
        rows.push(format!(
            "chunk\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            chunk.id,
            hex_encode(chunk.path.as_bytes()),
            chunk.file_hash,
            chunk.modified_time_ms,
            chunk.start_line,
            chunk.end_line,
            chunk.indexed_at_ms,
            hex_encode(chunk.embedding_provider.as_bytes()),
            hex_encode(chunk.embedding_model.as_bytes()),
            chunk.embedding_dimensions,
            encode_embedding(&chunk.embedding),
            hex_encode(chunk.text.as_bytes())
        ));
    }
    fs::write(path, rows.join("\n"))
        .map_err(|error| RagError::new(format!("failed to save RAG index: {error}")))
}

fn load_index(path: &Path) -> Result<RagIndex, RagError> {
    let text = fs::read_to_string(path)
        .map_err(|error| RagError::new(format!("failed to read RAG index: {error}")))?;
    let mut index = empty_index();
    for line in text.lines() {
        let parts = line.split('\t').collect::<Vec<_>>();
        match parts.as_slice() {
            ["stats", files, chunks, indexed_at] => {
                index.stats = RagIndexStats {
                    files_indexed: files.parse().unwrap_or(0),
                    chunks_indexed: chunks.parse().unwrap_or(0),
                    indexed_at_ms: indexed_at.parse().unwrap_or(0),
                };
            }
            [
                "chunk",
                id,
                path,
                file_hash,
                modified_time_ms,
                start_line,
                end_line,
                indexed_at_ms,
                embedding_provider,
                embedding_model,
                embedding_dimensions,
                embedding,
                text,
            ] => {
                index.chunks.push(RagChunk {
                    id: (*id).to_string(),
                    path: String::from_utf8(hex_decode(path)?)
                        .map_err(|error| RagError::new(error.to_string()))?,
                    file_hash: (*file_hash).to_string(),
                    modified_time_ms: modified_time_ms.parse().unwrap_or(0),
                    start_line: start_line.parse().unwrap_or(0),
                    end_line: end_line.parse().unwrap_or(0),
                    indexed_at_ms: indexed_at_ms.parse().unwrap_or(0),
                    embedding_provider: String::from_utf8(hex_decode(embedding_provider)?)
                        .map_err(|error| RagError::new(error.to_string()))?,
                    embedding_model: String::from_utf8(hex_decode(embedding_model)?)
                        .map_err(|error| RagError::new(error.to_string()))?,
                    embedding_dimensions: embedding_dimensions.parse().unwrap_or(0),
                    embedding: decode_embedding(embedding),
                    text: String::from_utf8(hex_decode(text)?)
                        .map_err(|error| RagError::new(error.to_string()))?,
                });
            }
            [
                "chunk",
                id,
                path,
                file_hash,
                modified_time_ms,
                start_line,
                end_line,
                indexed_at_ms,
                embedding,
                text,
            ] => {
                let embedding = decode_embedding(embedding);
                index.chunks.push(RagChunk {
                    id: (*id).to_string(),
                    path: String::from_utf8(hex_decode(path)?)
                        .map_err(|error| RagError::new(error.to_string()))?,
                    file_hash: (*file_hash).to_string(),
                    modified_time_ms: modified_time_ms.parse().unwrap_or(0),
                    start_line: start_line.parse().unwrap_or(0),
                    end_line: end_line.parse().unwrap_or(0),
                    indexed_at_ms: indexed_at_ms.parse().unwrap_or(0),
                    embedding_provider: "local".to_string(),
                    embedding_model: format!("local-hash-{EMBEDDING_DIMS}"),
                    embedding_dimensions: embedding.len(),
                    embedding,
                    text: String::from_utf8(hex_decode(text)?)
                        .map_err(|error| RagError::new(error.to_string()))?,
                });
            }
            _ => {}
        }
    }
    if index.stats.chunks_indexed == 0 {
        index.stats.chunks_indexed = index.chunks.len();
    }
    Ok(index)
}

fn encode_embedding(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format!("{value:.6}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_embedding(value: &str) -> Vec<f32> {
    value
        .split(',')
        .filter_map(|part| part.parse::<f32>().ok())
        .collect::<Vec<_>>()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(value: &str) -> Result<Vec<u8>, RagError> {
    if value.len() % 2 != 0 {
        return Err(RagError::new("hex value has odd length"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks_exact(2) {
        bytes.push((hex_value(chunk[0])? << 4) | hex_value(chunk[1])?);
    }
    Ok(bytes)
}

fn hex_value(value: u8) -> Result<u8, RagError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(RagError::new("invalid hex digit")),
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other if other.is_control() => escaped.push_str(&format!("\\u{:04x}", other as u32)),
            other => escaped.push(other),
        }
    }
    escaped
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    format!("{:016x}", stable_hash(bytes))
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

fn system_time_millis(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct FakeEmbedder {
        vectors: Vec<Vec<f32>>,
    }

    impl RagEmbedder for FakeEmbedder {
        fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            if self.vectors.len() != texts.len() {
                return Err(RagError::new("fake vector count mismatch"));
            }

            Ok(EmbeddingBatch {
                provider: "test-provider".to_string(),
                model: "test-embedding".to_string(),
                vectors: self.vectors.clone(),
            })
        }
    }

    fn temp_workspace() -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cindx-rag-test-{}-{}",
            stable_hash_hex(format!("{:?}", SystemTime::now()).as_bytes()),
            id
        ));
        fs::create_dir_all(&path).expect("workspace should be created");
        path
    }

    #[test]
    fn indexes_workspace_files_with_line_provenance() {
        let root = temp_workspace();
        fs::write(
            root.join("notes.md"),
            "Cindx uses SQLite events.\nRAG keeps source provenance.\n",
        )
        .expect("file should write");

        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        assert_eq!(index.stats.files_indexed, 1);
        assert_eq!(index.stats.chunks_indexed, 1);
        assert_eq!(index.chunks[0].path, "notes.md");
        assert_eq!(index.chunks[0].start_line, 1);
        assert_eq!(index.chunks[0].end_line, 2);
        assert_eq!(index.chunks[0].embedding_provider, "local");
        assert_eq!(index.chunks[0].embedding_dimensions, EMBEDDING_DIMS);
        assert!(!index.chunks[0].file_hash.is_empty());
    }

    #[test]
    fn skips_local_credentials_when_indexing_workspace() {
        let root = temp_workspace();
        fs::write(root.join("notes.md"), "safe project notes").expect("notes should write");
        fs::write(root.join(".env"), "API_KEY=secret-value").expect("env should write");
        fs::write(root.join("identity.pem"), "private-key-data").expect("key should write");

        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        assert_eq!(index.stats.files_indexed, 1);
        assert_eq!(index.chunks.len(), 1);
        assert_eq!(index.chunks[0].path, "notes.md");
    }

    #[test]
    fn searches_index_by_semantic_and_lexical_score() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "permission audit events and model traces")
            .expect("file should write");
        fs::write(root.join("b.md"), "recipe ingredients and cooking notes")
            .expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        let results = search_chunks(&index.chunks, "model audit trail", 3);

        assert!(!results.is_empty());
        assert_eq!(results[0].chunk.path, "a.md");
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn persists_and_loads_file_adapter_index() {
        let root = temp_workspace();
        let index_path = root.join(".cindx").join("rag-index.tsv");
        fs::write(root.join("readme.md"), "workspace retrieval with sources")
            .expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let mut adapter = FileRagAdapter::open(&index_path).expect("adapter should open");
        adapter.replace_all(index).expect("index should save");

        let loaded = FileRagAdapter::open(&index_path).expect("adapter should reload");
        let results = loaded.search("retrieval sources", 2).expect("search should work");

        assert_eq!(loaded.stats().chunks_indexed, 1);
        assert_eq!(results[0].chunk.path, "readme.md");
        assert_eq!(results[0].chunk.embedding_model, format!("local-hash-{EMBEDDING_DIMS}"));
    }

    #[test]
    fn indexes_workspace_with_external_embeddings() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "cloud embedding vector alpha")
            .expect("file should write");
        let mut embedder = FakeEmbedder {
            vectors: vec![vec![0.9, 0.1, 0.0]],
        };

        let index = index_workspace_with_embedder(&root, IndexOptions::default(), &mut embedder)
            .expect("index should build with external embeddings");

        assert_eq!(index.chunks.len(), 1);
        assert_eq!(index.chunks[0].embedding, vec![0.9, 0.1, 0.0]);
        assert_eq!(index.chunks[0].embedding_provider, "test-provider");
        assert_eq!(index.chunks[0].embedding_model, "test-embedding");
        assert_eq!(index.chunks[0].embedding_dimensions, 3);
    }

    #[test]
    fn searches_with_external_query_embedding() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "alpha").expect("file should write");
        fs::write(root.join("b.md"), "beta").expect("file should write");
        let mut embedder = FakeEmbedder {
            vectors: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        };
        let index = index_workspace_with_embedder(&root, IndexOptions::default(), &mut embedder)
            .expect("index should build");

        let results = search_chunks_with_embedding(&index.chunks, "needle", &[0.0, 1.0], 2);

        assert!(!results.is_empty());
        assert_eq!(results[0].chunk.path, "b.md");
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn semantic_search_rejects_mismatched_vector_spaces() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "alpha").expect("file should write");
        let mut embedder = FakeEmbedder {
            vectors: vec![vec![1.0, 0.0, 0.0]],
        };
        let index = index_workspace_with_embedder(&root, IndexOptions::default(), &mut embedder)
            .expect("index should build");

        let results = search_chunks_semantic(&index.chunks, &[1.0, 0.0], 5);

        assert!(results.is_empty());
    }

    #[test]
    fn literal_search_finds_exact_text_without_vector_match() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "unique-literal-needle and details")
            .expect("file should write");
        fs::write(root.join("b.md"), "unrelated text").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        let results = search_chunks_literal(&index.chunks, "unique-literal-needle", 5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk.path, "a.md");
        assert!(results[0].score >= 1.0);
    }

    #[test]
    fn indexing_respects_max_files() {
        let root = temp_workspace();
        for name in ["a.md", "b.md", "c.md"] {
            fs::write(root.join(name), format!("content for {name}"))
                .expect("file should write");
        }

        let index = index_workspace(
            &root,
            IndexOptions {
                max_files: 1,
                ..IndexOptions::default()
            },
        )
        .expect("index should build");

        assert_eq!(index.stats.files_indexed, 1);
        assert_eq!(index.chunks.len(), 1);
    }

    #[test]
    fn projects_lancedb_ready_records() {
        let root = temp_workspace();
        fs::write(root.join("readme.md"), "lancedb record schema").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        let records = lancedb_records(&index);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, "readme.md");
        assert_eq!(records[0].embedding_dimensions, records[0].vector.len());
        assert!(!records[0].text.is_empty());
    }

    #[test]
    fn exports_lancedb_records_jsonl() {
        let root = temp_workspace();
        fs::write(root.join("readme.md"), "lancedb jsonl export").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let output_path = root.join(".cindx").join("lancedb-records.jsonl");

        let rows = export_lancedb_records_jsonl(&index, &output_path).expect("export should write");
        let output = fs::read_to_string(output_path).expect("export should read");

        assert_eq!(rows, 1);
        assert!(output.contains("\"path\":\"readme.md\""));
        assert!(output.contains("\"vector\":["));
        assert!(output.contains("\"embedding_provider\":\"local\""));
    }

    #[test]
    fn grounded_prompt_cites_source_ranges() {
        let chunk = RagChunk {
            id: "1".to_string(),
            path: "docs/a.md".to_string(),
            file_hash: "abc".to_string(),
            modified_time_ms: 0,
            start_line: 3,
            end_line: 8,
            indexed_at_ms: 0,
            text: "important source".to_string(),
            embedding: embed_text("important source"),
            embedding_provider: "local".to_string(),
            embedding_model: format!("local-hash-{EMBEDDING_DIMS}"),
            embedding_dimensions: EMBEDDING_DIMS,
        };

        let prompt = build_grounded_answer_prompt(
            "What matters?",
            &[RagSearchResult { chunk, score: 0.9 }],
        );

        assert!(prompt.contains("[docs/a.md:3-8"));
        assert!(prompt.contains("What matters?"));
    }
}
