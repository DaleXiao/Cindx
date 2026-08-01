use agent_runtime::AgentRunControl;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRegistryError {
    label: &'static str,
}

impl HarnessRegistryError {
    fn poisoned(label: &'static str) -> Self {
        Self { label }
    }
}

impl fmt::Display for HarnessRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} lock poisoned", self.label)
    }
}

impl Error for HarnessRegistryError {}

#[derive(Debug)]
struct RunRegistryInner {
    label: &'static str,
    entries: Mutex<BTreeMap<String, Arc<AgentRunControl>>>,
}

#[derive(Debug, Clone)]
pub struct RunRegistry {
    inner: Arc<RunRegistryInner>,
}

impl RunRegistry {
    pub fn new(label: &'static str) -> Self {
        Self {
            inner: Arc::new(RunRegistryInner {
                label,
                entries: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    pub fn register(
        &self,
        key: impl Into<String>,
        control: Arc<AgentRunControl>,
    ) -> Result<Option<RegisteredRun>, HarnessRegistryError> {
        let key = key.into();
        let mut entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?;
        if entries.contains_key(&key) {
            return Ok(None);
        }
        entries.insert(key.clone(), Arc::clone(&control));
        drop(entries);
        Ok(Some(RegisteredRun {
            registry: self.clone(),
            key,
            control,
        }))
    }

    pub fn get(&self, key: &str) -> Result<Option<Arc<AgentRunControl>>, HarnessRegistryError> {
        Ok(self
            .inner
            .entries
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?
            .get(key)
            .cloned())
    }

    pub fn contains_any<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Result<bool, HarnessRegistryError> {
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?;
        Ok(keys.into_iter().any(|key| entries.contains_key(key)))
    }

    pub fn is_empty(&self) -> Result<bool, HarnessRegistryError> {
        Ok(self
            .inner
            .entries
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?
            .is_empty())
    }

    pub fn cancel_all(&self) -> Result<(), HarnessRegistryError> {
        let controls = self
            .inner
            .entries
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for control in controls {
            control.request_cancel();
        }
        Ok(())
    }

    pub fn remove_many(&self, keys: &[String], cancel: bool) -> Result<(), HarnessRegistryError> {
        let controls = {
            let mut entries = self
                .inner
                .entries
                .lock()
                .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?;
            keys.iter()
                .filter_map(|key| entries.remove(key))
                .collect::<Vec<_>>()
        };
        if cancel {
            for control in controls {
                control.request_cancel();
            }
        }
        Ok(())
    }

    fn remove_if_current(&self, key: &str, control: &Arc<AgentRunControl>) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, control))
        {
            entries.remove(key);
        }
    }
}

#[derive(Debug)]
pub struct RegisteredRun {
    registry: RunRegistry,
    key: String,
    control: Arc<AgentRunControl>,
}

impl RegisteredRun {
    pub fn control(&self) -> Arc<AgentRunControl> {
        Arc::clone(&self.control)
    }
}

impl Drop for RegisteredRun {
    fn drop(&mut self) {
        self.registry.remove_if_current(&self.key, &self.control);
    }
}

#[derive(Debug)]
struct ExclusiveKeyRegistryInner {
    label: &'static str,
    keys: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Clone)]
pub struct ExclusiveKeyRegistry {
    inner: Arc<ExclusiveKeyRegistryInner>,
}

impl ExclusiveKeyRegistry {
    pub fn new(label: &'static str) -> Self {
        Self {
            inner: Arc::new(ExclusiveKeyRegistryInner {
                label,
                keys: Mutex::new(BTreeSet::new()),
            }),
        }
    }

    pub fn try_acquire(
        &self,
        key: impl Into<String>,
    ) -> Result<Option<ExclusiveKeyLease>, HarnessRegistryError> {
        let key = key.into();
        let mut keys = self
            .inner
            .keys
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?;
        if !keys.insert(key.clone()) {
            return Ok(None);
        }
        drop(keys);
        Ok(Some(ExclusiveKeyLease {
            registry: self.clone(),
            key,
        }))
    }

