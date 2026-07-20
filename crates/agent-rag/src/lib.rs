use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "lancedb-store")]
use arrow_array::{
    types::Float32Type, Array, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch,
    RecordBatchIterator, StringArray, UInt64Array,
};
#[cfg(feature = "lancedb-store")]
use arrow_schema::{DataType, Field, Schema};
#[cfg(feature = "lancedb-store")]
use futures::TryStreamExt;
#[cfg(feature = "lancedb-store")]
use lancedb::{
    database::CreateTableMode,
    index::Index,
    query::{ExecutableQuery, QueryBase},
    DistanceType,
};
#[cfg(feature = "lancedb-store")]
use std::sync::OnceLock;

const EMBEDDING_DIMS: usize = 64;
const DEFAULT_MAX_FILE_BYTES: u64 = 512 * 1024;
const DEFAULT_MAX_FILES: usize = 10_000;
const DEFAULT_CHUNK_LINES: usize = 80;
const DEFAULT_CHUNK_OVERLAP: usize = 8;
const DEFAULT_EMBEDDING_BATCH_SIZE: usize = 20;
const FILE_SEARCH_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const FILE_SEARCH_MAX_FILES: usize = 20_000;
const FILE_SEARCH_CONTEXT_LINES: usize = 2;
#[cfg(feature = "lancedb-store")]
const LANCEDB_WORKSPACE_TABLE: &str = "workspace_chunks";
#[cfg(feature = "lancedb-store")]
const LANCEDB_ANN_MIN_ROWS: usize = 256;

#[cfg(feature = "lancedb-store")]
static LANCEDB_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub const RAG_INDEX_CANCELLED: &str = "RAG indexing cancelled";

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

#[cfg(feature = "lancedb-store")]
pub fn lancedb_index_exists(database_path: impl AsRef<Path>) -> bool {
    database_path
        .as_ref()
        .join(format!("{LANCEDB_WORKSPACE_TABLE}.lance"))
        .exists()
}

#[cfg(feature = "lancedb-store")]
pub fn replace_lancedb_index(
    database_path: impl AsRef<Path>,
    index: &RagIndex,
) -> Result<usize, RagError> {
    let database_path = database_path.as_ref();
    if index.chunks.is_empty() {
        if database_path.exists() {
            fs::remove_dir_all(database_path).map_err(|error| {
                RagError::new(format!("failed to clear empty LanceDB index: {error}"))
            })?;
        }
        return Ok(0);
    }
    let dimensions = index.chunks[0].embedding_dimensions;
    if dimensions == 0
        || index.chunks.iter().any(|chunk| {
            chunk.embedding_dimensions != dimensions || chunk.embedding.len() != dimensions
        })
    {
        return Err(RagError::new(
            "LanceDB index requires one non-empty embedding dimension",
        ));
    }
    let parent = database_path
        .parent()
        .ok_or_else(|| RagError::new("LanceDB path has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| RagError::new(format!("failed to create LanceDB parent: {error}")))?;
    let staging = parent.join(format!(
        ".lancedb-staging-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| RagError::new(format!("failed to reset LanceDB staging: {error}")))?;
    }
    let batch = lancedb_record_batch(index, dimensions)?;
    let row_count = index.chunks.len();
    lancedb_runtime()?.block_on(async {
        let database = lancedb::connect(&staging.to_string_lossy())
            .execute()
            .await
            .map_err(|error| RagError::new(format!("failed to open LanceDB staging: {error}")))?;
        let schema = batch.schema();
        let reader: Box<dyn arrow_array::RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));
        let table = database
            .create_table(LANCEDB_WORKSPACE_TABLE, reader)
            .mode(CreateTableMode::Overwrite)
            .execute()
            .await
            .map_err(|error| RagError::new(format!("failed to replace LanceDB table: {error}")))?;
        if row_count >= LANCEDB_ANN_MIN_ROWS {
            table
                .create_index(&["vector"], Index::Auto)
                .execute()
                .await
                .map_err(|error| RagError::new(format!("failed to build LanceDB ANN index: {error}")))?;
        }
        Ok::<(), RagError>(())
    })?;
    swap_lancedb_directory(database_path, &staging)?;
    Ok(row_count)
}

#[cfg(feature = "lancedb-store")]
pub fn search_lancedb_index(
    database_path: impl AsRef<Path>,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<RagSearchResult>, RagError> {
    if query_embedding.is_empty() {
        return Err(RagError::new("LanceDB query embedding is empty"));
    }
    let database_path = database_path.as_ref();
    if !lancedb_index_exists(database_path) {
        return Err(RagError::new("LanceDB workspace index is missing"));
    }
    lancedb_runtime()?.block_on(async {
        let database = lancedb::connect(&database_path.to_string_lossy())
            .execute()
            .await
            .map_err(|error| RagError::new(format!("failed to open LanceDB: {error}")))?;
        let table = database
            .open_table(LANCEDB_WORKSPACE_TABLE)
            .execute()
            .await
            .map_err(|error| RagError::new(format!("failed to open LanceDB table: {error}")))?;
        let batches = table
            .query()
            .nearest_to(query_embedding)
            .map_err(|error| RagError::new(format!("invalid LanceDB vector query: {error}")))?
            .distance_type(DistanceType::Cosine)
            .limit(limit.max(1).min(50))
            .execute()
            .await
            .map_err(|error| RagError::new(format!("LanceDB search failed: {error}")))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| RagError::new(format!("failed to collect LanceDB rows: {error}")))?;
        lancedb_results_from_batches(&batches)
    })
}

#[cfg(feature = "lancedb-store")]
fn lancedb_runtime() -> Result<&'static tokio::runtime::Runtime, RagError> {
    if let Some(runtime) = LANCEDB_RUNTIME.get() {
        return Ok(runtime);
    }
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| RagError::new(format!("failed to start LanceDB runtime: {error}")))?;
    let _ = LANCEDB_RUNTIME.set(runtime);
    LANCEDB_RUNTIME
        .get()
        .ok_or_else(|| RagError::new("failed to initialize LanceDB runtime"))
}

