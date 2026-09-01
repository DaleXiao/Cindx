use crate::{
    app_state::AppState,
    persistence_runtime::{
        normalized_attachment_mime, project_root_for_session, safe_attachment_name,
        validated_attachment_path,
    },
    runtime_constants::{MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_FILES, MAX_ATTACHMENT_TOTAL_BYTES},
    runtime_values::{slug_label, unique_id},
    view_models::{
        AbortAgentAttachmentBatchInput, AgentAttachmentView, RawAttachmentUploadMetadata,
        RemoveAgentAttachmentInput,
    },
};
use base64::Engine;
use std::{
    fs,
    path::{Path, PathBuf},
};

const ATTACHMENT_UPLOAD_METADATA_HEADER: &str = "x-cindx-attachment-metadata";

#[tauri::command]
pub(crate) async fn stage_agent_attachment(
    state: tauri::State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<AgentAttachmentView, String> {
    let metadata = raw_attachment_upload_metadata(&request)?;
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        tauri::ipc::InvokeBody::Json(_) => {
            return Err("attachment upload requires a binary IPC body".to_string())
        }
    };
    validate_raw_attachment_upload(&metadata, bytes.len())?;
    let root = project_root_for_session(&state, &metadata.session_id)?;
    let reservation = state
        .attachment_upload_batches
        .lock()
        .map_err(|error| format!("attachment batch lock poisoned: {error}"))?
        .reserve(&metadata, bytes.len() as u64)?;
    let bytes = bytes.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        stage_raw_agent_attachment(&root, metadata, bytes)
    })
    .await
    .map_err(|error| format!("attachment staging task failed: {error}"));
    let staged = match result {
        Ok(Ok(staged)) => staged,
        Ok(Err(error)) | Err(error) => {
            if let Ok(mut batches) = state.attachment_upload_batches.lock() {
                batches.rollback(reservation);
            }
            return Err(error);
        }
    };
    let staged_path = PathBuf::from(&staged.path);
    let completion = match state.attachment_upload_batches.lock() {
        Ok(mut batches) => batches.complete(reservation, staged_path.clone()),
        Err(error) => Err(format!("attachment batch lock poisoned: {error}")),
    };
    if let Err(error) = completion {
        let _ = fs::remove_file(staged_path);
        return Err(error);
    }
    Ok(staged)
}

fn raw_attachment_upload_metadata(
    request: &tauri::ipc::Request<'_>,
) -> Result<RawAttachmentUploadMetadata, String> {
    let encoded = request
        .headers()
        .get(ATTACHMENT_UPLOAD_METADATA_HEADER)
        .ok_or_else(|| "attachment upload metadata is missing".to_string())?
        .to_str()
        .map_err(|_| "attachment upload metadata is not valid ASCII".to_string())?;
    decode_raw_attachment_upload_metadata(encoded)
}

pub(crate) fn decode_raw_attachment_upload_metadata(
    encoded: &str,
) -> Result<RawAttachmentUploadMetadata, String> {
    let encoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("attachment upload metadata is not valid base64: {error}"))?;
    serde_json::from_slice(&encoded)
        .map_err(|error| format!("attachment upload metadata is invalid: {error}"))
}

pub(crate) fn validate_raw_attachment_upload(
    metadata: &RawAttachmentUploadMetadata,
    bytes_len: usize,
) -> Result<(), String> {
    if metadata.batch_file_count == 0 || metadata.batch_file_count > MAX_ATTACHMENT_FILES {
        return Err(format!(
            "a message can include at most {MAX_ATTACHMENT_FILES} attachments"
        ));
    }
    if metadata.batch_file_sizes.len() != metadata.batch_file_count {
        return Err("attachment batch manifest has an invalid file count".to_string());
    }
    if metadata.batch_index >= metadata.batch_file_count {
        return Err("attachment upload index is invalid".to_string());
    }
    if bytes_len > MAX_ATTACHMENT_BYTES
        || metadata
            .batch_file_sizes
            .iter()
            .any(|size| *size > MAX_ATTACHMENT_BYTES as u64)
    {
        return Err(format!(
            "{} exceeds the 20 MB attachment limit",
            metadata.name
        ));
    }
    let manifest_total = metadata
        .batch_file_sizes
        .iter()
        .copied()
        .fold(0_u64, u64::saturating_add);
    if metadata.batch_total_bytes != manifest_total {
        return Err("attachment batch manifest total is invalid".to_string());
    }
    if manifest_total > MAX_ATTACHMENT_TOTAL_BYTES as u64 {
        return Err("attachments exceed the 50 MB message limit".to_string());
    }
    if metadata.batch_file_sizes[metadata.batch_index] != bytes_len as u64 {
        return Err("attachment upload size does not match its batch manifest".to_string());
    }
    Ok(())
}

pub(crate) fn stage_raw_agent_attachment(
    root: &Path,
    metadata: RawAttachmentUploadMetadata,
    bytes: Vec<u8>,
) -> Result<AgentAttachmentView, String> {
    let attachment_root = root
        .join(".cindx")
        .join("attachments")
        .join(slug_label(&metadata.session_id));
    crate::private_files::private_dir_ensure(&attachment_root)
        .map_err(|error| format!("failed to create attachment directory: {error}"))?;
    let name = safe_attachment_name(&metadata.name);
    let id = unique_id("attachment");
    let path = attachment_root.join(format!("{id}-{}-{name}", metadata.batch_index));
    crate::private_files::private_file_write(&path, &bytes)
        .map_err(|error| format!("failed to stage attachment {name}: {error}"))?;
    Ok(AgentAttachmentView {
        id,
        name,
        path: path.display().to_string(),
        mime_type: normalized_attachment_mime(&metadata.mime_type, &path),
        size_bytes: bytes.len() as u64,
    })
}

#[tauri::command]
pub(crate) fn remove_agent_attachment(
    state: tauri::State<'_, AppState>,
    input: RemoveAgentAttachmentInput,
) -> Result<(), String> {
    let root = project_root_for_session(&state, &input.session_id)?;
    let path = validated_attachment_path(&root, &input.path)?;
    fs::remove_file(path).map_err(|error| format!("failed to remove attachment: {error}"))
}

#[tauri::command]
pub(crate) fn abort_agent_attachment_batch(
    state: tauri::State<'_, AppState>,
    input: AbortAgentAttachmentBatchInput,
) -> Result<(), String> {
    let paths = state
        .attachment_upload_batches
        .lock()
        .map_err(|error| format!("attachment batch lock poisoned: {error}"))?
        .abort(&input.session_id, &input.batch_id);
    cleanup_staged_attachment_paths(paths);
    Ok(())
}

pub(crate) fn cleanup_staged_attachment_paths(paths: Vec<PathBuf>) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}