    pub fn contains(&self, key: &str) -> Result<bool, HarnessRegistryError> {
        Ok(self
            .inner
            .keys
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?
            .contains(key))
    }

    pub fn contains_any<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Result<bool, HarnessRegistryError> {
        let active = self
            .inner
            .keys
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?;
        Ok(keys.into_iter().any(|key| active.contains(key)))
    }

    pub fn snapshot(&self) -> Result<BTreeSet<String>, HarnessRegistryError> {
        Ok(self
            .inner
            .keys
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?
            .clone())
    }

    pub fn remove_many(&self, keys: &[String]) -> Result<(), HarnessRegistryError> {
        let mut active = self
            .inner
            .keys
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?;
        for key in keys {
            active.remove(key);
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<(), HarnessRegistryError> {
        self.inner
            .keys
            .lock()
            .map_err(|_| HarnessRegistryError::poisoned(self.inner.label))?
            .clear();
        Ok(())
    }

    fn release(&self, key: &str) {
        self.inner
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }
}

#[derive(Debug)]
pub struct ExclusiveKeyLease {
    registry: ExclusiveKeyRegistry,
    key: String,
}

impl Drop for ExclusiveKeyLease {
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::{ExclusiveKeyRegistry, RunRegistry};
    use agent_runtime::{AgentRunControl, RunStopReason};
    use std::sync::Arc;

    #[test]
    fn registered_run_is_unique_and_released_by_its_guard() {
        let registry = RunRegistry::new("test runs");
        let first = Arc::new(AgentRunControl::new("auto"));
        let lease = registry
            .register("session-a", Arc::clone(&first))
            .unwrap()
            .unwrap();

        assert!(registry
            .register("session-a", Arc::new(AgentRunControl::new("auto")))
            .unwrap()
            .is_none());
        assert!(Arc::ptr_eq(
            &registry.get("session-a").unwrap().unwrap(),
            &first
        ));

        drop(lease);
        assert!(registry.get("session-a").unwrap().is_none());
    }

    #[test]
    fn stale_guard_does_not_remove_a_replacement_run() {
        let registry = RunRegistry::new("test runs");
        let first = registry
            .register("session-a", Arc::new(AgentRunControl::new("auto")))
            .unwrap()
            .unwrap();
        registry
            .remove_many(&["session-a".to_string()], false)
            .unwrap();
        let replacement = Arc::new(AgentRunControl::new("pro"));
        let _replacement_lease = registry
            .register("session-a", Arc::clone(&replacement))
            .unwrap()
            .unwrap();

        drop(first);
        assert!(Arc::ptr_eq(
            &registry.get("session-a").unwrap().unwrap(),
            &replacement
        ));
    }

    #[test]
    fn cancelling_and_removing_runs_is_explicit() {
        let registry = RunRegistry::new("test runs");
        let control = Arc::new(AgentRunControl::new("auto"));
        let _lease = registry
            .register("session-a", Arc::clone(&control))
            .unwrap()
            .unwrap();

        registry
            .remove_many(&["session-a".to_string()], true)
            .unwrap();

        assert_eq!(control.stop_reason(), Some(RunStopReason::UserCancelled));
        assert!(registry.is_empty().unwrap());
    }

    #[test]
    fn exclusive_key_lease_is_owned_and_releases_on_drop() {
        let registry = ExclusiveKeyRegistry::new("test keys");
        let lease = registry.try_acquire("session-a").unwrap().unwrap();
        assert!(registry.contains("session-a").unwrap());
        assert!(registry.try_acquire("session-a").unwrap().is_none());

        drop(lease);
        assert!(!registry.contains("session-a").unwrap());
        assert!(registry.try_acquire("session-a").unwrap().is_some());
    }
}
