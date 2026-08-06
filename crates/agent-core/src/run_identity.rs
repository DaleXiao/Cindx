use crate::{Event, Metadata};
use std::collections::BTreeMap;
use std::fmt;

pub const AGENT_RUN_ID_METADATA_KEY: &str = "agent_run_id";
pub const LOGICAL_AGENT_RUN_ID_METADATA_KEY: &str = "logical_agent_run_id";
pub const SOURCE_AGENT_RUN_ID_METADATA_KEY: &str = "source_agent_run_id";
pub const AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY: &str = "agent_run_identity_schema";
pub const AGENT_RUN_IDENTITY_V1_SCHEMA: &str = "cindx.agent-run-identity.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunIdentity {
    logical_run_id: String,
    attempt_run_id: String,
    source_attempt_run_id: Option<String>,
}

impl AgentRunIdentity {
    pub fn new(
        logical_run_id: impl Into<String>,
        attempt_run_id: impl Into<String>,
    ) -> Result<Self, AgentRunIdentityError> {
        let logical_run_id = logical_run_id.into();
        let attempt_run_id = attempt_run_id.into();
        require_non_empty(LOGICAL_AGENT_RUN_ID_METADATA_KEY, &logical_run_id)?;
        require_non_empty(AGENT_RUN_ID_METADATA_KEY, &attempt_run_id)?;
        Ok(Self {
            logical_run_id,
            attempt_run_id,
            source_attempt_run_id: None,
        })
    }

    pub fn continuation(
        logical_run_id: impl Into<String>,
        attempt_run_id: impl Into<String>,
        source_attempt_run_id: impl Into<String>,
    ) -> Result<Self, AgentRunIdentityError> {
        Self::new(logical_run_id, attempt_run_id)?.with_source_attempt_run_id(source_attempt_run_id)
    }

    pub fn with_source_attempt_run_id(
        mut self,
        source_attempt_run_id: impl Into<String>,
    ) -> Result<Self, AgentRunIdentityError> {
        let source_attempt_run_id = source_attempt_run_id.into();
        require_non_empty(SOURCE_AGENT_RUN_ID_METADATA_KEY, &source_attempt_run_id)?;
        self.source_attempt_run_id = Some(source_attempt_run_id);
        Ok(self)
    }

    pub fn from_metadata(metadata: &Metadata) -> Result<Option<Self>, AgentRunIdentityError> {
        let schema = metadata.get(AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY);
        let logical_run_id = metadata.get(LOGICAL_AGENT_RUN_ID_METADATA_KEY);
        let source_attempt_run_id = metadata.get(SOURCE_AGENT_RUN_ID_METADATA_KEY);

        if schema.is_none() && logical_run_id.is_none() {
            return Ok(None);
        }
        match schema.map(String::as_str) {
            Some(AGENT_RUN_IDENTITY_V1_SCHEMA) => {}
            Some(other) => {
                return Err(AgentRunIdentityError::UnsupportedSchema(other.to_string()));
            }
            None => return Err(AgentRunIdentityError::MissingVersionSchema),
        }
        let logical_run_id = required_metadata_value(metadata, LOGICAL_AGENT_RUN_ID_METADATA_KEY)?;
        let attempt_run_id = required_metadata_value(metadata, AGENT_RUN_ID_METADATA_KEY)?;
        let source_attempt_run_id = source_attempt_run_id
            .map(|value| {
                require_non_empty(SOURCE_AGENT_RUN_ID_METADATA_KEY, value)?;
                Ok(value.clone())
            })
            .transpose()?;

        Ok(Some(Self {
            logical_run_id: logical_run_id.clone(),
            attempt_run_id: attempt_run_id.clone(),
            source_attempt_run_id,
        }))
    }

    pub fn insert_into(&self, metadata: &mut Metadata) -> Result<(), AgentRunIdentityError> {
        validate_reserved(
            metadata,
            AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY,
            AGENT_RUN_IDENTITY_V1_SCHEMA,
        )?;
        validate_reserved(
            metadata,
            LOGICAL_AGENT_RUN_ID_METADATA_KEY,
            &self.logical_run_id,
        )?;
        validate_reserved(metadata, AGENT_RUN_ID_METADATA_KEY, &self.attempt_run_id)?;
        match &self.source_attempt_run_id {
            Some(source_attempt_run_id) => validate_reserved(
                metadata,
                SOURCE_AGENT_RUN_ID_METADATA_KEY,
                source_attempt_run_id,
            )?,
            None if metadata.contains_key(SOURCE_AGENT_RUN_ID_METADATA_KEY) => Err(
                AgentRunIdentityError::ReservedMetadataConflict(SOURCE_AGENT_RUN_ID_METADATA_KEY),
            )?,
            None => {}
        }
        metadata.insert(
            AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
            AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
        );
        metadata.insert(
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            self.logical_run_id.clone(),
        );
        metadata.insert(
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            self.attempt_run_id.clone(),
        );
        if let Some(source_attempt_run_id) = &self.source_attempt_run_id {
            metadata.insert(
                SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
                source_attempt_run_id.clone(),
            );
        }
        Ok(())
    }

