use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(feature = "lancedb-store")]
use std::future::Future;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
#[cfg(feature = "lancedb-store")]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

mod retrieval_fusion;

pub use retrieval_fusion::{
    fuse_retrieval_channels, fuse_retrieval_channels_for_query, merge_retrieval_channel,
    retrieval_ranges_overlap, FusedRagSource, RetrievalChannelOutcome, RetrievalFusionResult,
};

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
const EMBEDDING_DIMS: usize = 64;
const DEFAULT_MAX_FILE_BYTES: u64 = 512 * 1024;
const DEFAULT_MAX_FILES: usize = 10_000;
const DEFAULT_CHUNK_LINES: usize = 80;
const DEFAULT_CHUNK_OVERLAP: usize = 8;
const DEFAULT_EMBEDDING_BATCH_SIZE: usize = 20;
const FILE_SEARCH_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const FILE_SEARCH_MAX_FILES: usize = 20_000;
const FILE_SEARCH_CONTEXT_LINES: usize = 2;
const FILE_RAG_STATS_HEADER_MAX_BYTES: u64 = 256;
static STAGING_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static FILE_RAG_PATH_LEASES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<()>>>> = OnceLock::new();
#[cfg(feature = "lancedb-store")]
const LANCEDB_WORKSPACE_TABLE: &str = "workspace_chunks";
#[cfg(feature = "lancedb-store")]
const LANCEDB_ANN_MIN_ROWS: usize = 256;
#[cfg(feature = "lancedb-store")]
const LANCEDB_RECORD_BATCH_ROWS: usize = 256;
#[cfg(feature = "lancedb-store")]
const LANCEDB_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(feature = "lancedb-store")]
static LANCEDB_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
#[cfg(feature = "lancedb-store")]
static LANCEDB_PATH_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    let database_path = database_path.as_ref();
    let path_lock = lancedb_path_lock(database_path);
    let Ok(_guard) = path_lock.lock() else {
        return false;
    };
    let _ = recover_lancedb_swap(database_path);
    database_path
        .join(format!("{LANCEDB_WORKSPACE_TABLE}.lance"))
        .exists()
}

#[cfg(feature = "lancedb-store")]
pub fn replace_lancedb_index(
    database_path: impl AsRef<Path>,
    index: &RagIndex,
) -> Result<usize, RagError> {
    replace_lancedb_index_cancellable(database_path, index, || false)
}