#[cfg(feature = "lancedb-store")]
fn lancedb_record_batch(index: &RagIndex, dimensions: usize) -> Result<RecordBatch, RagError> {
    let vector = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        index.chunks.iter().map(|chunk| {
            Some(
                chunk
                    .embedding
                    .iter()
                    .copied()
                    .map(Some)
                    .collect::<Vec<_>>(),
            )
        }),
        i32::try_from(dimensions)
            .map_err(|_| RagError::new("LanceDB vector dimensions exceed i32"))?,
    );
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("file_hash", DataType::Utf8, false),
        Field::new("modified_time_ms", DataType::UInt64, false),
        Field::new("start_line", DataType::UInt64, false),
        Field::new("end_line", DataType::UInt64, false),
        Field::new("indexed_at_ms", DataType::UInt64, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("embedding_provider", DataType::Utf8, false),
        Field::new("embedding_model", DataType::Utf8, false),
        Field::new("embedding_dimensions", DataType::UInt64, false),
        Field::new("vector", vector.data_type().clone(), false),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.path.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.file_hash.as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.modified_time_ms),
        )),
        Arc::new(UInt64Array::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.start_line),
        )),
        Arc::new(UInt64Array::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.end_line),
        )),
        Arc::new(UInt64Array::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.indexed_at_ms),
        )),
        Arc::new(StringArray::from_iter_values(
            index.chunks.iter().map(|chunk| chunk.text.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            index
                .chunks
                .iter()
                .map(|chunk| chunk.embedding_provider.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            index
                .chunks
                .iter()
                .map(|chunk| chunk.embedding_model.as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            index
                .chunks
                .iter()
                .map(|chunk| chunk.embedding_dimensions as u64),
        )),
        Arc::new(vector),
    ];
    RecordBatch::try_new(schema, columns)
        .map_err(|error| RagError::new(format!("failed to build LanceDB record batch: {error}")))
}

#[cfg(feature = "lancedb-store")]
fn lancedb_results_from_batches(batches: &[RecordBatch]) -> Result<Vec<RagSearchResult>, RagError> {
    let mut results = Vec::new();
    for batch in batches {
        let ids = lancedb_string_column(batch, "id")?;
        let paths = lancedb_string_column(batch, "path")?;
        let file_hashes = lancedb_string_column(batch, "file_hash")?;
        let modified_times = lancedb_u64_column(batch, "modified_time_ms")?;
        let start_lines = lancedb_u64_column(batch, "start_line")?;
        let end_lines = lancedb_u64_column(batch, "end_line")?;
        let indexed_times = lancedb_u64_column(batch, "indexed_at_ms")?;
        let texts = lancedb_string_column(batch, "text")?;
        let providers = lancedb_string_column(batch, "embedding_provider")?;
        let models = lancedb_string_column(batch, "embedding_model")?;
        let dimensions = lancedb_u64_column(batch, "embedding_dimensions")?;
        let vectors = batch
            .column_by_name("vector")
            .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
            .ok_or_else(|| RagError::new("LanceDB result is missing vector"))?;
        let distances = batch
            .column_by_name("_distance")
            .and_then(|column| column.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| RagError::new("LanceDB result is missing cosine distance"))?;
        for row in 0..batch.num_rows() {
            let vector = vectors.value(row);
            let vector = vector
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| RagError::new("LanceDB vector row is not Float32"))?
                .values()
                .to_vec();
            results.push(RagSearchResult {
                chunk: RagChunk {
                    id: ids.value(row).to_string(),
                    path: paths.value(row).to_string(),
                    file_hash: file_hashes.value(row).to_string(),
                    modified_time_ms: modified_times.value(row),
                    start_line: start_lines.value(row),
                    end_line: end_lines.value(row),
                    indexed_at_ms: indexed_times.value(row),
                    text: texts.value(row).to_string(),
                    embedding: vector,
                    embedding_provider: providers.value(row).to_string(),
                    embedding_model: models.value(row).to_string(),
                    embedding_dimensions: dimensions.value(row) as usize,
                },
                score: (1.0 - distances.value(row)).clamp(0.0, 1.0),
            });
        }
    }
    Ok(results)
}

