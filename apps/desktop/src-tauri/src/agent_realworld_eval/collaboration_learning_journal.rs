use agent_application::{
    collaboration_learning_physical_run_sha256, CollaborationLearningAggregateStatusV1,
    CollaborationLearningCensorReasonV1, CollaborationLearningCensorReceiptV1,
    CollaborationLearningComparisonBindingV1, CollaborationLearningOfflineEntryV1,
    CollaborationLearningOfflineGenesisV1, CollaborationLearningOfflineReplayV1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

const JOURNAL_MANIFEST_SCHEMA: &str = "cindx.collaboration-learning-external-journal.v1";
const GENESIS_FILE_NAME: &str = "genesis.json";
const MANIFEST_FILE_NAME: &str = "manifest.json";
const ENTRIES_DIRECTORY_NAME: &str = "entries";
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_GENESIS_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 512 * 1024;

pub(super) struct CollaborationLearningExternalJournal {
    files: JournalFiles,
    genesis: CollaborationLearningOfflineGenesisV1,
    entries: Vec<CollaborationLearningOfflineEntryV1>,
    manifest: JournalManifest,
}

pub(super) struct CollaborationLearningJournalRecovery {
    journal: CollaborationLearningExternalJournal,
    replay: CollaborationLearningOfflineReplayV1,
    synthesized_censor_sha256: Option<String>,
}

impl CollaborationLearningJournalRecovery {
    pub(super) fn replay(&self) -> &CollaborationLearningOfflineReplayV1 {
        &self.replay
    }

    pub(super) fn synthesized_censor_sha256(&self) -> Option<&str> {
        self.synthesized_censor_sha256.as_deref()
    }

    pub(super) fn into_journal(self) -> CollaborationLearningExternalJournal {
        self.journal
    }
}

impl CollaborationLearningExternalJournal {
    pub(super) fn create_new(
        root: &Path,
        genesis: CollaborationLearningOfflineGenesisV1,
    ) -> Result<Self, String> {
        let genesis_json = genesis
            .to_json()
            .map_err(|error| format!("failed to encode collaboration learning genesis: {error}"))?;
        let (files, manifest) =
            JournalFiles::create_new(root, &genesis_json, genesis.digest().to_string())?;
        Ok(Self {
            files,
            genesis,
            entries: Vec::new(),
            manifest,
        })
    }

    pub(super) fn recover(root: &Path) -> Result<CollaborationLearningJournalRecovery, String> {
        let (files, mut manifest) = JournalFiles::open(root)?;
        let genesis_json = String::from_utf8(read_private_file(
            &files.genesis_path(),
            MAX_GENESIS_BYTES,
            "collaboration learning journal genesis",
        )?)
        .map_err(|_| "collaboration learning journal genesis is not UTF-8".to_string())?;
        let genesis = CollaborationLearningOfflineGenesisV1::from_json(&genesis_json)
            .map_err(|error| format!("failed to decode collaboration learning genesis: {error}"))?;
        if genesis.digest() != manifest.genesis_sha256 {
            return Err("collaboration learning journal genesis digest is invalid".to_string());
        }

        let mut entries = files.read_entries()?;
        let committed_count = usize::from(manifest.entry_count);
        if entries.len() < committed_count {
            return Err("collaboration learning journal is missing a committed entry".to_string());
        }
        if entries.len() > committed_count.saturating_add(1) {
            return Err("collaboration learning journal contains an entry fork".to_string());
        }
        validate_manifest_head(&genesis, &entries[..committed_count], &manifest)?;

        let mut synthesized_censor_sha256 = None;
        if entries.len() == committed_count + 1 {
            require_collecting_prefix(&genesis, &entries[..committed_count])?;
            let orphan = entries.last().expect("orphan entry exists");
            if let Some(pending) = &manifest.pending {
                validate_pending_successor(pending, orphan)?;
                if orphan.digest() == pending.recovery_entry_sha256 {
                    synthesized_censor_sha256 = Some(orphan.digest().to_string());
                }
            }
            CollaborationLearningOfflineReplayV1::reconstruct(&genesis, &entries).map_err(
                |error| format!("collaboration learning orphan successor is invalid: {error}"),
            )?;
            advance_manifest(&mut manifest, orphan)?;
            files.write_manifest(&manifest)?;
        } else if let Some(pending) = manifest.pending.clone() {
            require_collecting_prefix(&genesis, &entries)?;
            let recovery_entry = decode_pending_recovery(&pending)?;
            let mut recovered_entries = entries.clone();
            recovered_entries.push(recovery_entry.clone());
            CollaborationLearningOfflineReplayV1::reconstruct(&genesis, &recovered_entries)
                .map_err(|error| {
                    format!("collaboration learning recovery censor is invalid: {error}")
                })?;
            files.write_entry(
                recovery_entry.sequence(),
                recovery_entry.digest(),
                &pending.recovery_entry_json,
            )?;
            advance_manifest(&mut manifest, &recovery_entry)?;
            files.write_manifest(&manifest)?;
            synthesized_censor_sha256 = Some(recovery_entry.digest().to_string());
            entries = recovered_entries;
        }

        let replay = CollaborationLearningOfflineReplayV1::reconstruct(&genesis, &entries)
            .map_err(|error| format!("failed to replay collaboration learning journal: {error}"))?;
        Ok(CollaborationLearningJournalRecovery {
            journal: Self {
                files,
                genesis,
                entries,
                manifest,
            },
            replay,
            synthesized_censor_sha256,
        })
    }

    pub(super) fn append_entry(
        &mut self,
        entry: CollaborationLearningOfflineEntryV1,
    ) -> Result<(), String> {
        if self.is_exact_committed_retry(&entry)? {
            return Ok(());
        }
        if self.manifest.pending.is_some() {
            return Err(
                "collaboration learning pending capture must be completed explicitly".to_string(),
            );
        }
        self.append_successor(entry)
    }

    pub(super) fn record_pending(
        &mut self,
        binding: CollaborationLearningComparisonBindingV1,
        source_sha256: String,
        physical_agent_run_ids: Vec<String>,
    ) -> Result<(), String> {
        if self.manifest.pending.is_some() {
            return Err("collaboration learning journal already has a pending capture".to_string());
        }
        require_collecting_prefix(&self.genesis, &self.entries)?;
        let physical_run_sha256 = physical_agent_run_ids
            .iter()
            .map(|run_id| {
                collaboration_learning_physical_run_sha256(run_id).map_err(|error| {
                    format!("failed to bind collaboration learning physical run: {error}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let censor = CollaborationLearningCensorReceiptV1::for_physical_runs(
            binding,
            source_sha256,
            physical_run_sha256,
            CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
        )
        .map_err(|error| {
            format!("failed to construct collaboration learning recovery censor: {error}")
        })?;
        let sequence = self.next_sequence()?;
        let previous_entry_sha256 = self.manifest.head_entry_sha256.clone();
        let recovery_entry = CollaborationLearningOfflineEntryV1::censor(
            self.genesis.digest().to_string(),
            sequence,
            previous_entry_sha256.clone(),
            censor,
        )
        .map_err(|error| {
            format!("failed to construct collaboration learning recovery entry: {error}")
        })?;
        let recovery_entry_json = recovery_entry.to_json().map_err(|error| {
            format!("failed to encode collaboration learning recovery entry: {error}")
        })?;
        let mut candidates = self.entries.clone();
        candidates.push(recovery_entry.clone());
        CollaborationLearningOfflineReplayV1::reconstruct(&self.genesis, &candidates).map_err(
            |error| format!("collaboration learning pending capture is invalid: {error}"),
        )?;
        let mut next_manifest = self.manifest.clone();
        next_manifest.pending = Some(PendingRecoveryRecord {
            sequence,
            previous_entry_sha256,
            recovery_entry_sha256: recovery_entry.digest().to_string(),
            recovery_entry_json,
        });
        next_manifest.reseal()?;
        self.files.write_manifest(&next_manifest)?;
        self.manifest = next_manifest;
        Ok(())
    }

    pub(super) fn complete_pending(
        &mut self,
        entry: CollaborationLearningOfflineEntryV1,
    ) -> Result<(), String> {
        if self.is_exact_committed_retry(&entry)? {
            return Ok(());
        }
        let pending = self.manifest.pending.as_ref().ok_or_else(|| {
            "collaboration learning capture has no pending reservation".to_string()
        })?;
        validate_pending_successor(pending, &entry)?;
        self.append_successor(entry)
    }

    pub(super) fn replay(&self) -> Result<CollaborationLearningOfflineReplayV1, String> {
        CollaborationLearningOfflineReplayV1::reconstruct(&self.genesis, &self.entries)
            .map_err(|error| format!("failed to replay collaboration learning journal: {error}"))
    }

    pub(super) fn next_sequence(&self) -> Result<u16, String> {
        self.manifest
            .entry_count
            .checked_add(1)
            .ok_or_else(|| "collaboration learning journal sequence is exhausted".to_string())
    }

    pub(super) fn head_sha256(&self) -> Option<&str> {
        self.manifest.head_entry_sha256.as_deref()
    }

    fn is_exact_committed_retry(
        &self,
        entry: &CollaborationLearningOfflineEntryV1,
    ) -> Result<bool, String> {
        if entry.sequence() != self.manifest.entry_count {
            return Ok(false);
        }
        validate_manifest_head(&self.genesis, &self.entries, &self.manifest)?;
        let committed = self.entries.last().ok_or_else(|| {
            "collaboration learning journal committed retry has no in-memory entry".to_string()
        })?;
        if self.manifest.head_entry_sha256.as_deref() != Some(entry.digest())
            || committed.sequence() != entry.sequence()
            || committed.digest() != entry.digest()
        {
            return Err(
                "collaboration learning journal committed entry conflicts with its retry"
                    .to_string(),
            );
        }
        let retry_json = entry.to_json().map_err(|error| {
            format!("failed to encode collaboration learning committed retry: {error}")
        })?;
        let committed_json = committed.to_json().map_err(|error| {
            format!("failed to encode collaboration learning committed entry: {error}")
        })?;
        let disk_entries = self.files.read_entries()?;
        if committed_json != retry_json
            || disk_entries.len() != self.entries.len()
            || disk_entries
                .iter()
                .zip(&self.entries)
                .any(|(disk, memory)| {
                    disk.sequence() != memory.sequence() || disk.digest() != memory.digest()
                })
        {
            return Err(
                "collaboration learning journal committed retry disagrees with durable state"
                    .to_string(),
            );
        }
        let durable_bytes = read_private_file(
            &self.files.entry_path(entry.sequence(), entry.digest()),
            MAX_ENTRY_BYTES,
            "collaboration learning committed journal entry",
        )?;
        if durable_bytes != retry_json.as_bytes() {
            return Err(
                "collaboration learning journal committed entry is not canonical on disk"
                    .to_string(),
            );
        }
        Ok(true)
    }

    fn append_successor(
        &mut self,
        entry: CollaborationLearningOfflineEntryV1,
    ) -> Result<(), String> {
        validate_successor(&self.genesis, &self.entries, &self.manifest, &entry)?;
        let entry_json = entry.to_json().map_err(|error| {
            format!("failed to encode collaboration learning journal entry: {error}")
        })?;
        if !self.has_exact_durable_orphan(&entry, &entry_json)? {
            self.files
                .write_entry(entry.sequence(), entry.digest(), &entry_json)?;
        }
        let mut next_manifest = self.manifest.clone();
        advance_manifest(&mut next_manifest, &entry)?;
        self.files.write_manifest(&next_manifest)?;
        self.manifest = next_manifest;
        self.entries.push(entry);
        Ok(())
    }

    fn has_exact_durable_orphan(
        &self,
        entry: &CollaborationLearningOfflineEntryV1,
        entry_json: &str,
    ) -> Result<bool, String> {
        let durable_entries = self.files.read_entries()?;
        if durable_entries.len() < self.entries.len()
            || durable_entries.len() > self.entries.len().saturating_add(1)
            || durable_entries
                .iter()
                .zip(&self.entries)
                .any(|(durable, memory)| {
                    durable.sequence() != memory.sequence() || durable.digest() != memory.digest()
                })
        {
            return Err(
                "collaboration learning journal durable entries disagree with committed state"
                    .to_string(),
            );
        }
        if durable_entries.len() == self.entries.len() {
            return Ok(false);
        }
        let orphan = durable_entries.last().expect("durable orphan exists");
        if orphan.sequence() != entry.sequence() || orphan.digest() != entry.digest() {
            return Err(
                "collaboration learning journal orphan conflicts with its exact retry".to_string(),
            );
        }
        let durable_bytes = read_private_file(
            &self.files.entry_path(entry.sequence(), entry.digest()),
            MAX_ENTRY_BYTES,
            "collaboration learning orphan journal entry",
        )?;
        if durable_bytes != entry_json.as_bytes() {
            return Err(
                "collaboration learning journal orphan is not the canonical exact retry"
                    .to_string(),
            );
        }
        Ok(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRecoveryRecord {
    sequence: u16,
    previous_entry_sha256: Option<String>,
    recovery_entry_sha256: String,
    recovery_entry_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalManifest {
    schema: String,
    genesis_sha256: String,
    entry_count: u16,
    head_entry_sha256: Option<String>,
    pending: Option<PendingRecoveryRecord>,
    manifest_sha256: String,
}

impl JournalManifest {
    fn new(genesis_sha256: String) -> Result<Self, String> {
        require_sha256(&genesis_sha256, "journal genesis")?;
        let mut manifest = Self {
            schema: JOURNAL_MANIFEST_SCHEMA.to_string(),
            genesis_sha256,
            entry_count: 0,
            head_entry_sha256: None,
            pending: None,
            manifest_sha256: String::new(),
        };
        manifest.reseal()?;
        Ok(manifest)
    }

    fn reseal(&mut self) -> Result<(), String> {
        self.manifest_sha256 = self.payload_sha256()?;
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != JOURNAL_MANIFEST_SCHEMA {
            return Err("collaboration learning journal manifest schema is invalid".to_string());
        }
        require_sha256(&self.genesis_sha256, "journal genesis")?;
        if self.entry_count == 0 && self.head_entry_sha256.is_some()
            || self.entry_count > 0 && self.head_entry_sha256.is_none()
        {
            return Err("collaboration learning journal manifest head is inconsistent".to_string());
        }
        if let Some(head) = &self.head_entry_sha256 {
            require_sha256(head, "journal head")?;
        }
        if let Some(pending) = &self.pending {
            if self.entry_count.checked_add(1) != Some(pending.sequence)
                || pending.previous_entry_sha256 != self.head_entry_sha256
            {
                return Err(
                    "collaboration learning pending recovery is not the next entry".to_string(),
                );
            }
            require_sha256(&pending.recovery_entry_sha256, "pending recovery entry")?;
            if pending.recovery_entry_json.is_empty()
                || pending.recovery_entry_json.len() as u64 > MAX_ENTRY_BYTES
            {
                return Err(
                    "collaboration learning pending recovery payload is outside its bound"
                        .to_string(),
                );
            }
        }
        require_sha256(&self.manifest_sha256, "journal manifest")?;
        if self.manifest_sha256 != self.payload_sha256()? {
            return Err("collaboration learning journal manifest digest is invalid".to_string());
        }
        Ok(())
    }

    fn payload_sha256(&self) -> Result<String, String> {
        digest_json(
            b"cindx.collaboration-learning-external-journal.v1\0",
            &(
                self.schema.as_str(),
                self.genesis_sha256.as_str(),
                self.entry_count,
                self.head_entry_sha256.as_deref(),
                &self.pending,
            ),
            "collaboration learning journal manifest",
        )
    }
}

struct JournalFiles {
    root: PathBuf,
}

impl JournalFiles {
    fn create_new(
        root: &Path,
        genesis_json: &str,
        genesis_sha256: String,
    ) -> Result<(Self, JournalManifest), String> {
        if !root.is_absolute() {
            return Err("collaboration learning journal path must be absolute".to_string());
        }
        create_private_directory_new(root, "collaboration learning journal")?;
        let files = Self {
            root: root.to_path_buf(),
        };
        let result = (|| {
            create_private_directory_new(
                &files.entries_path(),
                "collaboration learning journal entries",
            )?;
            write_immutable_private_file(
                &files.genesis_path(),
                genesis_json.as_bytes(),
                "collaboration learning journal genesis",
            )?;
            let manifest = JournalManifest::new(genesis_sha256)?;
            files.write_manifest(&manifest)?;
            Ok(manifest)
        })();
        result.map(|manifest| (files, manifest))
    }

    fn open(root: &Path) -> Result<(Self, JournalManifest), String> {
        if !root.is_absolute() {
            return Err("collaboration learning journal path must be absolute".to_string());
        }
        require_private_directory(root, "collaboration learning journal")?;
        let files = Self {
            root: root.to_path_buf(),
        };
        require_private_directory(
            &files.entries_path(),
            "collaboration learning journal entries",
        )?;
        let bytes = read_private_file(
            &files.manifest_path(),
            MAX_MANIFEST_BYTES,
            "collaboration learning journal manifest",
        )?;
        let manifest: JournalManifest = serde_json::from_slice(&bytes).map_err(|error| {
            format!("failed to decode collaboration learning journal manifest: {error}")
        })?;
        manifest.validate()?;
        Ok((files, manifest))
    }

    fn genesis_path(&self) -> PathBuf {
        self.root.join(GENESIS_FILE_NAME)
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE_NAME)
    }

    fn entries_path(&self) -> PathBuf {
        self.root.join(ENTRIES_DIRECTORY_NAME)
    }

    fn entry_path(&self, sequence: u16, digest: &str) -> PathBuf {
        self.entries_path()
            .join(format!("{sequence:05}-{digest}.json"))
    }

    fn write_manifest(&self, manifest: &JournalManifest) -> Result<(), String> {
        require_private_directory(&self.root, "collaboration learning journal")?;
        manifest.validate()?;
        let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
            format!("failed to encode collaboration learning journal manifest: {error}")
        })?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(
                "collaboration learning journal manifest is outside its size bound".to_string(),
            );
        }
        tools::write_private_file_atomically(&self.manifest_path(), &bytes).map_err(|error| {
            format!("failed to persist collaboration learning journal manifest: {error}")
        })?;
        let metadata = fs::symlink_metadata(self.manifest_path()).map_err(|error| {
            format!("failed to inspect collaboration learning journal manifest: {error}")
        })?;
        require_private_mode(&metadata, "collaboration learning journal manifest")
    }

    fn write_entry(
        &self,
        sequence: u16,
        digest: &str,
        entry_json: &str,
    ) -> Result<PathBuf, String> {
        if sequence == 0 {
            return Err("collaboration learning journal entry sequence is zero".to_string());
        }
        require_sha256(digest, "collaboration learning journal entry")?;
        if entry_json.is_empty() || entry_json.len() as u64 > MAX_ENTRY_BYTES {
            return Err(
                "collaboration learning journal entry is outside its size bound".to_string(),
            );
        }
        let path = self.entry_path(sequence, digest);
        write_immutable_private_file(
            &path,
            entry_json.as_bytes(),
            "collaboration learning journal entry",
        )?;
        Ok(path)
    }

    fn entry_files(&self) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(self.entries_path()).map_err(|error| {
            format!("failed to list collaboration learning journal entries: {error}")
        })? {
            let entry = entry.map_err(|error| {
                format!("failed to inspect collaboration learning journal entry: {error}")
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err("collaboration learning journal entry name is not UTF-8".to_string());
            };
            if is_partial_file(name) {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                format!("failed to inspect collaboration learning journal entry: {error}")
            })?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err("collaboration learning journal contains a non-file entry".to_string());
            }
            require_private_mode(&metadata, "collaboration learning journal entry")?;
            paths.push(entry.path());
        }
        paths.sort();
        Ok(paths)
    }

    fn read_entries(&self) -> Result<Vec<CollaborationLearningOfflineEntryV1>, String> {
        let mut records = Vec::new();
        for path in self.entry_files()? {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    "collaboration learning journal entry name is not UTF-8".to_string()
                })?;
            let (file_sequence, file_digest) = parse_entry_file_name(name)?;
            records.push((file_sequence, file_digest.to_string(), path));
        }
        records.sort_by_key(|(sequence, _, _)| *sequence);
        if records.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("collaboration learning journal contains an entry fork".to_string());
        }

        let mut entries = Vec::with_capacity(records.len());
        for (file_sequence, file_digest, path) in records {
            let encoded = String::from_utf8(read_private_file(
                &path,
                MAX_ENTRY_BYTES,
                "collaboration learning journal entry",
            )?)
            .map_err(|_| "collaboration learning journal entry is not UTF-8".to_string())?;
            let entry =
                CollaborationLearningOfflineEntryV1::from_json(&encoded).map_err(|error| {
                    format!("failed to decode collaboration learning journal entry: {error}")
                })?;
            if entry.sequence() != file_sequence || entry.digest() != file_digest {
                return Err(
                    "collaboration learning journal entry name does not match its payload"
                        .to_string(),
                );
            }
            entries.push(entry);
        }
        Ok(entries)
    }
}