    pub fn logical_run_id(&self) -> &str {
        &self.logical_run_id
    }

    pub fn attempt_run_id(&self) -> &str {
        &self.attempt_run_id
    }

    pub fn source_attempt_run_id(&self) -> Option<&str> {
        self.source_attempt_run_id.as_deref()
    }
}

pub fn agent_run_id(metadata: &Metadata) -> Option<&str> {
    metadata.get(AGENT_RUN_ID_METADATA_KEY).map(String::as_str)
}

pub fn logical_agent_run_id(metadata: &Metadata) -> Option<&str> {
    metadata
        .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
        .map(String::as_str)
}

pub fn source_agent_run_id(metadata: &Metadata) -> Option<&str> {
    metadata
        .get(SOURCE_AGENT_RUN_ID_METADATA_KEY)
        .map(String::as_str)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AgentRunScope {
    task_id: String,
    project_id: Option<String>,
    session_id: Option<String>,
}

impl AgentRunScope {
    fn from_event(event: &Event) -> Self {
        Self {
            task_id: event.task_id.0.clone(),
            project_id: event.metadata.get("project_id").cloned(),
            session_id: event.metadata.get("session_id").cloned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttemptNode {
    scope: AgentRunScope,
    explicit_logical_run_id: Option<String>,
    source_attempt_run_id: Option<String>,
}

/// A validated, memoized projection of physical attempts onto logical runs.
///
/// Versioned logical IDs are authoritative. Legacy attempts are resolved by
/// following `source_agent_run_id` only within the same task/project/session.
/// Conflicts, missing legacy sources, cross-scope edges and cycles fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunLineage {
    attempts: BTreeMap<String, AttemptNode>,
    logical_run_ids: BTreeMap<String, String>,
}

impl AgentRunLineage {
    pub fn from_events(events: &[Event]) -> Result<Self, AgentRunIdentityError> {
        let mut attempts = BTreeMap::<String, AttemptNode>::new();
        for event in events {
            let explicit_identity = AgentRunIdentity::from_metadata(&event.metadata)?;
            let Some(attempt_run_id) = agent_run_id(&event.metadata) else {
                if source_agent_run_id(&event.metadata).is_some() {
                    return Err(AgentRunIdentityError::MissingMetadataValue(
                        AGENT_RUN_ID_METADATA_KEY,
                    ));
                }
                continue;
            };
            require_non_empty(AGENT_RUN_ID_METADATA_KEY, attempt_run_id)?;
            let source_attempt_run_id = source_agent_run_id(&event.metadata)
                .map(|source_attempt_run_id| {
                    require_non_empty(SOURCE_AGENT_RUN_ID_METADATA_KEY, source_attempt_run_id)?;
                    Ok(source_attempt_run_id.to_string())
                })
                .transpose()?
                // Legacy same-attempt recovery used this key to identify the
                // physical replay source. It is not a continuation edge.
                .filter(|source_attempt_run_id| source_attempt_run_id != attempt_run_id);
            let explicit_logical_run_id = explicit_identity
                .as_ref()
                .map(|identity| identity.logical_run_id.clone());
            let scope = AgentRunScope::from_event(event);

            match attempts.get_mut(attempt_run_id) {
                Some(existing) => {
                    if existing.scope != scope {
                        return Err(AgentRunIdentityError::AttemptCrossesScope(
                            attempt_run_id.to_string(),
                        ));
                    }
                    merge_optional_value(
                        &mut existing.explicit_logical_run_id,
                        explicit_logical_run_id,
                        || {
                            AgentRunIdentityError::ConflictingLogicalRunId(
                                attempt_run_id.to_string(),
                            )
                        },
                    )?;
                    if existing.explicit_logical_run_id.is_some() {
                        existing.source_attempt_run_id = None;
                    } else {
                        merge_optional_value(
                            &mut existing.source_attempt_run_id,
                            source_attempt_run_id,
                            || {
                                AgentRunIdentityError::ConflictingSourceAttempt(
                                    attempt_run_id.to_string(),
                                )
                            },
                        )?;
                    }
                }
                None => {
                    let source_attempt_run_id = if explicit_logical_run_id.is_some() {
                        None
                    } else {
                        source_attempt_run_id
                    };
                    attempts.insert(
                        attempt_run_id.to_string(),
                        AttemptNode {
                            scope,
                            explicit_logical_run_id,
                            source_attempt_run_id,
                        },
                    );
                }
            }
        }

        let mut logical_run_ids = BTreeMap::new();
        let mut visit_state = BTreeMap::<String, VisitState>::new();
        for attempt_run_id in attempts.keys() {
            resolve_attempt(
                attempt_run_id,
                &attempts,
                &mut logical_run_ids,
                &mut visit_state,
            )?;
        }
        let mut logical_scopes = BTreeMap::<String, AgentRunScope>::new();
        for (attempt_run_id, logical_run_id) in &logical_run_ids {
            let scope = &attempts
                .get(attempt_run_id)
                .expect("resolved Agent attempt must retain its scope")
                .scope;
            match logical_scopes.get(logical_run_id) {
                Some(existing) if existing != scope => {
                    return Err(AgentRunIdentityError::LogicalRunCrossesScope(
                        logical_run_id.clone(),
                    ));
                }
                Some(_) => {}
                None => {
                    logical_scopes.insert(logical_run_id.clone(), scope.clone());
                }
            }
        }
        Ok(Self {
            attempts,
            logical_run_ids,
        })
    }

    pub fn logical_run_id_for_attempt(&self, attempt_run_id: &str) -> Option<&str> {
        self.logical_run_ids.get(attempt_run_id).map(String::as_str)
    }

    pub fn logical_run_id_for_event(
        &self,
        event: &Event,
    ) -> Result<Option<&str>, AgentRunIdentityError> {
        let Some(attempt_run_id) = agent_run_id(&event.metadata) else {
            return Ok(None);
        };
        let Some(node) = self.attempts.get(attempt_run_id) else {
            return Err(AgentRunIdentityError::UnknownAttempt(
                attempt_run_id.to_string(),
            ));
        };
        if node.scope != AgentRunScope::from_event(event) {
            return Err(AgentRunIdentityError::AttemptCrossesScope(
                attempt_run_id.to_string(),
            ));
        }
        Ok(self.logical_run_id_for_attempt(attempt_run_id))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Resolved,
}

fn resolve_attempt(
    attempt_run_id: &str,
    attempts: &BTreeMap<String, AttemptNode>,
    logical_run_ids: &mut BTreeMap<String, String>,
    visit_state: &mut BTreeMap<String, VisitState>,
) -> Result<String, AgentRunIdentityError> {
    if let Some(logical_run_id) = logical_run_ids.get(attempt_run_id) {
        return Ok(logical_run_id.clone());
    }
    if visit_state.get(attempt_run_id) == Some(&VisitState::Visiting) {
        return Err(AgentRunIdentityError::LineageCycle(
            attempt_run_id.to_string(),
        ));
    }
    let node = attempts
        .get(attempt_run_id)
        .ok_or_else(|| AgentRunIdentityError::UnknownAttempt(attempt_run_id.to_string()))?;
    visit_state.insert(attempt_run_id.to_string(), VisitState::Visiting);

    if let Some(explicit_logical_run_id) = &node.explicit_logical_run_id {
        logical_run_ids.insert(attempt_run_id.to_string(), explicit_logical_run_id.clone());
        visit_state.insert(attempt_run_id.to_string(), VisitState::Resolved);
        return Ok(explicit_logical_run_id.clone());
    }

    let source_logical_run_id = match node.source_attempt_run_id.as_deref() {
        Some(source_attempt_run_id) => match attempts.get(source_attempt_run_id) {
            Some(source) if source.scope != node.scope => {
                return Err(AgentRunIdentityError::SourceCrossesScope {
                    attempt_run_id: attempt_run_id.to_string(),
                    source_attempt_run_id: source_attempt_run_id.to_string(),
                });
            }
            Some(_) => Some(resolve_attempt(
                source_attempt_run_id,
                attempts,
                logical_run_ids,
                visit_state,
            )?),
            None => {
                return Err(AgentRunIdentityError::MissingSourceAttempt {
                    attempt_run_id: attempt_run_id.to_string(),
                    source_attempt_run_id: source_attempt_run_id.to_string(),
                });
            }
        },
        None => None,
    };

    let logical_run_id = match source_logical_run_id {
        Some(source) => source,
        None => attempt_run_id.to_string(),
    };
    logical_run_ids.insert(attempt_run_id.to_string(), logical_run_id.clone());
    visit_state.insert(attempt_run_id.to_string(), VisitState::Resolved);
    Ok(logical_run_id)
}

fn merge_optional_value<T: PartialEq>(
    target: &mut Option<T>,
    candidate: Option<T>,
    conflict: impl FnOnce() -> AgentRunIdentityError,
) -> Result<(), AgentRunIdentityError> {
    match (target.as_ref(), candidate) {
        (Some(existing), Some(candidate)) if existing != &candidate => Err(conflict()),
        (None, Some(candidate)) => {
            *target = Some(candidate);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn required_metadata_value<'a>(
    metadata: &'a Metadata,
    key: &'static str,
) -> Result<&'a String, AgentRunIdentityError> {
    let value = metadata
        .get(key)
        .ok_or(AgentRunIdentityError::MissingMetadataValue(key))?;
    require_non_empty(key, value)?;
    Ok(value)
}

fn require_non_empty(key: &'static str, value: &str) -> Result<(), AgentRunIdentityError> {
    if value.trim().is_empty() {
        Err(AgentRunIdentityError::EmptyMetadataValue(key))
    } else {
        Ok(())
    }
}

fn validate_reserved(
    metadata: &Metadata,
    key: &'static str,
    value: &str,
) -> Result<(), AgentRunIdentityError> {
    match metadata.get(key) {
        Some(existing) if existing == value => Ok(()),
        Some(_) => Err(AgentRunIdentityError::ReservedMetadataConflict(key)),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunIdentityError {
    EmptyMetadataValue(&'static str),
    MissingMetadataValue(&'static str),
    MissingVersionSchema,
    UnsupportedSchema(String),
    ReservedMetadataConflict(&'static str),
    AttemptCrossesScope(String),
    LogicalRunCrossesScope(String),
    SourceCrossesScope {
        attempt_run_id: String,
        source_attempt_run_id: String,
    },
    ConflictingLogicalRunId(String),
    ConflictingSourceAttempt(String),
    MissingSourceAttempt {
        attempt_run_id: String,
        source_attempt_run_id: String,
    },
    LineageCycle(String),
    UnknownAttempt(String),
}

impl fmt::Display for AgentRunIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMetadataValue(key) => write!(formatter, "metadata key `{key}` is empty"),
            Self::MissingMetadataValue(key) => write!(formatter, "metadata key `{key}` is missing"),
            Self::MissingVersionSchema => write!(formatter, "Agent run identity schema is missing"),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported Agent run identity schema `{schema}`")
            }
            Self::ReservedMetadataConflict(key) => {
                write!(formatter, "metadata key `{key}` is reserved")
            }
            Self::AttemptCrossesScope(attempt) => {
                write!(formatter, "Agent attempt `{attempt}` crosses run scope")
            }
            Self::LogicalRunCrossesScope(logical_run_id) => write!(
                formatter,
                "logical Agent run `{logical_run_id}` crosses run scope"
            ),
            Self::SourceCrossesScope {
                attempt_run_id,
                source_attempt_run_id,
            } => write!(
                formatter,
                "Agent attempt `{attempt_run_id}` references source `{source_attempt_run_id}` outside its run scope"
            ),
            Self::ConflictingLogicalRunId(attempt) => write!(
                formatter,
                "Agent attempt `{attempt}` has conflicting logical run identities"
            ),
            Self::ConflictingSourceAttempt(attempt) => write!(
                formatter,
                "Agent attempt `{attempt}` has conflicting source attempts"
            ),
            Self::MissingSourceAttempt {
                attempt_run_id,
                source_attempt_run_id,
            } => write!(
                formatter,
                "legacy Agent attempt `{attempt_run_id}` is missing source `{source_attempt_run_id}`"
            ),
            Self::LineageCycle(attempt) => {
                write!(formatter, "Agent attempt lineage contains a cycle at `{attempt}`")
            }
            Self::UnknownAttempt(attempt) => {
                write!(formatter, "Agent attempt `{attempt}` is outside the lineage")
            }
        }
    }
}

impl std::error::Error for AgentRunIdentityError {}

#[cfg(test)]
#[path = "run_identity_tests.rs"]
mod tests;
