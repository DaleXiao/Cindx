use crate::{
    configuration_models::ProviderConfig,
    event_projection::write_private_file_atomically,
    knowledge_runtime::CloudRagEmbedder,
    memory_vector_generation_runtime::{
        memory_vector_manifest_matches, memory_vector_projection_sha256,
        open_memory_vector_snapshot, MemoryVectorManifest, PendingMemoryVectorGeneration,
    },
    runtime_constants::MEMORY_VECTOR_MANIFEST_SCHEMA,
    runtime_values::current_time_millis,
};
use agent_memory::{MemoryLedger, MemoryRecord};
use agent_rag::{
    apply_embeddings_to_index_cancellable, lancedb_index_exists, local_query_embedding,
    replace_lancedb_index, RagChunk, RagIndex, RagIndexStats,
};
use std::path::Path;

pub(crate) struct PreparedMemoryVectorRefresh {
    embedding_backend: String,
    fallback_error: Option<String>,
    index: RagIndex,
    projection_sha256: String,
}

pub(crate) fn memory_rag_index(ledger: &MemoryLedger) -> RagIndex {
    let indexed_at_ms = current_time_millis();
    let chunks = ledger
        .records
        .iter()
        .filter(|record| ledger.record_is_active_for_recall(record))
        .map(|record| memory_rag_chunk(ledger, record, indexed_at_ms))
        .collect::<Vec<_>>();
    RagIndex {
        stats: RagIndexStats {
            files_indexed: chunks.len(),
            chunks_indexed: chunks.len(),
            indexed_at_ms,
        },
        chunks,
    }
}

fn memory_rag_chunk(ledger: &MemoryLedger, record: &MemoryRecord, indexed_at_ms: u64) -> RagChunk {
    let embedding = local_query_embedding(&record.content);
    RagChunk {
        id: record.id.clone(),
        path: format!(
            "memory://{}/{}/{}",
            ledger.project_id,
            record.kind.label(),
            record.id
        ),
        file_hash: record.fingerprint.clone(),
        modified_time_ms: record.updated_at_ms,
        start_line: 1,
        end_line: 1,
        indexed_at_ms,
        text: record.content.clone(),
        embedding_dimensions: embedding.len(),
        embedding,
        embedding_provider: "local".to_string(),
        embedding_model: "local-hash".to_string(),
    }
}

pub(crate) fn prepare_project_memory_vector_refresh(
    workspace_root: &Path,
    config: &ProviderConfig,
    ledger: &MemoryLedger,
) -> Result<Option<PreparedMemoryVectorRefresh>, String> {
    let projection_sha256 = memory_vector_projection_sha256(ledger);
    let snapshot = open_memory_vector_snapshot(workspace_root, &ledger.project_id)?;
    let now_ms = current_time_millis();
    if let Some(manifest) = snapshot.manifest.as_ref() {
        let generation_matches = snapshot
            .generation_id
            .as_deref()
            .is_none_or(|generation| manifest.generation_id == generation);
        if generation_matches
            && (manifest.record_count == 0 || lancedb_index_exists(&snapshot.database_path))
            && memory_vector_manifest_matches(manifest, &projection_sha256, config, now_ms)
        {
            return Ok(None);
        }
    }
    drop(snapshot);

    let mut index = memory_rag_index(ledger);
    let mut embedding_backend = "local".to_string();
    let mut fallback_error = None;
    if config.is_ready() && !index.chunks.is_empty() {
        let mut embedder = CloudRagEmbedder {
            config: config.clone(),
            cancellation: None,
            expected_steer_epoch: None,
            resource_checkpoint: None,
        };
        match apply_embeddings_to_index_cancellable(&mut index, &mut embedder, || false) {
            Ok(()) => embedding_backend = "cloud".to_string(),
            Err(error) => {
                embedding_backend = "local-fallback".to_string();
                fallback_error = Some(error.to_string());
            }
        }
    }
    Ok(Some(PreparedMemoryVectorRefresh {
        embedding_backend,
        fallback_error,
        index,
        projection_sha256,
    }))
}

pub(crate) fn publish_prepared_memory_vector_refresh(
    workspace_root: &Path,
    ledger: &MemoryLedger,
    prepared: PreparedMemoryVectorRefresh,
) -> Result<Option<String>, String> {
    let PreparedMemoryVectorRefresh {
        embedding_backend,
        fallback_error,
        index,
        projection_sha256,
    } = prepared;
    let mut pending = PendingMemoryVectorGeneration::create(workspace_root, &ledger.project_id)?;
    replace_lancedb_index(&pending.database_path, &index).map_err(|error| error.to_string())?;
    pending.acquire_lease()?;
    let (embedding_provider, embedding_model, embedding_dimensions) = index
        .chunks
        .first()
        .map(|chunk| {
            (
                chunk.embedding_provider.clone(),
                chunk.embedding_model.clone(),
                chunk.embedding_dimensions,
            )
        })
        .unwrap_or_else(|| ("local".to_string(), "local-hash".to_string(), 0));
    let manifest = MemoryVectorManifest {
        schema: MEMORY_VECTOR_MANIFEST_SCHEMA.to_string(),
        generation_id: pending.generation_id.clone(),
        projection_sha256,
        record_count: index.chunks.len(),
        embedding_backend,
        embedding_provider,
        embedding_model,
        embedding_dimensions,
        generated_at_ms: current_time_millis(),
    };
    let payload = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("failed to encode memory vector manifest: {error}"))?;
    write_private_file_atomically(&pending.manifest_path, &payload, "memory vector manifest")?;
    pending.publish()?;
    Ok(fallback_error)
}