fn validate_manifest_head(
    genesis: &CollaborationLearningOfflineGenesisV1,
    entries: &[CollaborationLearningOfflineEntryV1],
    manifest: &JournalManifest,
) -> Result<(), String> {
    let replay = CollaborationLearningOfflineReplayV1::reconstruct(genesis, entries)
        .map_err(|error| format!("collaboration learning committed journal is invalid: {error}"))?;
    if replay.entry_count() != manifest.entry_count
        || replay.head_sha256() != manifest.head_entry_sha256.as_deref()
    {
        return Err(
            "collaboration learning journal manifest does not match its committed entries"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_successor(
    genesis: &CollaborationLearningOfflineGenesisV1,
    entries: &[CollaborationLearningOfflineEntryV1],
    manifest: &JournalManifest,
    entry: &CollaborationLearningOfflineEntryV1,
) -> Result<(), String> {
    validate_manifest_head(genesis, entries, manifest)?;
    require_collecting_prefix(genesis, entries)?;
    let expected_sequence = manifest
        .entry_count
        .checked_add(1)
        .ok_or_else(|| "collaboration learning journal sequence is exhausted".to_string())?;
    if entry.genesis_sha256() != genesis.digest()
        || entry.sequence() != expected_sequence
        || entry.previous_entry_sha256() != manifest.head_entry_sha256.as_deref()
    {
        return Err("collaboration learning journal successor is not contiguous".to_string());
    }
    if let Some(pending) = &manifest.pending {
        validate_pending_successor(pending, entry)?;
    }
    let mut candidates = entries.to_vec();
    candidates.push(entry.clone());
    CollaborationLearningOfflineReplayV1::reconstruct(genesis, &candidates)
        .map_err(|error| format!("collaboration learning journal successor is invalid: {error}"))?;
    Ok(())
}

fn require_collecting_prefix(
    genesis: &CollaborationLearningOfflineGenesisV1,
    entries: &[CollaborationLearningOfflineEntryV1],
) -> Result<(), String> {
    let replay = CollaborationLearningOfflineReplayV1::reconstruct(genesis, entries)
        .map_err(|error| format!("collaboration learning journal prefix is invalid: {error}"))?;
    if replay.aggregate().status() != CollaborationLearningAggregateStatusV1::Collecting {
        return Err(
            "collaboration learning journal cannot continue a terminal aggregate".to_string(),
        );
    }
    Ok(())
}

fn validate_pending_successor(
    pending: &PendingRecoveryRecord,
    entry: &CollaborationLearningOfflineEntryV1,
) -> Result<(), String> {
    let recovery_entry = decode_pending_recovery(pending)?;
    let pending_physical_runs = recovery_entry.physical_run_sha256().map_err(|error| {
        format!("collaboration learning pending physical runs are invalid: {error}")
    })?;
    let successor_physical_runs = entry.physical_run_sha256().map_err(|error| {
        format!("collaboration learning successor physical runs are invalid: {error}")
    })?;
    if entry.sequence() != pending.sequence
        || entry.previous_entry_sha256() != pending.previous_entry_sha256.as_deref()
        || entry.binding_sha256() != recovery_entry.binding_sha256()
        || successor_physical_runs != pending_physical_runs
    {
        return Err(
            "collaboration learning successor does not match its pending capture".to_string(),
        );
    }
    Ok(())
}

fn decode_pending_recovery(
    pending: &PendingRecoveryRecord,
) -> Result<CollaborationLearningOfflineEntryV1, String> {
    let entry = CollaborationLearningOfflineEntryV1::from_json(&pending.recovery_entry_json)
        .map_err(|error| {
            format!("failed to decode collaboration learning pending recovery: {error}")
        })?;
    if entry.sequence() != pending.sequence
        || entry.previous_entry_sha256() != pending.previous_entry_sha256.as_deref()
        || entry.digest() != pending.recovery_entry_sha256
        || entry.censor_reason()
            != Some(CollaborationLearningCensorReasonV1::IncompleteInstrumentation)
    {
        return Err("collaboration learning pending recovery is invalid".to_string());
    }
    Ok(entry)
}

fn advance_manifest(
    manifest: &mut JournalManifest,
    entry: &CollaborationLearningOfflineEntryV1,
) -> Result<(), String> {
    let expected_sequence = manifest
        .entry_count
        .checked_add(1)
        .ok_or_else(|| "collaboration learning journal sequence is exhausted".to_string())?;
    if entry.sequence() != expected_sequence
        || entry.previous_entry_sha256() != manifest.head_entry_sha256.as_deref()
    {
        return Err("collaboration learning journal successor is not contiguous".to_string());
    }
    manifest.entry_count = entry.sequence();
    manifest.head_entry_sha256 = Some(entry.digest().to_string());
    manifest.pending = None;
    manifest.reseal()
}

fn parse_entry_file_name(name: &str) -> Result<(u16, &str), String> {
    let stem = name.strip_suffix(".json").ok_or_else(|| {
        "collaboration learning journal entry name has an invalid suffix".to_string()
    })?;
    let (sequence, digest) = stem.split_once('-').ok_or_else(|| {
        "collaboration learning journal entry name has an invalid shape".to_string()
    })?;
    if sequence.len() != 5 || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(
            "collaboration learning journal entry name has an invalid sequence".to_string(),
        );
    }
    let sequence = sequence.parse::<u16>().map_err(|_| {
        "collaboration learning journal entry name has an invalid sequence".to_string()
    })?;
    if sequence == 0 {
        return Err("collaboration learning journal entry name has a zero sequence".to_string());
    }
    require_sha256(digest, "collaboration learning journal entry name")?;
    Ok((sequence, digest))
}

fn create_private_directory_new(path: &Path, label: &str) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|error| format!("failed to create {label}: {error}"))?;
    require_private_directory(path, label)
}

