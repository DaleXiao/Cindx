use crate::{
    runtime_constants::MAX_ATTACHMENT_TOTAL_BYTES, view_models::RawAttachmentUploadMetadata,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

const MAX_ACTIVE_ATTACHMENT_BATCHES: usize = 32;

#[derive(Default)]
pub(crate) struct AttachmentUploadBatches {
    batches: BTreeMap<String, AttachmentUploadBatch>,
}

struct AttachmentUploadBatch {
    session_id: String,
    file_sizes: Vec<u64>,
    total_bytes: u64,
    reserved_indexes: BTreeSet<usize>,
    completed_paths: BTreeMap<usize, PathBuf>,
    actual_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct AttachmentUploadReservation {
    batch_id: String,
    index: usize,
    bytes: u64,
}

impl AttachmentUploadBatches {
    pub(crate) fn reserve(
        &mut self,
        metadata: &RawAttachmentUploadMetadata,
        bytes: u64,
    ) -> Result<AttachmentUploadReservation, String> {
        if metadata.batch_id.trim().is_empty() || metadata.batch_id.len() > 128 {
            return Err("attachment batch id is invalid".to_string());
        }
        if !self.batches.contains_key(&metadata.batch_id) {
            if self.batches.len() >= MAX_ACTIVE_ATTACHMENT_BATCHES {
                return Err("too many attachment batches are active".to_string());
            }
            self.batches.insert(
                metadata.batch_id.clone(),
                AttachmentUploadBatch {
                    session_id: metadata.session_id.clone(),
                    file_sizes: metadata.batch_file_sizes.clone(),
                    total_bytes: metadata.batch_total_bytes,
                    reserved_indexes: BTreeSet::new(),
                    completed_paths: BTreeMap::new(),
                    actual_bytes: 0,
                },
            );
        }
        let batch = self
            .batches
            .get_mut(&metadata.batch_id)
            .expect("attachment batch was inserted");
        if batch.session_id != metadata.session_id
            || batch.file_sizes != metadata.batch_file_sizes
            || batch.total_bytes != metadata.batch_total_bytes
        {
            return Err("attachment batch manifest changed during upload".to_string());
        }
        if batch.reserved_indexes.contains(&metadata.batch_index)
            || batch.completed_paths.contains_key(&metadata.batch_index)
        {
            return Err("attachment batch index was uploaded more than once".to_string());
        }
        let projected_bytes = batch.actual_bytes.saturating_add(bytes);
        if projected_bytes > MAX_ATTACHMENT_TOTAL_BYTES as u64
            || projected_bytes > batch.total_bytes
        {
            return Err("attachments exceed the 50 MB message limit".to_string());
        }
        batch.reserved_indexes.insert(metadata.batch_index);
        batch.actual_bytes = projected_bytes;
        Ok(AttachmentUploadReservation {
            batch_id: metadata.batch_id.clone(),
            index: metadata.batch_index,
            bytes,
        })
    }

    pub(crate) fn rollback(&mut self, reservation: AttachmentUploadReservation) {
        let Some(batch) = self.batches.get_mut(&reservation.batch_id) else {
            return;
        };
        if batch.reserved_indexes.remove(&reservation.index) {
            batch.actual_bytes = batch.actual_bytes.saturating_sub(reservation.bytes);
        }
        if batch.reserved_indexes.is_empty() && batch.completed_paths.is_empty() {
            self.batches.remove(&reservation.batch_id);
        }
    }

    pub(crate) fn complete(
        &mut self,
        reservation: AttachmentUploadReservation,
        path: PathBuf,
    ) -> Result<(), String> {
        let batch = self
            .batches
            .get_mut(&reservation.batch_id)
            .ok_or_else(|| "attachment batch is no longer active".to_string())?;
        if !batch.reserved_indexes.remove(&reservation.index) {
            return Err("attachment batch reservation is missing".to_string());
        }
        batch.completed_paths.insert(reservation.index, path);
        if batch.completed_paths.len() == batch.file_sizes.len() {
            if batch.actual_bytes != batch.total_bytes {
                return Err("attachment batch actual size does not match its manifest".to_string());
            }
            self.batches.remove(&reservation.batch_id);
        }
        Ok(())
    }

    pub(crate) fn abort(&mut self, session_id: &str, batch_id: &str) -> Vec<PathBuf> {
        let matches_session = self
            .batches
            .get(batch_id)
            .is_some_and(|batch| batch.session_id == session_id);
        if !matches_session {
            return Vec::new();
        }
        self.batches
            .remove(batch_id)
            .map(|batch| batch.completed_paths.into_values().collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.batches.len()
    }
}