#[cfg(feature = "lancedb-store")]
pub fn replace_lancedb_index_cancellable(
    database_path: impl AsRef<Path>,
    index: &RagIndex,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<usize, RagError> {
    let database_path = database_path.as_ref();
    let path_lock = lancedb_path_lock(database_path);
    let _guard = path_lock
        .lock()
        .map_err(|error| RagError::new(format!("LanceDB path lock poisoned: {error}")))?;
    recover_lancedb_swap(database_path)?;
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    if index.chunks.is_empty() {
        if database_path.exists() {
            fs::remove_dir_all(database_path).map_err(|error| {
                RagError::new(format!("failed to clear empty LanceDB index: {error}"))
            })?;
        }
        return Ok(0);
    }
    let dimensions = index.chunks[0].embedding_dimensions;
    if dimensions == 0 {
        return Err(RagError::new(
            "LanceDB index requires one non-empty embedding dimension",
        ));
    }
    for chunk in &index.chunks {
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        if chunk.embedding_dimensions != dimensions || chunk.embedding.len() != dimensions {
            return Err(RagError::new(
                "LanceDB index requires one non-empty embedding dimension",
            ));
        }
    }
    let parent = database_path
        .parent()
        .ok_or_else(|| RagError::new("LanceDB path has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| RagError::new(format!("failed to create LanceDB parent: {error}")))?;
    let staging = unique_lancedb_sibling(database_path, "staging");
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    let batches = lancedb_record_batches_cancellable(index, dimensions, &mut should_cancel)?;
    let row_count = index.chunks.len();
    if should_cancel() {
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    let build_result = lancedb_runtime()?.block_on(async {
        let database = await_lancedb_operation_cancellable(
            async {
                lancedb::connect(&staging.to_string_lossy())
                    .execute()
                    .await
                    .map_err(|error| {
                        RagError::new(format!("failed to open LanceDB staging: {error}"))
                    })
            },
            &mut should_cancel,
        )
        .await?;
        let schema = batches[0].schema();
        let reader: Box<dyn arrow_array::RecordBatchReader + Send> = Box::new(
            RecordBatchIterator::new(batches.into_iter().map(Ok), schema),
        );
        let table = await_lancedb_operation_cancellable(
            async {
                database
                    .create_table(LANCEDB_WORKSPACE_TABLE, reader)
                    .mode(CreateTableMode::Overwrite)
                    .execute()
                    .await
                    .map_err(|error| {
                        RagError::new(format!("failed to replace LanceDB table: {error}"))
                    })
            },
            &mut should_cancel,
        )
        .await?;
        if row_count >= LANCEDB_ANN_MIN_ROWS {
            await_lancedb_operation_cancellable(
                async {
                    table
                        .create_index(&["vector"], Index::Auto)
                        .execute()
                        .await
                        .map_err(|error| {
                            RagError::new(format!("failed to build LanceDB ANN index: {error}"))
                        })
                },
                &mut should_cancel,
            )
            .await?;
        }
        Ok::<(), RagError>(())
    });
    if let Err(error) = build_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if should_cancel() {
        let _ = fs::remove_dir_all(&staging);
        return Err(RagError::new(RAG_INDEX_CANCELLED));
    }
    if let Err(error) = swap_lancedb_directory(database_path, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
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
    let path_lock = lancedb_path_lock(database_path);
    let _guard = path_lock
        .lock()
        .map_err(|error| RagError::new(format!("LanceDB path lock poisoned: {error}")))?;
    recover_lancedb_swap(database_path)?;
    if !database_path
        .join(format!("{LANCEDB_WORKSPACE_TABLE}.lance"))
        .exists()
    {
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
async fn await_lancedb_operation_cancellable<T>(
    operation: impl Future<Output = Result<T, RagError>>,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<T, RagError> {
    tokio::pin!(operation);
    loop {
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        match tokio::time::timeout(LANCEDB_CANCEL_POLL_INTERVAL, operation.as_mut()).await {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}

#[cfg(feature = "lancedb-store")]
fn lancedb_record_batches_cancellable(
    index: &RagIndex,
    dimensions: usize,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<Vec<RecordBatch>, RagError> {
    let mut batches = Vec::with_capacity(index.chunks.len().div_ceil(LANCEDB_RECORD_BATCH_ROWS));
    for chunks in index.chunks.chunks(LANCEDB_RECORD_BATCH_ROWS) {
        ensure_rag_index_not_cancelled(should_cancel)?;
        batches.push(lancedb_record_batch_cancellable(
            chunks,
            dimensions,
            should_cancel,
        )?);
    }
    Ok(batches)
}

#[cfg(feature = "lancedb-store")]
fn lancedb_record_batch_cancellable(
    chunks: &[RagChunk],
    dimensions: usize,
    should_cancel: &mut dyn FnMut() -> bool,
) -> Result<RecordBatch, RagError> {
    ensure_rag_index_not_cancelled(should_cancel)?;
    let vector = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        chunks.iter().map(|chunk| {
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
    ensure_rag_index_not_cancelled(should_cancel)?;
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
            chunks.iter().map(|chunk| chunk.id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            chunks.iter().map(|chunk| chunk.path.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            chunks.iter().map(|chunk| chunk.file_hash.as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            chunks.iter().map(|chunk| chunk.modified_time_ms),
        )),
        Arc::new(UInt64Array::from_iter_values(
            chunks.iter().map(|chunk| chunk.start_line),
        )),
        Arc::new(UInt64Array::from_iter_values(
            chunks.iter().map(|chunk| chunk.end_line),
        )),
        Arc::new(UInt64Array::from_iter_values(
            chunks.iter().map(|chunk| chunk.indexed_at_ms),
        )),
        Arc::new(StringArray::from_iter_values(
            chunks.iter().map(|chunk| chunk.text.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            chunks.iter().map(|chunk| chunk.embedding_provider.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            chunks.iter().map(|chunk| chunk.embedding_model.as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            chunks.iter().map(|chunk| chunk.embedding_dimensions as u64),
        )),
        Arc::new(vector),
    ];
    ensure_rag_index_not_cancelled(should_cancel)?;
    RecordBatch::try_new(schema, columns)
        .map_err(|error| RagError::new(format!("failed to build LanceDB record batch: {error}")))
}

#[cfg(feature = "lancedb-store")]
fn ensure_rag_index_not_cancelled(should_cancel: &mut dyn FnMut() -> bool) -> Result<(), RagError> {
    if should_cancel() {
        Err(RagError::new(RAG_INDEX_CANCELLED))
    } else {
        Ok(())
    }
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
fn lancedb_u64_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt64Array, RagError> {
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
    let had_existing = database_path.exists();
    if !had_existing {
        return fs::rename(staging, database_path)
            .map_err(|error| RagError::new(format!("failed to activate LanceDB index: {error}")));
    }
    let backup = unique_lancedb_sibling(database_path, "backup");
    let marker = lancedb_swap_marker_path(database_path)?;
    write_lancedb_swap_marker(&marker, staging, &backup)?;
    fs::rename(database_path, &backup)
        .map_err(|error| RagError::new(format!("failed to stage old LanceDB: {error}")))?;
    if let Err(error) = fs::rename(staging, database_path) {
        let _ = fs::rename(&backup, database_path);
        let _ = fs::remove_file(&marker);
        return Err(RagError::new(format!(
            "failed to activate LanceDB index: {error}"
        )));
    }
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    let _ = fs::remove_dir_all(&backup);
    let _ = fs::remove_file(&marker);
    Ok(())
}

#[cfg(feature = "lancedb-store")]
fn lancedb_path_lock(database_path: &Path) -> Arc<Mutex<()>> {
    let key = fs::canonicalize(database_path).unwrap_or_else(|_| database_path.to_path_buf());
    let mut locks = LANCEDB_PATH_LOCKS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        lock
    } else {
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }
}

#[cfg(feature = "lancedb-store")]
fn unique_lancedb_sibling(database_path: &Path, label: &str) -> PathBuf {
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let name = database_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("lancedb");
    parent.join(format!(
        ".{name}-{label}-{}-{}-{}",
        std::process::id(),
        current_time_millis(),
        STAGING_FILE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed)
    ))
}

#[cfg(feature = "lancedb-store")]
fn lancedb_swap_marker_path(database_path: &Path) -> Result<PathBuf, RagError> {
    let parent = database_path
        .parent()
        .ok_or_else(|| RagError::new("LanceDB path has no parent directory"))?;
    let name = database_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("lancedb");
    Ok(parent.join(format!(".{name}-swap")))
}

#[cfg(feature = "lancedb-store")]
fn write_lancedb_swap_marker(marker: &Path, staging: &Path, backup: &Path) -> Result<(), RagError> {
    let staging_name = staging
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| RagError::new("LanceDB staging path is invalid"))?;
    let backup_name = backup
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| RagError::new("LanceDB backup path is invalid"))?;
    let temporary = staging_file_path(marker, "swap-marker");
    let result = (|| {
        let mut file = fs::File::create(&temporary).map_err(|error| {
            RagError::new(format!("failed to create LanceDB swap marker: {error}"))
        })?;
        writeln!(file, "{staging_name}")
            .and_then(|_| writeln!(file, "{backup_name}"))
            .map_err(|error| {
                RagError::new(format!("failed to write LanceDB swap marker: {error}"))
            })?;
        file.sync_all().map_err(|error| {
            RagError::new(format!("failed to sync LanceDB swap marker: {error}"))
        })?;
        fs::rename(&temporary, marker).map_err(|error| {
            RagError::new(format!("failed to commit LanceDB swap marker: {error}"))
        })?;
        if let Some(parent) = marker.parent() {
            if let Ok(directory) = fs::File::open(parent) {
                directory.sync_all().map_err(|error| {
                    RagError::new(format!("failed to sync LanceDB swap directory: {error}"))
                })?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(feature = "lancedb-store")]
fn recover_lancedb_swap(database_path: &Path) -> Result<(), RagError> {
    let marker = lancedb_swap_marker_path(database_path)?;
    if !marker.exists() {
        return Ok(());
    }
    let parent = database_path
        .parent()
        .ok_or_else(|| RagError::new("LanceDB path has no parent directory"))?;
    let marker_text = fs::read_to_string(&marker).ok();
    let siblings = marker_text.as_deref().and_then(|marker_text| {
        let mut lines = marker_text.lines();
        let staging = lines
            .next()
            .filter(|value| valid_lancedb_sibling_name(value))?;
        let backup = lines
            .next()
            .filter(|value| valid_lancedb_sibling_name(value))?;
        Some((parent.join(staging), parent.join(backup)))
    });
    if database_path.exists() {
        if let Some((staging, backup)) = siblings {
            let _ = fs::remove_dir_all(staging);
            let _ = fs::remove_dir_all(backup);
        }
        let _ = fs::remove_file(&marker);
        return Ok(());
    }
    let (staging, backup) = siblings.ok_or_else(|| {
        RagError::new("LanceDB swap marker is unreadable while the database is unavailable")
    })?;
    if staging.exists() {
        fs::rename(&staging, database_path).map_err(|error| {
            RagError::new(format!("failed to recover staged LanceDB index: {error}"))
        })?;
        let _ = fs::remove_dir_all(&backup);
        let _ = fs::remove_file(&marker);
        return Ok(());
    }
    if backup.exists() {
        fs::rename(&backup, database_path).map_err(|error| {
            RagError::new(format!("failed to restore previous LanceDB index: {error}"))
        })?;
    }
    let _ = fs::remove_file(&marker);
    Ok(())
}

#[cfg(feature = "lancedb-store")]
fn valid_lancedb_sibling_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

pub trait RagAdapter {
    fn replace_all(&mut self, index: RagIndex) -> Result<RagIndexStats, RagError>;

    fn search(&self, query: &str, limit: usize) -> Result<Vec<RagSearchResult>, RagError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRagAdapter {
    path: PathBuf,
    index: Arc<RagIndex>,
    _path_lease: Arc<()>,
}

impl FileRagAdapter {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, RagError> {
        let path = path.into();
        let path_lease = acquire_file_rag_path_lease(&path)?;
        let index = if path.exists() {
            load_index(&path)?
        } else {
            empty_index()
        };

        Ok(Self {
            path,
            index: Arc::new(index),
            _path_lease: path_lease,
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

    pub fn path(&self) -> &Path {
        &self.path
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

    pub fn replace_all_cancellable(
        &mut self,
        index: RagIndex,
        should_cancel: impl FnMut() -> bool,
    ) -> Result<RagIndexStats, RagError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RagError::new(format!("failed to create RAG directory: {error}"))
            })?;
        }
        save_index_cancellable(&self.path, &index, should_cancel)?;
        self.index = Arc::new(index);
        Ok(self.index.stats.clone())
    }
}

pub fn read_file_rag_stats(path: impl AsRef<Path>) -> Result<Option<RagIndexStats>, RagError> {
    let file = match fs::File::open(path.as_ref()) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(RagIndexStats::default()));
        }
        Err(error) => {
            return Err(RagError::new(format!(
                "failed to read RAG index stats: {error}"
            )));
        }
    };
    let mut header = Vec::new();
    let mut reader = BufReader::new(file).take(FILE_RAG_STATS_HEADER_MAX_BYTES);
    reader
        .read_until(b'\n', &mut header)
        .map_err(|error| RagError::new(format!("failed to read RAG index stats: {error}")))?;
    if header.len() as u64 == FILE_RAG_STATS_HEADER_MAX_BYTES && !header.ends_with(b"\n") {
        return Ok(None);
    }
    while matches!(header.last(), Some(b'\n' | b'\r')) {
        header.pop();
    }
    let Ok(header) = std::str::from_utf8(&header) else {
        return Ok(None);
    };
    let mut parts = header.split('\t');
    if parts.next() != Some("stats") {
        return Ok(None);
    }
    let (Some(files), Some(chunks), Some(indexed_at), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(None);
    };
    let (Ok(files_indexed), Ok(chunks_indexed), Ok(indexed_at_ms)) =
        (files.parse(), chunks.parse(), indexed_at.parse())
    else {
        return Ok(None);
    };
    Ok(Some(RagIndexStats {
        files_indexed,
        chunks_indexed,
        indexed_at_ms,
    }))
}

fn acquire_file_rag_path_lease(path: &Path) -> Result<Arc<()>, RagError> {
    let path = stable_path_identity(path);
    let mut leases = FILE_RAG_PATH_LEASES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| RagError::new(format!("RAG path lease registry poisoned: {error}")))?;
    leases.retain(|_, lease| lease.strong_count() > 0);
    if let Some(lease) = leases.get(&path).and_then(Weak::upgrade) {
        return Ok(lease);
    }
    let lease = Arc::new(());
    leases.insert(path, Arc::downgrade(&lease));
    Ok(lease)
}

pub fn remove_file_rag_generation_if_unleased(
    index_path: &Path,
    generation_root: &Path,
) -> Result<bool, RagError> {
    let index_path = stable_path_identity(index_path);
    let mut leases = FILE_RAG_PATH_LEASES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|error| RagError::new(format!("RAG path lease registry poisoned: {error}")))?;
    leases.retain(|_, lease| lease.strong_count() > 0);
    if leases.get(&index_path).and_then(Weak::upgrade).is_some() {
        return Ok(false);
    }
    fs::remove_dir_all(generation_root)
        .map_err(|error| RagError::new(format!("failed to remove RAG generation: {error}")))?;
    Ok(true)
}

fn stable_path_identity(path: &Path) -> PathBuf {
    let mut candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut unresolved = Vec::new();
    loop {
        if let Ok(mut resolved) = fs::canonicalize(&candidate) {
            for component in unresolved.iter().rev() {
                resolved.push(component);
            }
            return lexically_normalize_path(&resolved);
        }
        let Some(parent) = candidate.parent() else {
            return lexically_normalize_path(&candidate);
        };
        let component = candidate
            .strip_prefix(parent)
            .unwrap_or(candidate.as_path())
            .to_path_buf();
        if component.as_os_str().is_empty() {
            return lexically_normalize_path(&candidate);
        }
        unresolved.push(component);
        candidate = parent.to_path_buf();
    }
}

fn lexically_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

impl RagAdapter for FileRagAdapter {
    fn replace_all(&mut self, index: RagIndex) -> Result<RagIndexStats, RagError> {
        self.replace_all_cancellable(index, || false)
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
    apply_embeddings_to_index_cancellable(&mut index, embedder, &mut should_cancel)?;
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
        if provider
            .as_ref()
            .is_some_and(|value| value != &batch.provider)
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
    let query_norm = vector_norm(query_embedding);
    let scored = chunks
        .iter()
        .enumerate()
        .filter(|(_, chunk)| chunk.embedding_dimensions == query_embedding.len())
        .filter_map(|(index, chunk)| {
            let score =
                cosine_similarity_with_left_norm(query_embedding, query_norm, &chunk.embedding);
            (score > 0.0).then_some((index, score))
        })
        .collect::<Vec<_>>();
    top_scored_chunks(chunks, scored, limit)
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
    let scored = chunks
        .iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let normalized_text = chunk.text.to_lowercase();
            let normalized_path = chunk.path.to_lowercase();
            let exact_matches = normalized_text.matches(&normalized_query).count();
            let path_exact = normalized_path.contains(&normalized_query);
            let score = if path_exact {
                1.25 + (exact_matches.min(8) as f32 * 0.04)
            } else if exact_matches > 0 {
                1.0 + (exact_matches.min(8) as f32 * 0.05)
            } else {
                let text_score =
                    lexical_overlap_in_normalized_text(&query_tokens, &normalized_text);
                let path_score =
                    lexical_overlap_in_normalized_text(&query_tokens, &normalized_path);
                (text_score * 0.75) + (path_score * 0.45)
            };
            (score > 0.0).then_some((index, score))
        })
        .collect::<Vec<_>>();
    top_scored_chunks(chunks, scored, limit.clamp(1, 50))
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
    let query_norm = vector_norm(query_embedding);
    let scored = chunks
        .iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let vector_score = if query_embedding.len() == chunk.embedding_dimensions {
                cosine_similarity_with_left_norm(query_embedding, query_norm, &chunk.embedding)
            } else {
                0.0
            };
            let lexical_score = lexical_overlap_in_text(&query_tokens, &chunk.text);
            let score = (vector_score * 0.72) + (lexical_score * 0.28);
            (score > 0.0).then_some((index, score))
        })
        .collect::<Vec<_>>();
    top_scored_chunks(chunks, scored, limit)
}

fn top_scored_chunks(
    chunks: &[RagChunk],
    mut scored: Vec<(usize, f32)>,
    limit: usize,
) -> Vec<RagSearchResult> {
    let compare = |(left_index, left_score): &(usize, f32),
                   (right_index, right_score): &(usize, f32)| {
        right_score
            .partial_cmp(left_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| chunks[*left_index].path.cmp(&chunks[*right_index].path))
            .then_with(|| {
                chunks[*left_index]
                    .start_line
                    .cmp(&chunks[*right_index].start_line)
            })
    };
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, compare);
        scored.truncate(limit);
    }
    scored.sort_by(compare);
    scored
        .into_iter()
        .map(|(index, score)| RagSearchResult {
            chunk: chunks[index].clone(),
            score,
        })
        .collect()
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
    export_lancedb_records_jsonl_cancellable(index, path, || false)
}

pub fn export_lancedb_records_jsonl_cancellable(
    index: &RagIndex,
    path: impl AsRef<Path>,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<usize, RagError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RagError::new(format!(
                "failed to create LanceDB export directory: {error}"
            ))
        })?;
    }
    let staging = staging_file_path(path, "lancedb-export");
    let result = (|| {
        let file = fs::File::create(&staging).map_err(|error| {
            RagError::new(format!(
                "failed to create LanceDB export staging file: {error}"
            ))
        })?;
        let mut writer = BufWriter::new(file);
        for (index, chunk) in index.chunks.iter().enumerate() {
            if should_cancel() {
                return Err(RagError::new(RAG_INDEX_CANCELLED));
            }
            if index > 0 {
                writer.write_all(b"\n").map_err(|error| {
                    RagError::new(format!("failed to write LanceDB export: {error}"))
                })?;
            }
            let record = LanceDbRecord {
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
            };
            writer
                .write_all(lancedb_record_json(&record).as_bytes())
                .map_err(|error| {
                    RagError::new(format!("failed to write LanceDB export: {error}"))
                })?;
        }
        writer
            .flush()
            .map_err(|error| RagError::new(format!("failed to flush LanceDB export: {error}")))?;
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        fs::rename(&staging, path)
            .map_err(|error| RagError::new(format!("failed to commit LanceDB export: {error}")))?;
        Ok(index.chunks.len())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
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
        Err(error) => {
            return Err(RagError::new(format!(
                "failed to stat search path: {error}"
            )))
        }
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
        Err(error) => {
            return Err(RagError::new(format!(
                "failed to search directory: {error}"
            )))
        }
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
            id: stable_hash_hex(
                format!("{relative}:{start_line}:{end_line}:{file_hash}").as_bytes(),
            ),
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
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
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
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".icns", ".ico", ".pdf", ".zip", ".gz", ".tar",
        ".sqlite", ".sqlite3", ".db", ".app", ".dylib", ".so", ".a",
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

fn vector_norm(vector: &[f32]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
}

fn cosine_similarity_with_left_norm(left: &[f32], left_norm: f32, right: &[f32]) -> f32 {
    let (dot, right_squared_norm) = left
        .iter()
        .zip(right.iter())
        .fold((0.0f32, 0.0f32), |(dot, norm), (left, right)| {
            (dot + left * right, norm + right * right)
        });
    let right_norm = right_squared_norm.sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }

    (dot / (left_norm * right_norm)).max(0.0)
}

fn lexical_overlap(query: &BTreeMap<String, usize>, chunk: &BTreeMap<String, usize>) -> f32 {
    if query.is_empty() {
        return 0.0;
    }

    let hits = query
        .keys()
        .filter(|token| chunk.contains_key(*token))
        .count();
    hits as f32 / query.len() as f32
}

fn lexical_overlap_in_text(query: &BTreeMap<String, usize>, text: &str) -> f32 {
    lexical_overlap_in_chars(
        query,
        text.chars().flat_map(|character| character.to_lowercase()),
    )
}

fn lexical_overlap_in_normalized_text(
    query: &BTreeMap<String, usize>,
    normalized_text: &str,
) -> f32 {
    lexical_overlap_in_chars(query, normalized_text.chars())
}

fn lexical_overlap_in_chars(
    query: &BTreeMap<String, usize>,
    characters: impl IntoIterator<Item = char>,
) -> f32 {
    if query.is_empty() {
        return 0.0;
    }

    let mut matched = BTreeSet::new();
    let mut current = String::new();
    let record_token = |token: &mut String, matched: &mut BTreeSet<String>| {
        if !token.is_empty() {
            if query.contains_key(token) {
                matched.insert(std::mem::take(token));
            } else {
                token.clear();
            }
        }
    };
    for character in characters {
        if character.is_alphanumeric() || character == '_' {
            current.push(character);
        } else {
            record_token(&mut current, &mut matched);
            if matched.len() == query.len() {
                return 1.0;
            }
        }
    }
    record_token(&mut current, &mut matched);
    matched.len() as f32 / query.len() as f32
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

fn staging_file_path(path: &Path, label: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("index");
    path.with_file_name(format!(
        ".{file_name}.{label}-{}-{}-{}",
        std::process::id(),
        current_time_millis(),
        STAGING_FILE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed)
    ))
}

fn save_index_cancellable(
    path: &Path,
    index: &RagIndex,
    mut should_cancel: impl FnMut() -> bool,
) -> Result<(), RagError> {
    let staging = staging_file_path(path, "rag-staging");
    let result = (|| {
        let file = fs::File::create(&staging)
            .map_err(|error| RagError::new(format!("failed to create RAG staging: {error}")))?;
        let mut writer = BufWriter::new(file);
        write!(
            writer,
            "stats\t{}\t{}\t{}",
            index.stats.files_indexed, index.stats.chunks_indexed, index.stats.indexed_at_ms
        )
        .map_err(|error| RagError::new(format!("failed to save RAG index: {error}")))?;
        for chunk in &index.chunks {
            if should_cancel() {
                return Err(RagError::new(RAG_INDEX_CANCELLED));
            }
            write!(
                writer,
                "\nchunk\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
            )
            .map_err(|error| RagError::new(format!("failed to save RAG index: {error}")))?;
        }
        writer
            .flush()
            .map_err(|error| RagError::new(format!("failed to flush RAG index: {error}")))?;
        if should_cancel() {
            return Err(RagError::new(RAG_INDEX_CANCELLED));
        }
        fs::rename(&staging, path)
            .map_err(|error| RagError::new(format!("failed to commit RAG index: {error}")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
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
            ["chunk", id, path, file_hash, modified_time_ms, start_line, end_line, indexed_at_ms, embedding_provider, embedding_model, embedding_dimensions, embedding, text] =>
            {
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
            ["chunk", id, path, file_hash, modified_time_ms, start_line, end_line, indexed_at_ms, embedding, text] =>
            {
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
    fn lightweight_stats_reader_ignores_a_large_invalid_tail() {
        let root = temp_workspace();
        let index_path = root.join("rag-index.tsv");
        let mut file = fs::File::create(&index_path).expect("index fixture should create");
        file.write_all(b"stats\t7\t19\t1234\n")
            .expect("stats header should write");
        file.write_all(&vec![0xff; 1024 * 1024])
            .expect("invalid tail should write");
        drop(file);

        let stats = read_file_rag_stats(&index_path)
            .expect("stats read should succeed")
            .expect("valid header should produce stats");

        assert_eq!(
            stats,
            RagIndexStats {
                files_indexed: 7,
                chunks_indexed: 19,
                indexed_at_ms: 1234,
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lightweight_stats_reader_distinguishes_missing_and_malformed_headers() {
        let root = temp_workspace();
        let missing = read_file_rag_stats(root.join("missing.tsv"))
            .expect("missing index should be a zero state");
        assert_eq!(missing, Some(RagIndexStats::default()));

        let malformed_path = root.join("malformed.tsv");
        fs::write(&malformed_path, b"chunk\tnot-a-stats-header\n")
            .expect("malformed fixture should write");
        assert_eq!(
            read_file_rag_stats(&malformed_path).expect("malformed header should be non-fatal"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn missing_rag_path_lease_has_stable_symlink_and_parent_identity() {
        use std::os::unix::fs::symlink;

        let container = temp_workspace();
        let real_root = container.join("real");
        fs::create_dir_all(&real_root).expect("real root should exist");
        let alias_root = container.join("alias");
        symlink(&real_root, &alias_root).expect("workspace alias should exist");
        let aliased_index = alias_root
            .join("missing")
            .join("..")
            .join("generation")
            .join("rag-index.tsv");
        let real_index = real_root.join("generation").join("rag-index.tsv");
        assert_eq!(
            stable_path_identity(&aliased_index),
            stable_path_identity(&real_index)
        );

        let adapter = FileRagAdapter::open(&aliased_index)
            .expect("missing generation adapter should acquire a path lease");
        let generation_root = real_root.join("generation");
        fs::create_dir_all(&generation_root).expect("generation should exist");
        fs::write(&real_index, "stats\t0\t0\t0\n").expect("RAG index should exist");
        assert!(
            !remove_file_rag_generation_if_unleased(&real_index, &generation_root)
                .expect("leased generation removal should be checked")
        );
        assert!(generation_root.is_dir());
        drop(adapter);
        assert!(
            remove_file_rag_generation_if_unleased(&real_index, &generation_root)
                .expect("unleased generation should be removed")
        );
        assert!(!generation_root.exists());
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

        assert!(
            workspace_index_is_fresh(&root, &index.chunks, IndexOptions::default(), || false)
                .expect("freshness should be checked")
        );

        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&notes, "modified workspace notes").expect("file should update");
        assert!(
            !workspace_index_is_fresh(&root, &index.chunks, IndexOptions::default(), || false)
                .expect("modified file should be detected")
        );

        let updated =
            index_workspace(&root, IndexOptions::default()).expect("index should rebuild");
        fs::write(root.join("new.md"), "new knowledge").expect("new file should write");
        assert!(
            !workspace_index_is_fresh(&root, &updated.chunks, IndexOptions::default(), || false)
                .expect("new file should be detected")
        );

        fs::remove_file(root.join("new.md")).expect("new file should remove");
        fs::remove_file(&notes).expect("indexed file should remove");
        assert!(
            !workspace_index_is_fresh(&root, &updated.chunks, IndexOptions::default(), || false)
                .expect("deleted file should be detected")
        );
    }

    #[test]
    fn empty_workspace_index_is_fresh_until_an_indexable_file_appears() {
        let root = temp_workspace();
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        assert!(index.chunks.is_empty());
        assert!(
            workspace_index_is_fresh(&root, &index.chunks, IndexOptions::default(), || false)
                .expect("empty workspace freshness should be checked")
        );

        fs::write(root.join("notes.md"), "new knowledge").expect("fixture should write");
        assert!(
            !workspace_index_is_fresh(&root, &index.chunks, IndexOptions::default(), || false)
                .expect("new file should invalidate the empty index")
        );
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
        fs::write(
            root.join("a.md"),
            "permission audit events and model traces",
        )
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
        let results = loaded
            .search("retrieval sources", 2)
            .expect("search should work");

        assert_eq!(loaded.stats().chunks_indexed, 1);
        assert_eq!(results[0].chunk.path, "readme.md");
        assert_eq!(
            results[0].chunk.embedding_model,
            format!("local-hash-{EMBEDDING_DIMS}")
        );
    }

    #[test]
    fn cancelled_file_adapter_replace_keeps_the_previous_snapshot() {
        let root = temp_workspace();
        let index_path = root.join(".cindx").join("rag-index.tsv");
        fs::write(root.join("readme.md"), "stable retrieval snapshot").expect("file should write");
        let original = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let mut replacement = original.clone();
        replacement.chunks[0].path = "replacement.md".to_string();
        let mut adapter = FileRagAdapter::open(&index_path).expect("adapter should open");
        adapter
            .replace_all(original)
            .expect("original index should save");

        let error = adapter
            .replace_all_cancellable(replacement, || true)
            .expect_err("replacement should cancel");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
        assert_eq!(adapter.chunks()[0].path, "readme.md");
        let restored = FileRagAdapter::open(&index_path).expect("index should reload");
        assert_eq!(restored.chunks()[0].path, "readme.md");
    }

    #[test]
    fn concurrent_staging_files_never_share_a_path() {
        let destination = Path::new("rag-index.tsv");

        let first = staging_file_path(destination, "rag-staging");
        let second = staging_file_path(destination, "rag-staging");

        assert_ne!(first, second);
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn concurrent_lancedb_directories_never_share_a_path() {
        let destination = Path::new(".cindx/lancedb");

        let first = unique_lancedb_sibling(destination, "staging");
        let second = unique_lancedb_sibling(destination, "staging");
        let backup = unique_lancedb_sibling(destination, "backup");

        assert_ne!(first, second);
        assert_ne!(first, backup);
        assert_ne!(second, backup);
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn interrupted_lancedb_swap_activates_the_complete_staging_directory() {
        let root = temp_workspace();
        let database = root.join(".cindx").join("lancedb");
        let parent = database.parent().expect("database should have parent");
        fs::create_dir_all(&database).expect("old database should exist");
        fs::write(database.join("old"), "old").expect("old marker should write");
        let staging = unique_lancedb_sibling(&database, "staging");
        let backup = unique_lancedb_sibling(&database, "backup");
        fs::create_dir_all(&staging).expect("staging database should exist");
        fs::write(staging.join("new"), "new").expect("new marker should write");
        let marker = lancedb_swap_marker_path(&database).expect("marker path should resolve");
        write_lancedb_swap_marker(&marker, &staging, &backup).expect("swap marker should persist");
        fs::rename(&database, &backup).expect("old database should be staged");

        recover_lancedb_swap(&database).expect("interrupted swap should recover");

        assert!(database.join("new").is_file());
        assert!(!backup.exists());
        assert!(!marker.exists());
        assert!(parent.exists());
    }

    #[test]
    fn cloning_file_adapter_shares_the_immutable_index_snapshot() {
        let root = temp_workspace();
        let index_path = root.join(".cindx").join("rag-index.tsv");
        fs::write(root.join("readme.md"), "shared retrieval snapshot").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let mut adapter = FileRagAdapter::open(&index_path).expect("adapter should open");
        adapter.replace_all(index).expect("index should save");

        let cloned = adapter.clone();

        assert!(Arc::ptr_eq(&adapter.index, &cloned.index));
    }

    #[test]
    fn indexes_workspace_with_external_embeddings() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "cloud embedding vector alpha").expect("file should write");
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

        let error =
            index_workspace_with_embedder_cancellable(&root, options, &mut embedder, || {
                checks += 1;
                checks >= 6
            })
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

        assert!(index.chunks.iter().all(|chunk| chunk.path != "large.log"));
        let results =
            search_workspace_files_cancellable(&root, "unique direct file evidence", 4, || false)
                .expect("direct file search should succeed");

        assert_eq!(results[0].chunk.path, "large.log");
        assert!(results[0]
            .chunk
            .text
            .contains("unique direct file evidence"));
        assert!(results[0].chunk.start_line <= results[0].chunk.end_line);
    }

    #[test]
    fn indexing_respects_max_files() {
        let root = temp_workspace();
        for name in ["a.md", "b.md", "c.md"] {
            fs::write(root.join(name), format!("content for {name}")).expect("file should write");
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
    fn cancelled_lancedb_export_preserves_the_previous_file() {
        let root = temp_workspace();
        fs::write(root.join("readme.md"), "lancedb cancellable export").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let output_path = root.join(".cindx").join("lancedb-records.jsonl");
        fs::create_dir_all(output_path.parent().expect("output should have parent"))
            .expect("output directory should exist");
        fs::write(&output_path, "stable export").expect("previous export should write");

        let error = export_lancedb_records_jsonl_cancellable(&index, &output_path, || true)
            .expect_err("export should cancel");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
        assert_eq!(
            fs::read_to_string(output_path).expect("previous export should remain"),
            "stable export"
        );
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn cancellable_lancedb_replace_stops_before_building() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "cancelled lancedb replacement").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let database_path = root.join(".cindx").join("lancedb");

        let error = replace_lancedb_index_cancellable(&database_path, &index, || true)
            .expect_err("replacement should cancel");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
        assert!(!database_path.exists());
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn arrow_record_batch_build_observes_cancellation_between_bounded_steps() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "cancel during Arrow conversion").expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let dimensions = index.chunks[0].embedding_dimensions;
        let mut cancellation_checks = 0usize;

        let error = lancedb_record_batches_cancellable(&index, dimensions, &mut || {
            cancellation_checks += 1;
            cancellation_checks == 3
        })
        .expect_err("Arrow conversion should stop at the requested boundary");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
        assert_eq!(cancellation_checks, 3);
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn bounded_arrow_batches_preserve_every_row_and_schema() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "bounded Arrow batches").expect("file should write");
        let mut index =
            index_workspace(&root, IndexOptions::default()).expect("index should build");
        let prototype = index.chunks[0].clone();
        let row_count = LANCEDB_RECORD_BATCH_ROWS + 1;
        index.chunks = (0..row_count)
            .map(|row| {
                let mut chunk = prototype.clone();
                chunk.id = format!("chunk-{row}");
                chunk
            })
            .collect();
        let dimensions = prototype.embedding_dimensions;

        let batches = lancedb_record_batches_cancellable(&index, dimensions, &mut || false)
            .expect("Arrow conversion should succeed");

        assert_eq!(batches.len(), 2);
        assert_eq!(
            batches.iter().map(RecordBatch::num_rows).sum::<usize>(),
            row_count
        );
        assert!(batches
            .iter()
            .all(|batch| batch.schema() == batches[0].schema()));
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn pending_lancedb_operation_observes_cancellation_on_the_next_poll() {
        let mut cancellation_checks = 0usize;
        let started_at = std::time::Instant::now();

        let error = lancedb_runtime()
            .expect("runtime should start")
            .block_on(await_lancedb_operation_cancellable(
                std::future::pending::<Result<(), RagError>>(),
                &mut || {
                    cancellation_checks += 1;
                    cancellation_checks == 2
                },
            ))
            .expect_err("pending LanceDB operation should be cancelled");

        assert_eq!(error.message, RAG_INDEX_CANCELLED);
        assert_eq!(cancellation_checks, 2);
        assert!(started_at.elapsed() < std::time::Duration::from_secs(1));
    }

    #[cfg(feature = "lancedb-store")]
    #[test]
    fn persists_and_searches_a_real_lancedb_index() {
        let root = temp_workspace();
        fs::write(root.join("a.md"), "permission audit model traces").expect("file should write");
        fs::write(root.join("b.md"), "recipe ingredients cooking notes")
            .expect("file should write");
        let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
        let database_path = root.join(".cindx").join("lancedb");

        let rows =
            replace_lancedb_index(&database_path, &index).expect("LanceDB index should persist");
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

        let prompt =
            build_grounded_answer_prompt("What matters?", &[RagSearchResult { chunk, score: 0.9 }]);

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

        let sample_count = 11usize;
        let mut semantic_samples = Vec::with_capacity(sample_count);
        let mut literal_samples = Vec::with_capacity(sample_count);
        for _ in 0..sample_count {
            let semantic_started_at = std::time::Instant::now();
            let semantic = search_chunks_semantic(&chunks, &query_embedding, 12);
            semantic_samples.push(semantic_started_at.elapsed().as_micros());
            let literal_started_at = std::time::Instant::now();
            let literal = search_chunks_literal(&chunks, "graph memory file evidence", 12);
            literal_samples.push(literal_started_at.elapsed().as_micros());
            assert_eq!(semantic.len(), 12);
            assert_eq!(literal.len(), 12);
        }
        semantic_samples.sort_unstable();
        literal_samples.sort_unstable();
        let percentile = |samples: &[u128], value: usize| {
            samples[(samples.len().saturating_sub(1) * value) / 100]
        };
        let semantic_micros = percentile(&semantic_samples, 50);
        let semantic_p95_micros = percentile(&semantic_samples, 95);
        let literal_micros = percentile(&literal_samples, 50);
        let literal_p95_micros = percentile(&literal_samples, 95);
        println!(
            "{{\"schema\":\"cindx.rag-search-diagnostic.v1\",\"chunks\":{chunk_count},\"dimensions\":{EMBEDDING_DIMS},\"estimated_payload_bytes\":{estimated_payload_bytes},\"sample_count\":{sample_count},\"semantic_micros\":{semantic_micros},\"semantic_p95_micros\":{semantic_p95_micros},\"literal_micros\":{literal_micros},\"literal_p95_micros\":{literal_p95_micros}}}"
        );
    }
}