#[cfg(feature = "lancedb-store")]
fn lancedb_string_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a StringArray, RagError> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| RagError::new(format!("LanceDB result is missing {name}")))
}

#[cfg(feature = "lancedb-store")]
fn lancedb_u64_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a UInt64Array, RagError> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
        .ok_or_else(|| RagError::new(format!("LanceDB result is missing {name}")))
}

#[cfg(feature = "lancedb-store")]
fn swap_lancedb_directory(database_path: &Path, staging: &Path) -> Result<(), RagError> {
    let parent = database_path
        .parent()
        .ok_or_else(|| RagError::new("LanceDB path has no parent directory"))?;
    let backup = parent.join(format!(
        ".lancedb-backup-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let had_existing = database_path.exists();
    if had_existing {
        fs::rename(database_path, &backup)
            .map_err(|error| RagError::new(format!("failed to stage old LanceDB: {error}")))?;
    }
    if let Err(error) = fs::rename(staging, database_path) {
        if had_existing {
            let _ = fs::rename(&backup, database_path);
        }
        return Err(RagError::new(format!(
            "failed to activate LanceDB index: {error}"
        )));
    }
    if had_existing {
        fs::remove_dir_all(&backup)
            .map_err(|error| RagError::new(format!("failed to remove old LanceDB: {error}")))?;
    }
    Ok(())
}

pub trait RagAdapter {
    fn replace_all(&mut self, index: RagIndex) -> Result<RagIndexStats, RagError>;

    fn search(&self, query: &str, limit: usize) -> Result<Vec<RagSearchResult>, RagError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRagAdapter {
    path: PathBuf,
    index: Arc<RagIndex>,
}

impl FileRagAdapter {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, RagError> {
        let path = path.into();
        let index = if path.exists() {
            load_index(&path)?
        } else {
            empty_index()
        };

        Ok(Self {
            path,
            index: Arc::new(index),
        })
    }

    pub fn stats(&self) -> &RagIndexStats {
        &self.index.stats
    }

    pub fn chunks(&self) -> &[RagChunk] {
        &self.index.chunks
    }

    pub fn index(&self) -> &RagIndex {
        &self.index
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
        self.index = Arc::new(index);

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
    index_workspace_cancellable(workspace_root, options, || false)
}

pub fn index_workspace_cancellable(
    workspace_root: impl AsRef<Path>,
    options: IndexOptions,
    mut should_cancel: impl FnMut() -> bool,
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
        &mut should_cancel,
    )?;

    let stats = RagIndexStats {
        files_indexed,
        chunks_indexed: chunks.len(),
        indexed_at_ms,
    };

    Ok(RagIndex { chunks, stats })
}

pub fn workspace_index_is_fresh(
    workspace_root: impl AsRef<Path>,
    chunks: &[RagChunk],
    options: IndexOptions,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<bool, RagError> {
    if chunks.is_empty() {
        return Ok(false);
    }
    let indexed_files = chunks
        .iter()
        .map(|chunk| (chunk.path.clone(), chunk.modified_time_ms))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut files_indexed = 0usize;
    let fresh = inspect_workspace_freshness(
        workspace_root.as_ref(),
        workspace_root.as_ref(),
        &options,
        &indexed_files,
        &mut seen,
        &mut files_indexed,
        &mut should_cancel,
    )?;
    Ok(fresh && seen.len() == indexed_files.len())
}

pub fn index_workspace_with_embedder(
    workspace_root: impl AsRef<Path>,
    options: IndexOptions,
    embedder: &mut dyn RagEmbedder,
) -> Result<RagIndex, RagError> {
    index_workspace_with_embedder_cancellable(workspace_root, options, embedder, || false)
}

pub fn index_workspace_with_embedder_cancellable(
    workspace_root: impl AsRef<Path>,
    options: IndexOptions,
    embedder: &mut dyn RagEmbedder,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<RagIndex, RagError> {
    let mut index = index_workspace_cancellable(workspace_root, options, &mut should_cancel)?;
    apply_embeddings_to_index_cancellable(
        &mut index,
        embedder,
        &mut should_cancel,
    )?;
    Ok(index)
}

pub fn apply_embeddings_to_index_cancellable(
    index: &mut RagIndex,
    embedder: &mut dyn RagEmbedder,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<(), RagError> {
    let texts = index
        .chunks
        .iter()
        .map(|chunk| chunk.text.clone())
        .collect::<Vec<_>>();
    if texts.is_empty() {
        return Ok(());
    }

    let mut provider = None;
    let mut model = None;
    let mut vectors = Vec::with_capacity(texts.len());
    for texts_batch in texts.chunks(DEFAULT_EMBEDDING_BATCH_SIZE) {
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        let batch = embedder.embed_texts(texts_batch)?;
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        if batch.vectors.len() != texts_batch.len() {
            return Err(RagError::new(format!(
                "embedding count mismatch: got {}, expected {}",
                batch.vectors.len(),
                texts_batch.len()
            )));
        }
        if provider.as_ref().is_some_and(|value| value != &batch.provider)
            || model.as_ref().is_some_and(|value| value != &batch.model)
        {
            return Err(RagError::new(
                "embedding provider or model changed between batches",
            ));
        }
        provider.get_or_insert(batch.provider);
        model.get_or_insert(batch.model);
        vectors.extend(batch.vectors);
    }

    let provider = provider.unwrap_or_default();
    let model = model.unwrap_or_default();
    for (chunk, vector) in index.chunks.iter_mut().zip(vectors) {
        if vector.is_empty() {
            return Err(RagError::new("embedding vector was empty"));
        }
        chunk.embedding_dimensions = vector.len();
        chunk.embedding = vector;
        chunk.embedding_provider = provider.clone();
        chunk.embedding_model = model.clone();
    }
    Ok(())
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
    let limit = limit.clamp(1, 50);
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
            let normalized_path = chunk.path.to_lowercase();
            let exact_matches = normalized_text.matches(&normalized_query).count();
            let path_exact = normalized_path.contains(&normalized_query);
            let text_score = lexical_overlap(&query_tokens, &token_counts(&normalized_text));
            let path_score = lexical_overlap(&query_tokens, &token_counts(&normalized_path));
            let score = if path_exact {
                1.25 + (exact_matches.min(8) as f32 * 0.04)
            } else if exact_matches > 0 {
                1.0 + (exact_matches.min(8) as f32 * 0.05)
            } else {
                (text_score * 0.75) + (path_score * 0.45)
            };
            (score > 0.0).then_some(RagSearchResult { chunk, score })
        })
        .collect::<Vec<_>>();
    sort_and_truncate_results(&mut results, limit.clamp(1, 50));
    results
}

pub fn search_workspace_files_cancellable(
    workspace_root: impl AsRef<Path>,
    query: &str,
    limit: usize,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<Vec<RagSearchResult>, RagError> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }
    let mut results = Vec::new();
    let mut files_scanned = 0usize;
    search_workspace_path(
        workspace_root.as_ref(),
        workspace_root.as_ref(),
        &normalized_query,
        &token_counts(&normalized_query),
        &mut results,
        &mut files_scanned,
        &mut should_cancel,
    )?;
    sort_and_truncate_results(&mut results, limit.clamp(1, 50));
    Ok(results)
}

pub fn search_chunks_with_embedding(
    chunks: &[RagChunk],
    query: &str,
    query_embedding: &[f32],
    limit: usize,
) -> Vec<RagSearchResult> {
    let limit = limit.clamp(1, 50);
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

#[allow(clippy::too_many_arguments)]
fn search_workspace_path(
    workspace_root: &Path,
    current: &Path,
    normalized_query: &str,
    query_tokens: &BTreeMap<String, usize>,
    results: &mut Vec<RagSearchResult>,
    files_scanned: &mut usize,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<(), RagError> {
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    if *files_scanned >= FILE_SEARCH_MAX_FILES {
        return Ok(());
    }
    let metadata = match fs::metadata(current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(error) => return Err(RagError::new(format!("failed to stat search path: {error}"))),
    };
    if metadata.is_file() {
        search_workspace_file(
            workspace_root,
            current,
            &metadata,
            normalized_query,
            query_tokens,
            results,
            files_scanned,
        )?;
        return Ok(());
    }

    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(error) => return Err(RagError::new(format!("failed to search directory: {error}"))),
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        if *files_scanned >= FILE_SEARCH_MAX_FILES {
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
            search_workspace_path(
                workspace_root,
                &path,
                normalized_query,
                query_tokens,
                results,
                files_scanned,
                should_cancel,
            )?;
        } else if metadata.is_file() {
            search_workspace_file(
                workspace_root,
                &path,
                &metadata,
                normalized_query,
                query_tokens,
                results,
                files_scanned,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn search_workspace_file(
    workspace_root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
    normalized_query: &str,
    query_tokens: &BTreeMap<String, usize>,
    results: &mut Vec<RagSearchResult>,
    files_scanned: &mut usize,
) -> Result<(), RagError> {
    if *files_scanned >= FILE_SEARCH_MAX_FILES
        || metadata.len() == 0
        || metadata.len() > FILE_SEARCH_MAX_FILE_BYTES
    {
        return Ok(());
    }
    let relative = relative_workspace_path(workspace_root, path)?;
    if is_probably_binary_path(&relative) {
        return Ok(());
    }
    *files_scanned += 1;
    let Ok(file) = fs::File::open(path) else {
        return Ok(());
    };
    let mut reader = BufReader::new(file.take(FILE_SEARCH_MAX_FILE_BYTES));
    let mut lines = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(read) = reader.read_line(&mut line) else {
            return Ok(());
        };
        if read == 0 {
            break;
        }
        lines.push(line.trim_end_matches(['\r', '\n']).to_string());
    }
    if lines.is_empty() {
        return Ok(());
    }
    if lines.iter().any(|line| line.contains('\0')) {
        return Ok(());
    }

    let normalized_path = relative.to_lowercase();
    let path_exact = normalized_path.contains(normalized_query);
    let path_overlap = lexical_overlap(query_tokens, &token_counts(&normalized_path));
    let mut exact_matches = 0usize;
    let mut best_line = None;
    let mut best_line_overlap = 0.0f32;
    for (index, source_line) in lines.iter().enumerate() {
        let normalized_line = source_line.to_lowercase();
        let matches = normalized_line.matches(normalized_query).count();
        exact_matches += matches;
        let overlap = lexical_overlap(query_tokens, &token_counts(&normalized_line));
        if matches > 0 || overlap > best_line_overlap {
            best_line = Some(index);
            best_line_overlap = if matches > 0 { 1.0 } else { overlap };
        }
    }
    let score = if path_exact {
        1.35 + (exact_matches.min(8) as f32 * 0.04)
    } else if exact_matches > 0 {
        1.0 + (exact_matches.min(8) as f32 * 0.05) + (path_overlap * 0.2)
    } else {
        (best_line_overlap * 0.8) + (path_overlap * 0.55)
    };
    if score <= 0.0 {
        return Ok(());
    }

    let focus = best_line.unwrap_or(0);
    let start = focus.saturating_sub(FILE_SEARCH_CONTEXT_LINES);
    let end = (focus + FILE_SEARCH_CONTEXT_LINES + 1).min(lines.len());
    let text = lines[start..end]
        .iter()
        .map(|line| truncate_search_line(line, 512))
        .collect::<Vec<_>>()
        .join("\n");
    let file_hash = stable_hash_hex(lines.join("\n").as_bytes());
    let modified_time_ms = metadata
        .modified()
        .ok()
        .and_then(system_time_millis)
        .unwrap_or(0);
    results.push(RagSearchResult {
        chunk: RagChunk {
            id: stable_hash_hex(
                format!("file-search:{relative}:{}:{}:{file_hash}", start + 1, end).as_bytes(),
            ),
            path: relative,
            file_hash,
            modified_time_ms,
            start_line: start as u64 + 1,
            end_line: end as u64,
            indexed_at_ms: current_time_millis(),
            embedding: embed_text(&text),
            embedding_provider: "local".to_string(),
            embedding_model: format!("local-hash-{EMBEDDING_DIMS}"),
            embedding_dimensions: EMBEDDING_DIMS,
            text,
        },
        score,
    });
    Ok(())
}

fn truncate_search_line(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let mut truncated = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() {
        truncated.push_str("...");
    }
    truncated
}

fn collect_chunks(
    workspace_root: &Path,
    current: &Path,
    options: &IndexOptions,
    indexed_at_ms: u64,
    chunks: &mut Vec<RagChunk>,
    files_indexed: &mut usize,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<(), RagError> {
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    if *files_indexed >= options.max_files.max(1) {
        return Ok(());
    }
    let metadata = match fs::metadata(current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(error) => return Err(RagError::new(format!("failed to stat path: {error}"))),
    };
    if metadata.is_file() {
        index_file(
            workspace_root,
            current,
            &metadata,
            options,
            indexed_at_ms,
            chunks,
            files_indexed,
            should_cancel,
        )?;
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
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
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
            collect_chunks(
                workspace_root,
                &path,
                options,
                indexed_at_ms,
                chunks,
                files_indexed,
                should_cancel,
            )?;
        } else if metadata.is_file() {
            index_file(
                workspace_root,
                &path,
                &metadata,
                options,
                indexed_at_ms,
                chunks,
                files_indexed,
                should_cancel,
            )?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn inspect_workspace_freshness(
    workspace_root: &Path,
    current: &Path,
    options: &IndexOptions,
    indexed_files: &BTreeMap<String, u64>,
    seen: &mut BTreeSet<String>,
    files_indexed: &mut usize,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<bool, RagError> {
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    if *files_indexed >= options.max_files.max(1) {
        return Ok(true);
    }
    let metadata = match fs::metadata(current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(true),
        Err(error) => return Err(RagError::new(format!("failed to stat path: {error}"))),
    };
    if metadata.is_file() {
        return inspect_file_freshness(
            workspace_root,
            current,
            &metadata,
            options,
            indexed_files,
            seen,
            files_indexed,
        );
    }

    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(true),
        Err(error) => return Err(RagError::new(format!("failed to read directory: {error}"))),
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
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
        let fresh = if metadata.is_dir() {
            inspect_workspace_freshness(
                workspace_root,
                &path,
                options,
                indexed_files,
                seen,
                files_indexed,
                should_cancel,
            )?
        } else if metadata.is_file() {
            inspect_file_freshness(
                workspace_root,
                &path,
                &metadata,
                options,
                indexed_files,
                seen,
                files_indexed,
            )?
        } else {
            true
        };
        if !fresh {
            return Ok(false);
        }
    }
    Ok(true)
}

fn inspect_file_freshness(
    workspace_root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
    options: &IndexOptions,
    indexed_files: &BTreeMap<String, u64>,
    seen: &mut BTreeSet<String>,
    files_indexed: &mut usize,
) -> Result<bool, RagError> {
    if metadata.len() > options.max_file_bytes {
        return Ok(true);
    }
    let relative = relative_workspace_path(workspace_root, path)?;
    if is_probably_binary_path(&relative) {
        return Ok(true);
    }
    let modified_time_ms = metadata
        .modified()
        .ok()
        .and_then(system_time_millis)
        .unwrap_or(0);
    if let Some(indexed_time) = indexed_files.get(&relative) {
        *files_indexed += 1;
        seen.insert(relative);
        return Ok(*indexed_time == modified_time_ms);
    }

    let Ok(content) = fs::read_to_string(path) else {
        return Ok(true);
    };
    if content.trim().is_empty() {
        return Ok(true);
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn index_file(
    workspace_root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
    options: &IndexOptions,
    indexed_at_ms: u64,
    chunks: &mut Vec<RagChunk>,
    files_indexed: &mut usize,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<(), RagError> {
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
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
    if !value.len().is_multiple_of(2) {
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
    fn workspace_index_freshness_detects_modified_added_and_deleted_files() {
        let root = temp_workspace();
        let notes = root.join("notes.md");
        fs::write(&notes, "original workspace notes").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        assert!(workspace_index_is_fresh(
            &root,
            &index.chunks,
            IndexOptions::default(),
            || false
        )
        .expect("freshness should be checked"));

        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&notes, "modified workspace notes").expect("file should update");
        assert!(!workspace_index_is_fresh(
            &root,
            &index.chunks,
            IndexOptions::default(),
            || false
        )
        .expect("modified file should be detected"));

        let updated = index_workspace(&root, IndexOptions::default()).expect("index should rebuild");
        fs::write(root.join("new.md"), "new knowledge").expect("new file should write");
        assert!(!workspace_index_is_fresh(
            &root,
            &updated.chunks,
            IndexOptions::default(),
            || false
        )
        .expect("new file should be detected"));

        fs::remove_file(root.join("new.md")).expect("new file should remove");
        fs::remove_file(&notes).expect("indexed file should remove");
        assert!(!workspace_index_is_fresh(
            &root,
            &updated.chunks,
            IndexOptions::default(),
            || false
        )
        .expect("deleted file should be detected"));
    }

    #[test]
    fn literal_search_uses_file_paths_as_retrieval_evidence() {
        let root = temp_workspace();
        fs::create_dir_all(root.join("src")).expect("source directory should create");
        fs::write(
            root.join("src/permission_router.rs"),
            "routes requests through the configured policy",
        )
        .expect("source file should write");
        fs::write(root.join("notes.md"), "unrelated project notes").expect("notes should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        let results = search_chunks_literal(&index.chunks, "permission_router", 4);

        assert_eq!(results[0].chunk.path, "src/permission_router.rs");
        assert!(results[0].score > 1.0);
    }

    #[test]
    fn cancellable_index_stops_during_workspace_scan() {
        let root = temp_workspace();
        fs::write(root.join("one.md"), "one").expect("first file should write");
        fs::write(root.join("two.md"), "two").expect("second file should write");
        let mut checks = 0;

        let error = index_workspace_cancellable(&root, IndexOptions::default(), || {
            checks += 1;
            checks >= 3
        })
        .expect_err("index should be cancelled");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
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
    fn cloning_file_adapter_shares_the_immutable_index_snapshot() {
        let root = temp_workspace();
        let index_path = root.join(".cindx").join("rag-index.tsv");
        fs::write(root.join("readme.md"), "shared retrieval snapshot")
            .expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let mut adapter = FileRagAdapter::open(&index_path).expect("adapter should open");
        adapter.replace_all(index).expect("index should save");

        let cloned = adapter.clone();

        assert!(Arc::ptr_eq(&adapter.index, &cloned.index));
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
    fn external_embeddings_are_requested_in_provider_safe_batches() {
        struct RecordingEmbedder {
            batch_sizes: Vec<usize>,
        }

        impl RagEmbedder for RecordingEmbedder {
            fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, RagError> {
                self.batch_sizes.push(texts.len());
                Ok(EmbeddingBatch {
                    provider: "test-provider".to_string(),
                    model: "test-embedding".to_string(),
                    vectors: vec![vec![1.0, 0.0]; texts.len()],
                })
            }
        }

        let root = temp_workspace();
        let content = (0..360)
            .map(|line| format!("knowledge line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(root.join("many-chunks.md"), content).expect("file should write");
        let mut embedder = RecordingEmbedder {
            batch_sizes: Vec::new(),
        };
        let options = IndexOptions {
            chunk_lines: 8,
            chunk_overlap: 0,
            ..IndexOptions::default()
        };

        let index = index_workspace_with_embedder(&root, options, &mut embedder)
            .expect("batched external embeddings should build");

        assert_eq!(index.chunks.len(), 45);
        assert_eq!(embedder.batch_sizes, vec![20, 20, 5]);
        assert!(index
            .chunks
            .iter()
            .all(|chunk| chunk.embedding_model == "test-embedding"));
    }

    #[test]
    fn cancellable_external_index_stops_between_embedding_batches() {
        struct CountingEmbedder {
            calls: usize,
        }

        impl RagEmbedder for CountingEmbedder {
            fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, RagError> {
                self.calls += 1;
                Ok(EmbeddingBatch {
                    provider: "test-provider".to_string(),
                    model: "test-embedding".to_string(),
                    vectors: vec![vec![1.0, 0.0]; texts.len()],
                })
            }
        }

        let root = temp_workspace();
        let content = (0..360)
            .map(|line| format!("knowledge line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(root.join("many-chunks.md"), content).expect("file should write");
        let mut embedder = CountingEmbedder { calls: 0 };
        let options = IndexOptions {
            chunk_lines: 8,
            chunk_overlap: 0,
            ..IndexOptions::default()
        };
        let mut checks = 0usize;

        let error = index_workspace_with_embedder_cancellable(
            &root,
            options,
            &mut embedder,
            || {
                checks += 1;
                checks >= 6
            },
        )
        .expect_err("external indexing should stop when cancelled");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
        assert!(embedder.calls < 3);
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
    fn direct_file_search_finds_content_excluded_from_the_rag_index() {
        let root = temp_workspace();
        fs::write(root.join("indexed.md"), "ordinary indexed content").expect("file should write");
        fs::write(
            root.join("large.log"),
            format!("{}\nunique direct file evidence\n", "x".repeat(600 * 1024)),
        )
        .expect("large file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");

        assert!(index
            .chunks
            .iter()
            .all(|chunk| chunk.path != "large.log"));
        let results = search_workspace_files_cancellable(
            &root,
            "unique direct file evidence",
            4,
            || false,
        )
        .expect("direct file search should succeed");

        assert_eq!(results[0].chunk.path, "large.log");
        assert!(results[0].chunk.text.contains("unique direct file evidence"));
        assert!(results[0].chunk.start_line <= results[0].chunk.end_line);
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

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn persists_and_searches_a_real_lancedb_index() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "permission audit model traces")
            .expect("file should write");
        fs::write(root.join("b.md"), "recipe ingredients cooking notes")
            .expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let database_path = root.join(".cindx").join("lancedb");

        let rows = replace_lancedb_index(&database_path, &index)
            .expect("LanceDB index should persist");
        let results = search_lancedb_index(
            &database_path,
            &local_query_embedding("model audit trail"),
            2,
        )
        .expect("LanceDB search should succeed");

        assert_eq!(rows, 2);
        assert!(lancedb_index_exists(&database_path));
        assert_eq!(results[0].chunk.path, "a.md");
        assert!(results[0].score > 0.0);
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

    #[test]
    #[ignore = "performance diagnostic; run through the quality-gate performance profile"]
    fn synthetic_rag_search_scaling_diagnostic() {
        let chunk_count = 20_000usize;
        let chunks = (0..chunk_count)
            .map(|index| {
                let text = format!(
                    "workspace retrieval chunk {index} with graph memory and file evidence"
                );
                RagChunk {
                    id: format!("chunk-{index}"),
                    path: format!("src/module-{index}.rs"),
                    file_hash: format!("hash-{index}"),
                    modified_time_ms: index as u64,
                    start_line: 1,
                    end_line: 8,
                    indexed_at_ms: 1,
                    embedding: embed_text(&text),
                    embedding_provider: "local".to_string(),
                    embedding_model: format!("local-hash-{EMBEDDING_DIMS}"),
                    embedding_dimensions: EMBEDDING_DIMS,
                    text,
                }
            })
            .collect::<Vec<_>>();
        let estimated_payload_bytes = chunks
            .iter()
            .map(|chunk| {
                chunk.id.len()
                    + chunk.path.len()
                    + chunk.file_hash.len()
                    + chunk.text.len()
                    + chunk.embedding.len() * std::mem::size_of::<f32>()
            })
            .sum::<usize>();
        let query_embedding = local_query_embedding("graph memory file evidence");

        let semantic_started_at = std::time::Instant::now();
        let semantic = search_chunks_semantic(&chunks, &query_embedding, 12);
        let semantic_micros = semantic_started_at.elapsed().as_micros();
        let literal_started_at = std::time::Instant::now();
        let literal = search_chunks_literal(&chunks, "graph memory file evidence", 12);
        let literal_micros = literal_started_at.elapsed().as_micros();

        assert_eq!(semantic.len(), 12);
        assert_eq!(literal.len(), 12);
        println!(
            "{{\"schema\":\"cindx.rag-search-diagnostic.v1\",\"chunks\":{chunk_count},\"dimensions\":{EMBEDDING_DIMS},\"estimated_payload_bytes\":{estimated_payload_bytes},\"semantic_micros\":{semantic_micros},\"literal_micros\":{literal_micros}}}"
        );
    }
}