fn require_private_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{label} is not a private directory"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("{label} permissions are not private"));
    }
    Ok(())
}

fn write_immutable_private_file(path: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} path has no parent"))?;
    require_private_directory(parent, &format!("{label} parent"))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("record");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| format!("failed to create temporary {label}: {error}"))?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure temporary {label}: {error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("failed to write temporary {label}: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("failed to sync temporary {label}: {error}"))?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| format!("failed to publish immutable {label}: {}", error.error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect immutable {label}: {error}"))?;
    require_private_mode(&metadata, label)?;
    sync_directory(parent, label)
}

fn read_private_file(path: &Path, maximum_bytes: u64, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{label} is not a regular file"));
    }
    require_private_mode(&metadata, label)?;
    if metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(format!("{label} is outside its size bound"));
    }
    fs::read(path).map_err(|error| format!("failed to read {label}: {error}"))
}

fn require_private_mode(metadata: &fs::Metadata, label: &str) -> Result<(), String> {
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(format!("{label} permissions are not 0600"));
    }
    Ok(())
}

fn sync_directory(path: &Path, label: &str) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed to sync {label} directory: {error}"))
}

fn is_partial_file(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(".tmp")
}

fn require_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label} digest is invalid"));
    }
    Ok(())
}

fn digest_json<T: Serialize>(domain: &[u8], payload: &T, label: &str) -> Result<String, String> {
    let encoded = serde_json::to_vec(payload)
        .map_err(|error| format!("failed to encode {label}: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
#[path = "collaboration_learning_journal_tests.rs"]
mod tests;
