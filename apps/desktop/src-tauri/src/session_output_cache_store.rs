use agent_application::AgentOutputArtifact as AgentOutputArtifactView;
use std::{collections::BTreeMap, sync::Mutex};

const SESSION_OUTPUT_CACHE_LIMIT: usize = 32;

#[derive(Debug, Clone)]
struct SessionOutputCacheEntry {
    event_count: u64,
    latest_sequence: u64,
    outputs: Vec<AgentOutputArtifactView>,
    last_accessed_at_ms: u64,
}

pub(crate) enum SessionOutputCacheLookup {
    Hit(Vec<AgentOutputArtifactView>),
    Miss {
        event_count: u64,
        latest_sequence: u64,
        outputs: Vec<AgentOutputArtifactView>,
    },
    Empty,
}

#[derive(Default)]
pub(crate) struct SessionOutputCache {
    entries: Mutex<BTreeMap<String, SessionOutputCacheEntry>>,
}

impl SessionOutputCache {
    pub(crate) fn lookup(
        &self,
        session_id: &str,
        event_count: u64,
        latest_sequence: u64,
        now_ms: u64,
    ) -> Result<SessionOutputCacheLookup, String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|error| format!("session output cache lock poisoned: {error}"))?;
        if let Some(entry) = entries.get_mut(session_id) {
            if entry.event_count == event_count && entry.latest_sequence == latest_sequence {
                entry.last_accessed_at_ms = now_ms;
                return Ok(SessionOutputCacheLookup::Hit(entry.outputs.clone()));
            }
        }
        Ok(match entries.remove(session_id) {
            Some(entry) => SessionOutputCacheLookup::Miss {
                event_count: entry.event_count,
                latest_sequence: entry.latest_sequence,
                outputs: entry.outputs,
            },
            None => SessionOutputCacheLookup::Empty,
        })
    }

    pub(crate) fn store(
        &self,
        session_id: &str,
        event_count: u64,
        latest_sequence: u64,
        outputs: Vec<AgentOutputArtifactView>,
        now_ms: u64,
    ) -> Result<(), String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|error| format!("session output cache lock poisoned: {error}"))?;
        if !entries.contains_key(session_id) && entries.len() >= SESSION_OUTPUT_CACHE_LIMIT {
            if let Some(oldest_session_id) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed_at_ms)
                .map(|(session_id, _)| session_id.clone())
            {
                entries.remove(&oldest_session_id);
            }
        }
        entries.insert(
            session_id.to_string(),
            SessionOutputCacheEntry {
                event_count,
                latest_sequence,
                outputs,
                last_accessed_at_ms: now_ms,
            },
        );
        Ok(())
    }

    pub(crate) fn remove_many(&self, session_ids: &[String]) -> Result<(), String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|error| format!("session output cache lock poisoned: {error}"))?;
        for session_id in session_ids {
            entries.remove(session_id);
        }
        Ok(())
    }
}
