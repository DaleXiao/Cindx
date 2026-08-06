use agent_core::{
    Event, EventId, EventKind, Metadata, PermissionDecision, PermissionRequest,
    PermissionRequestId, PermissionResolution, PermissionRisk, TaskId,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::path::Path;
use std::ptr;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
const SHELL_PERMISSION_CAPABILITY_KEY_VERSION: &str = "shell-command-v1";
const SHELL_PERMISSION_CAPABILITY_MIGRATION: &str = "permission_capability_key_shell_sha256_v1";

#[allow(non_camel_case_types)]
enum sqlite3 {}

#[allow(non_camel_case_types)]
enum sqlite3_stmt {}

type SqliteDestructor = Option<unsafe extern "C" fn(*mut c_void)>;

#[link(name = "sqlite3")]
unsafe extern "C" {
    fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut sqlite3) -> c_int;
    fn sqlite3_open_v2(
        filename: *const c_char,
        pp_db: *mut *mut sqlite3,
        flags: c_int,
        z_vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
    fn sqlite3_get_autocommit(db: *mut sqlite3) -> c_int;
    fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(value: *mut c_void);
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        sql: *const c_char,
        n_byte: c_int,
        pp_stmt: *mut *mut sqlite3_stmt,
        pz_tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_changes(db: *mut sqlite3) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut sqlite3_stmt,
        index: c_int,
        value: *const c_char,
        n: c_int,
        destructor: SqliteDestructor,
    ) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut sqlite3_stmt, index: c_int, value: i64) -> c_int;
    fn sqlite3_bind_null(stmt: *mut sqlite3_stmt, index: c_int) -> c_int;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, index: c_int) -> *const c_uchar;
    fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, index: c_int) -> i64;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageError {
    pub message: String,
}

impl StorageError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for StorageError {}

pub trait EventStore {
    fn append(&mut self, event: Event) -> Result<(), StorageError>;

    fn list_by_task(&self, task_id: &TaskId) -> Result<Vec<Event>, StorageError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionAuditRecord {
    pub request: PermissionRequest,
    pub requested_at_ms: u64,
    pub resolution: Option<PermissionResolution>,
}

pub trait PermissionStore {
    fn save_permission_request(
        &mut self,
        request: PermissionRequest,
        requested_at_ms: u64,
    ) -> Result<(), StorageError>;

    fn resolve_permission(&mut self, resolution: PermissionResolution) -> Result<(), StorageError>;

    fn get_permission_request(
        &self,
        request_id: &PermissionRequestId,
    ) -> Result<Option<PermissionRequest>, StorageError>;

    fn list_permission_audits(&self) -> Result<Vec<PermissionAuditRecord>, StorageError>;
}

pub struct SqliteStore {
    connection: *mut sqlite3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRevision {
    pub event_count: u64,
    pub latest_sequence: u64,
    pub latest_timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReadModel {
    pub revision: u64,
    pub payload: String,
}

const EVENT_SCOPE_COLUMNS: [(&str, &str); 8] = [
    ("project_id", "project_id"),
    ("session_id", "session_id"),
    ("agent_run_id", "agent_run_id"),
    (LOGICAL_AGENT_RUN_ID_METADATA_KEY, "logical_agent_run_id"),
    ("collaboration_id", "collaboration_id"),
    ("prompt_profile", "prompt_profile"),
    ("queue_id", "queue_id"),
    ("result_effect_fingerprint", "effect_fingerprint"),
];
const PERMISSION_SCOPE_COLUMNS: [(&str, &str); 2] = [
    ("session_id", "session_id"),
    ("agent_run_id", "agent_run_id"),
];

fn event_scope_column(key: &str) -> Option<&'static str> {
    EVENT_SCOPE_COLUMNS
        .iter()
        .find_map(|(metadata_key, column)| (*metadata_key == key).then_some(*column))
}

fn event_effect_fingerprint(metadata: &Metadata) -> Option<&str> {
    metadata
        .get("result_effect_fingerprint")
        .or_else(|| metadata.get("result_input_fingerprint"))
        .map(String::as_str)
}

fn permission_scope_column(key: &str) -> Option<&'static str> {
    PERMISSION_SCOPE_COLUMNS
        .iter()
        .find_map(|(metadata_key, column)| (*metadata_key == key).then_some(*column))
}

unsafe impl Send for SqliteStore {}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_string_lossy().to_string();
        let c_path = CString::new(path).map_err(|error| StorageError::new(error.to_string()))?;
        let mut connection = ptr::null_mut();
        let code = unsafe { sqlite3_open(c_path.as_ptr(), &mut connection) };

        if code != SQLITE_OK {
            let message = sqlite_error_message(connection);
            if !connection.is_null() {
                unsafe {
                    sqlite3_close(connection);
                }
            }
            return Err(StorageError::new(message));
        }

        let store = Self { connection };
        store.configure_writable_connection()?;
        store.migrate()?;
        Ok(store)
    }

    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_string_lossy().to_string();
        let c_path = CString::new(path).map_err(|error| StorageError::new(error.to_string()))?;
        let mut connection = ptr::null_mut();
        let code = unsafe {
            sqlite3_open_v2(
                c_path.as_ptr(),
                &mut connection,
                SQLITE_OPEN_READONLY,
                ptr::null(),
            )
        };

        if code != SQLITE_OK {
            let message = sqlite_error_message(connection);
            if !connection.is_null() {
                unsafe {
                    sqlite3_close(connection);
                }
            }
            return Err(StorageError::new(message));
        }

        let store = Self { connection };
        store.configure_read_only_connection()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        Self::open(":memory:")
    }

    fn configure_writable_connection(&self) -> Result<(), StorageError> {
        self.exec_batch(
            "
            pragma busy_timeout = 5000;
            pragma journal_mode = WAL;
            pragma synchronous = NORMAL;
            ",
        )
    }

    fn configure_read_only_connection(&self) -> Result<(), StorageError> {
        self.exec_batch(
            "
            pragma busy_timeout = 5000;
            pragma query_only = ON;
            ",
        )
    }

    pub fn with_read_snapshot<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.exec_batch("begin deferred transaction;")?;
        match operation(self) {
            Ok(value) => {
                self.exec_batch("commit;")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.exec_batch("rollback;");
                Err(error)
            }
        }
    }

    pub fn with_immediate_transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.exec_batch("begin immediate transaction;")?;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(self))) {
            Ok(Ok(value)) => match self.exec_batch("commit;") {
                Ok(()) => Ok(value),
                Err(error) => {
                    let _ = self.exec_batch("rollback;");
                    Err(error)
                }
            },
            Ok(Err(error)) => {
                let _ = self.exec_batch("rollback;");
                Err(error)
            }
            Err(payload) => {
                let _ = self.exec_batch("rollback;");
                std::panic::resume_unwind(payload)
            }
        }
    }

    /// Resolves one permission inside the caller's active transaction so the
    /// resolution can commit atomically with its canonical lifecycle events.
    pub fn resolve_permission_in_transaction(
        &mut self,
        resolution: &PermissionResolution,
    ) -> Result<(), StorageError> {
        if unsafe { sqlite3_get_autocommit(self.connection) } != 0 {
            return Err(StorageError::new(
                "permission resolution requires an active transaction",
            ));
        }
        let mut insert = self.prepare(
            "
            insert or replace into permission_resolutions(
              request_id, decision, resolved_at_ms, resolved_by
            )
            values (?1, ?2, ?3, ?4)
            ",
        )?;
        insert.bind_text(1, &resolution.request_id.0)?;
        insert.bind_text(2, permission_decision_to_str(&resolution.decision))?;
        insert.bind_i64(3, resolution.resolved_at_ms as i64)?;
        insert.bind_text(4, &resolution.resolved_by)?;
        insert.expect_done()?;

        let mut update =
            self.prepare("update permission_requests set status = 'resolved' where id = ?1")?;
        update.bind_text(1, &resolution.request_id.0)?;
        update.expect_done()
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn execute_batch_for_testing(&self, sql: &str) -> Result<(), StorageError> {
        self.exec_batch(sql)
    }

    pub fn next_sequence(&self, task_id: &TaskId) -> Result<u64, StorageError> {
        let mut statement =
            self.prepare("select coalesce(max(sequence), 0) + 1 from events where task_id = ?1")?;
        statement.bind_text(1, &task_id.0)?;

        if statement.step()? == StepResult::Row {
            Ok(statement.column_i64(0) as u64)
        } else {
            Ok(1)
        }
    }

    pub fn append_next_event(
        &mut self,
        id: EventId,
        task_id: TaskId,
        timestamp_ms: u64,
        kind: EventKind,
        summary: String,
        metadata: Metadata,
    ) -> Result<(), StorageError> {
        let mut statement = self.prepare(
            "
            insert into events(
              id, task_id, sequence, timestamp_ms, kind, summary, metadata_text,
              project_id, session_id, agent_run_id, logical_agent_run_id,
              collaboration_id, prompt_profile, queue_id, tool_call_id, effect_fingerprint
            )
            values (
              ?1, ?2,
              (select coalesce(max(sequence), 0) + 1 from events where task_id = ?2),
              ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
            )
            ",
        )?;

        statement.bind_text(1, &id.0)?;
        statement.bind_text(2, &task_id.0)?;
        statement.bind_i64(3, timestamp_ms as i64)?;
        statement.bind_text(4, event_kind_to_str(&kind))?;
        statement.bind_text(5, &summary)?;
        statement.bind_text(6, &metadata_to_text(&metadata))?;
        statement.bind_optional_text(7, metadata.get("project_id").map(String::as_str))?;
        statement.bind_optional_text(8, metadata.get("session_id").map(String::as_str))?;
        statement.bind_optional_text(9, metadata.get("agent_run_id").map(String::as_str))?;
        statement.bind_optional_text(
            10,
            metadata
                .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
                .map(String::as_str),
        )?;
        statement.bind_optional_text(11, metadata.get("collaboration_id").map(String::as_str))?;
        statement.bind_optional_text(12, metadata.get("prompt_profile").map(String::as_str))?;
        statement.bind_optional_text(13, metadata.get("queue_id").map(String::as_str))?;
        statement.bind_optional_text(14, metadata.get("tool_call_id").map(String::as_str))?;
        statement.bind_optional_text(15, event_effect_fingerprint(&metadata))?;
        statement.expect_done()
    }

    pub fn list_by_task_and_metadata_values_in_scope(
        &self,
        task_id: &TaskId,
        key: &str,
        values: &[String],
        scope_key: &str,
        scope_value: &str,
    ) -> Result<Vec<Event>, StorageError> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let column = event_scope_column(key).ok_or_else(|| {
            StorageError::new(format!("event metadata key is not indexed: {key}"))
        })?;
        let scope_column = event_scope_column(scope_key).ok_or_else(|| {
            StorageError::new(format!(
                "event scope metadata key is not indexed: {scope_key}"
            ))
        })?;
        let placeholders = (0..values.len())
            .map(|index| format!("?{}", index + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = self.prepare(&format!(
            "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
             from events
             where task_id = ?1 and {scope_column} = ?2 and {column} in ({placeholders})
             order by sequence asc"
        ))?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, scope_value)?;
        for (index, value) in values.iter().enumerate() {
            statement.bind_text((index + 3) as c_int, value)?;
        }
        events_from_statement(&mut statement)
    }

    pub fn event_revision(&self, task_id: &TaskId) -> Result<EventRevision, StorageError> {
        let mut statement = self.prepare(
            "select count(*), coalesce(max(sequence), 0), coalesce(max(timestamp_ms), 0)\n             from events where task_id = ?1",
        )?;
        statement.bind_text(1, &task_id.0)?;
        event_revision_from_statement(&mut statement)
    }

    /// Reads only the indexed event tail for callers that already own a
    /// persisted event count and need to discover an append-only delta.
    pub fn latest_sequence(&self, task_id: &TaskId) -> Result<u64, StorageError> {
        let mut statement = self.prepare(
            "select sequence from events\n             where task_id = ?1\n             order by sequence desc limit 1",
        )?;
        statement.bind_text(1, &task_id.0)?;
        if statement.step()? != StepResult::Row {
            return Ok(0);
        }
        Ok(statement.column_i64(0) as u64)
    }

    pub fn list_by_task_after(
        &self,
        task_id: &TaskId,
        after_sequence: u64,
    ) -> Result<Vec<Event>, StorageError> {
        let mut statement = self.prepare(
            "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text\n             from events\n             where task_id = ?1 and sequence > ?2\n             order by sequence asc",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_i64(2, after_sequence.min(i64::MAX as u64) as i64)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_kinds(
        &self,
        task_id: &TaskId,
        kinds: &[EventKind],
    ) -> Result<Vec<Event>, StorageError> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..kinds.len())
            .map(|index| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = self.prepare(&format!(
            "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text\n             from events\n             where task_id = ?1 and kind in ({placeholders})\n             order by sequence asc"
        ))?;
        statement.bind_text(1, &task_id.0)?;
        for (index, kind) in kinds.iter().enumerate() {
            statement.bind_text((index + 2) as c_int, event_kind_to_str(kind))?;
        }
        events_from_statement(&mut statement)
    }

    pub fn event_revision_by_metadata(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
    ) -> Result<EventRevision, StorageError> {
        if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select count(*), coalesce(max(sequence), 0), coalesce(max(timestamp_ms), 0)\n                 from events where task_id = ?1 and {column} = ?2"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_text(2, value)?;
            return event_revision_from_statement(&mut statement);
        }
        let row = format!(
            "{}\t{}",
            hex_encode(key.as_bytes()),
            hex_encode(value.as_bytes())
        );
        let mut statement = self.prepare(
            "
            select count(*), coalesce(max(sequence), 0), coalesce(max(timestamp_ms), 0)
            from events
            where task_id = ?1
              and instr(char(10) || metadata_text || char(10), char(10) || ?2 || char(10)) > 0
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, &row)?;
        event_revision_from_statement(&mut statement)
    }

    pub fn list_by_task_and_metadata(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
    ) -> Result<Vec<Event>, StorageError> {
        if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text\n                 from events where task_id = ?1 and {column} = ?2 order by sequence asc"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_text(2, value)?;
            return events_from_statement(&mut statement);
        }
        let row = format!(
            "{}\t{}",
            hex_encode(key.as_bytes()),
            hex_encode(value.as_bytes())
        );
        let mut statement = self.prepare(
            "
            select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
            from events
            where task_id = ?1
              and instr(char(10) || metadata_text || char(10), char(10) || ?2 || char(10)) > 0
            order by sequence asc
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, &row)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_tool_call_id(
        &self,
        task_id: &TaskId,
        tool_call_id: &str,
    ) -> Result<Vec<Event>, StorageError> {
        let mut statement = self.prepare(
            "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
             from events
             where task_id = ?1 and tool_call_id = ?2
             order by sequence asc",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, tool_call_id)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_effect_fingerprint(
        &self,
        task_id: &TaskId,
        effect_fingerprint: &str,
    ) -> Result<Vec<Event>, StorageError> {
        let mut statement = self.prepare(
            "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
             from events
             where task_id = ?1 and effect_fingerprint = ?2
             order by sequence asc",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, effect_fingerprint)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_metadata_or_unscoped(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
    ) -> Result<Vec<Event>, StorageError> {
        if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text\n                 from events\n                 where task_id = ?1 and ({column} = ?2 or {column} is null)\n                 order by sequence asc"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_text(2, value)?;
            return events_from_statement(&mut statement);
        }
        let encoded_key = hex_encode(key.as_bytes());
        let row = format!("{}\t{}", encoded_key, hex_encode(value.as_bytes()));
        let key_prefix = format!("{}\t", encoded_key);
        let mut statement = self.prepare(
            "
            select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
            from events
            where task_id = ?1
              and (
                instr(char(10) || metadata_text || char(10), char(10) || ?2 || char(10)) > 0
                or instr(char(10) || metadata_text || char(10), char(10) || ?3) = 0
              )
            order by sequence asc
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, &row)?;
        statement.bind_text(3, &key_prefix)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_metadata_or_unscoped_after(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
        after_sequence: u64,
    ) -> Result<Vec<Event>, StorageError> {
        if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text\n                 from events\n                 where task_id = ?1\n                   and sequence > ?2\n                   and ({column} = ?3 or {column} is null)\n                 order by sequence asc"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_i64(2, after_sequence as i64)?;
            statement.bind_text(3, value)?;
            return events_from_statement(&mut statement);
        }

        let encoded_key = hex_encode(key.as_bytes());
        let row = format!("{}\t{}", encoded_key, hex_encode(value.as_bytes()));
        let key_prefix = format!("{}\t", encoded_key);
        let mut statement = self.prepare(
            "
            select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
            from events
            where task_id = ?1
              and sequence > ?2
              and (
                instr(char(10) || metadata_text || char(10), char(10) || ?3 || char(10)) > 0
                or instr(char(10) || metadata_text || char(10), char(10) || ?4) = 0
              )
            order by sequence asc
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_i64(2, after_sequence as i64)?;
        statement.bind_text(3, &row)?;
        statement.bind_text(4, &key_prefix)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_metadata_after(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
        after_sequence: u64,
    ) -> Result<Vec<Event>, StorageError> {
        self.list_by_task_and_metadata_after_with_tool_metadata_limit(
            task_id,
            key,
            value,
            after_sequence,
            usize::MAX,
        )
    }

    pub fn list_by_task_and_metadata_after_with_tool_metadata_limit(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
        after_sequence: u64,
        max_tool_metadata_bytes: usize,
    ) -> Result<Vec<Event>, StorageError> {
        let max_tool_metadata_bytes = max_tool_metadata_bytes.min(i64::MAX as usize) as i64;
        if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select id, task_id, sequence, timestamp_ms, kind, summary,\n                        case when kind = 'tool_call_finished' and length(metadata_text) > ?4\n                             then '' else metadata_text end\n                 from events\n                 where task_id = ?1 and {column} = ?2 and sequence > ?3\n                 order by sequence asc"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_text(2, value)?;
            statement.bind_i64(3, after_sequence as i64)?;
            statement.bind_i64(4, max_tool_metadata_bytes)?;
            return events_from_statement(&mut statement);
        }
        let row = format!(
            "{}\t{}",
            hex_encode(key.as_bytes()),
            hex_encode(value.as_bytes())
        );
        let mut statement = self.prepare(
            "
            select id, task_id, sequence, timestamp_ms, kind, summary,
                   case when kind = 'tool_call_finished' and length(metadata_text) > ?4
                        then '' else metadata_text end
            from events
            where task_id = ?1
              and sequence > ?2
              and instr(char(10) || metadata_text || char(10), char(10) || ?3 || char(10)) > 0
            order by sequence asc
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_i64(2, after_sequence as i64)?;
        statement.bind_text(3, &row)?;
        statement.bind_i64(4, max_tool_metadata_bytes)?;
        events_from_statement(&mut statement)
    }

    pub fn list_by_task_and_metadata_before(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
        before_sequence: u64,
        limit: usize,
    ) -> Result<Vec<Event>, StorageError> {
        self.list_by_task_and_metadata_before_with_tool_metadata_limit(
            task_id,
            key,
            value,
            before_sequence,
            limit,
            usize::MAX,
        )
    }

    pub fn list_by_task_and_metadata_before_with_tool_metadata_limit(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
        before_sequence: u64,
        limit: usize,
        max_tool_metadata_bytes: usize,
    ) -> Result<Vec<Event>, StorageError> {
        let limit = limit.clamp(1, 2_000) as i64;
        let max_tool_metadata_bytes = max_tool_metadata_bytes.min(i64::MAX as usize) as i64;
        let mut events = if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select id, task_id, sequence, timestamp_ms, kind, summary,\n                        case when kind = 'tool_call_finished' and length(metadata_text) > ?5\n                             then '' else metadata_text end\n                 from events\n                 where task_id = ?1 and {column} = ?2 and sequence < ?3\n                 order by sequence desc limit ?4"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_text(2, value)?;
            statement.bind_i64(3, before_sequence.min(i64::MAX as u64) as i64)?;
            statement.bind_i64(4, limit)?;
            statement.bind_i64(5, max_tool_metadata_bytes)?;
            events_from_statement(&mut statement)?
        } else {
            let row = format!(
                "{}\t{}",
                hex_encode(key.as_bytes()),
                hex_encode(value.as_bytes())
            );
            let mut statement = self.prepare(
                "
                select id, task_id, sequence, timestamp_ms, kind, summary,
                       case when kind = 'tool_call_finished' and length(metadata_text) > ?5
                            then '' else metadata_text end
                from events
                where task_id = ?1
                  and sequence < ?2
                  and instr(char(10) || metadata_text || char(10), char(10) || ?3 || char(10)) > 0
                order by sequence desc limit ?4
                ",
            )?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_i64(2, before_sequence.min(i64::MAX as u64) as i64)?;
            statement.bind_text(3, &row)?;
            statement.bind_i64(4, limit)?;
            statement.bind_i64(5, max_tool_metadata_bytes)?;
            events_from_statement(&mut statement)?
        };
        events.reverse();
        Ok(events)
    }

    pub fn has_task_metadata_event_before(
        &self,
        task_id: &TaskId,
        key: &str,
        value: &str,
        before_sequence: u64,
    ) -> Result<bool, StorageError> {
        if let Some(column) = event_scope_column(key) {
            let mut statement = self.prepare(&format!(
                "select 1 from events\n                 where task_id = ?1 and {column} = ?2 and sequence < ?3\n                 limit 1"
            ))?;
            statement.bind_text(1, &task_id.0)?;
            statement.bind_text(2, value)?;
            statement.bind_i64(3, before_sequence.min(i64::MAX as u64) as i64)?;
            return Ok(statement.step()? == StepResult::Row);
        }
        let row = format!(
            "{}\t{}",
            hex_encode(key.as_bytes()),
            hex_encode(value.as_bytes())
        );
        let mut statement = self.prepare(
            "
            select 1 from events
            where task_id = ?1
              and sequence < ?2
              and instr(char(10) || metadata_text || char(10), char(10) || ?3 || char(10)) > 0
            limit 1
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_i64(2, before_sequence.min(i64::MAX as u64) as i64)?;
        statement.bind_text(3, &row)?;
        Ok(statement.step()? == StepResult::Row)
    }

    pub fn load_read_model(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<StoredReadModel>, StorageError> {
        let mut statement = self.prepare(
            "select revision, payload from read_models where namespace = ?1 and model_key = ?2",
        )?;
        statement.bind_text(1, namespace)?;
        statement.bind_text(2, key)?;
        if statement.step()? != StepResult::Row {
            return Ok(None);
        }
        Ok(Some(StoredReadModel {
            revision: statement.column_i64(0) as u64,
            payload: statement.column_text(1)?,
        }))
    }

    pub fn save_read_model(
        &mut self,
        namespace: &str,
        key: &str,
        revision: u64,
        payload: &str,
    ) -> Result<(), StorageError> {
        let mut statement = self.prepare(
            "insert into read_models(namespace, model_key, revision, payload) values (?1, ?2, ?3, ?4)\n             on conflict(namespace, model_key) do update set revision = excluded.revision, payload = excluded.payload",
        )?;
        statement.bind_text(1, namespace)?;
        statement.bind_text(2, key)?;
        statement.bind_i64(3, revision as i64)?;
        statement.bind_text(4, payload)?;
        statement.expect_done()
    }

    /// Publishes a read model only when the stored row still matches the
    /// revision and payload observed by the caller. `None` inserts only when
    /// the row is still absent.
    pub fn compare_exchange_read_model(
        &mut self,
        namespace: &str,
        key: &str,
        expected: Option<&StoredReadModel>,
        revision: u64,
        payload: &str,
    ) -> Result<bool, StorageError> {
        let mut statement = match expected {
            Some(expected) => {
                if revision < expected.revision {
                    return Ok(false);
                }
                let mut statement = self.prepare(
                    "update read_models
                     set revision = ?3, payload = ?4
                     where namespace = ?1 and model_key = ?2
                       and revision = ?5 and payload = ?6",
                )?;
                statement.bind_text(1, namespace)?;
                statement.bind_text(2, key)?;
                statement.bind_i64(3, revision as i64)?;
                statement.bind_text(4, payload)?;
                statement.bind_i64(5, expected.revision as i64)?;
                statement.bind_text(6, &expected.payload)?;
                statement
            }
            None => {
                let mut statement = self.prepare(
                    "insert into read_models(namespace, model_key, revision, payload)
                     values (?1, ?2, ?3, ?4)
                     on conflict(namespace, model_key) do nothing",
                )?;
                statement.bind_text(1, namespace)?;
                statement.bind_text(2, key)?;
                statement.bind_i64(3, revision as i64)?;
                statement.bind_text(4, payload)?;
                statement
            }
        };
        statement.expect_done()?;
        drop(statement);
        Ok(unsafe { sqlite3_changes(self.connection) } == 1)
    }

    pub fn delete_read_model(&mut self, namespace: &str, key: &str) -> Result<(), StorageError> {
        let mut statement =
            self.prepare("delete from read_models where namespace = ?1 and model_key = ?2")?;
        statement.bind_text(1, namespace)?;
        statement.bind_text(2, key)?;
        statement.expect_done()
    }

    pub fn list_permission_audits_for_session(
        &self,
        task_id: &TaskId,
        session_id: &str,
        active_run_id: Option<&str>,
        requested_after_ms: u64,
    ) -> Result<Vec<PermissionAuditRecord>, StorageError> {
        let mut statement = if active_run_id.is_some() {
            self.prepare(
                "select
                   pr.id, pr.task_id, pr.risk, pr.action, pr.reason, pr.scope,
                   pr.metadata_text, pr.requested_at_ms, rr.decision,
                   rr.resolved_at_ms, rr.resolved_by
                 from permission_requests pr
                 left join permission_resolutions rr on rr.request_id = pr.id
                 where pr.task_id = ?1 and pr.session_id = ?2 and pr.agent_run_id = ?3
                 order by pr.requested_at_ms desc, pr.id desc",
            )?
        } else {
            self.prepare(
                "select
                   pr.id, pr.task_id, pr.risk, pr.action, pr.reason, pr.scope,
                   pr.metadata_text, pr.requested_at_ms, rr.decision,
                   rr.resolved_at_ms, rr.resolved_by
                 from permission_requests pr
                 left join permission_resolutions rr on rr.request_id = pr.id
                 where pr.task_id = ?1 and pr.session_id = ?2 and pr.requested_at_ms >= ?3
                 order by pr.requested_at_ms desc, pr.id desc",
            )?
        };
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, session_id)?;
        if let Some(run_id) = active_run_id {
            statement.bind_text(3, run_id)?;
        } else {
            statement.bind_i64(3, requested_after_ms.min(i64::MAX as u64) as i64)?;
        }
        permission_audits_from_statement(&mut statement)
    }

    pub fn has_session_permission_capability(
        &self,
        task_id: &TaskId,
        session_id: &str,
        request: &PermissionRequest,
        exact_scope: bool,
        require_capability_key: bool,
    ) -> Result<bool, StorageError> {
        let capability_key = require_capability_key
            .then(|| permission_capability_key(request))
            .flatten();
        if require_capability_key && capability_key.is_none() {
            return Ok(false);
        }
        let mut query = String::from(
            "select 1
             from permission_requests pr
             inner join permission_resolutions rr on rr.request_id = pr.id
             where pr.task_id = ?1
               and pr.session_id = ?2
               and pr.risk = ?3
               and pr.action = ?4",
        );
        if exact_scope {
            query.push_str("\n               and pr.scope = ?5");
        }
        if capability_key.is_some() {
            let binding = if exact_scope { 6 } else { 5 };
            query.push_str(&format!(
                "\n               and pr.capability_key = ?{binding}"
            ));
        }
        query.push_str(
            "\n               and rr.decision = 'allow_for_session'\n             limit 1",
        );

        let mut statement = self.prepare(&query)?;
        statement.bind_text(1, &task_id.0)?;
        statement.bind_text(2, session_id)?;
        statement.bind_text(3, permission_risk_to_str(&request.risk))?;
        statement.bind_text(4, &request.action)?;
        if exact_scope {
            statement.bind_text(5, &request.scope)?;
        }
        if let Some(capability_key) = capability_key.as_deref() {
            statement.bind_text(if exact_scope { 6 } else { 5 }, capability_key)?;
        }
        Ok(statement.step()? == StepResult::Row)
    }

    pub fn delete_records_by_metadata(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<(), StorageError> {
        self.with_immediate_transaction(|transaction| {
            transaction.delete_records_by_metadata_in_transaction(key, value)
        })
    }

    /// Deletes event and permission rows for one metadata scope inside the
    /// caller's active transaction.
    ///
    /// This is the transaction-composable counterpart to
    /// [`Self::delete_records_by_metadata`]. It rejects calls made outside a
    /// transaction so a multi-table delete cannot accidentally be partially
    /// committed.
    pub fn delete_records_by_metadata_in_transaction(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<(), StorageError> {
        if unsafe { sqlite3_get_autocommit(self.connection) } != 0 {
            return Err(StorageError::new(
                "scoped record deletion requires an active transaction",
            ));
        }
        let row = format!(
            "{}\t{}",
            hex_encode(key.as_bytes()),
            hex_encode(value.as_bytes())
        );
        let metadata_predicate =
            "instr(char(10) || metadata_text || char(10), char(10) || ?1 || char(10)) > 0";
        let (permission_predicate, permission_value) = permission_scope_column(key)
            .map(|column| (format!("{column} = ?1"), value.to_string()))
            .unwrap_or_else(|| (metadata_predicate.to_string(), row.clone()));
        let mut delete_resolutions = self.prepare(&format!(
            "delete from permission_resolutions where request_id in (select id from permission_requests where {permission_predicate})"
        ))?;
        delete_resolutions.bind_text(1, &permission_value)?;
        delete_resolutions.expect_done()?;

        let mut delete_requests = self.prepare(&format!(
            "delete from permission_requests where {permission_predicate}"
        ))?;
        delete_requests.bind_text(1, &permission_value)?;
        delete_requests.expect_done()?;

        let (event_predicate, event_value) = event_scope_column(key)
            .map(|column| (format!("{column} = ?1"), value.to_string()))
            .unwrap_or_else(|| (metadata_predicate.to_string(), row));
        let mut delete_events =
            self.prepare(&format!("delete from events where {event_predicate}"))?;
        delete_events.bind_text(1, &event_value)?;
        delete_events.expect_done()
    }

    pub fn delete_events_by_ids(&mut self, event_ids: &[String]) -> Result<(), StorageError> {
        if event_ids.is_empty() {
            return Ok(());
        }
        self.exec_batch("begin immediate transaction")?;
        let result = (|| {
            for event_id in event_ids {
                let mut statement = self.prepare("delete from events where id = ?1")?;
                statement.bind_text(1, event_id)?;
                statement.expect_done()?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.exec_batch("commit"),
            Err(error) => {
                let _ = self.exec_batch("rollback");
                Err(error)
            }
        }
    }

    pub fn list_all_events(&self) -> Result<Vec<Event>, StorageError> {
        let mut statement = self.prepare(
            "
            select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
            from events
            order by timestamp_ms asc, task_id asc, sequence asc
            ",
        )?;
        let mut events = Vec::new();
        while statement.step()? == StepResult::Row {
            events.push(Event {
                id: EventId(statement.column_text(0)?),
                task_id: TaskId(statement.column_text(1)?),
                sequence: statement.column_i64(2) as u64,
                timestamp_ms: statement.column_i64(3) as u64,
                kind: str_to_event_kind(&statement.column_text(4)?)?,
                summary: statement.column_text(5)?,
                metadata: metadata_from_text(&statement.column_text(6)?)?,
            });
        }
        Ok(events)
    }

    pub fn oversized_tool_event_ids(
        &self,
        min_metadata_bytes: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut statement = self.prepare(
            "select id from events
             where kind = 'tool_call_finished' and length(metadata_text) > ?1
             order by timestamp_ms asc, task_id asc, sequence asc",
        )?;
        statement.bind_i64(1, min_metadata_bytes.min(i64::MAX as usize) as i64)?;
        let mut ids = Vec::new();
        while statement.step()? == StepResult::Row {
            ids.push(statement.column_text(0)?);
        }
        Ok(ids)
    }

    pub fn event_by_id(&self, event_id: &str) -> Result<Option<Event>, StorageError> {
        let mut statement = self.prepare(
            "select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
             from events where id = ?1",
        )?;
        statement.bind_text(1, event_id)?;
        let mut events = events_from_statement(&mut statement)?;
        Ok(events.pop())
    }

    pub fn update_event_content(&mut self, event: &Event) -> Result<(), StorageError> {
        let mut statement = self.prepare(
            "update events
             set summary = ?1,
                 metadata_text = ?2,
                 project_id = ?3,
                 session_id = ?4,
                 agent_run_id = ?5,
                 logical_agent_run_id = ?6,
                 collaboration_id = ?7,
                 prompt_profile = ?8,
                 queue_id = ?9,
                 tool_call_id = ?10,
                 effect_fingerprint = ?11
             where id = ?12",
        )?;
        statement.bind_text(1, &event.summary)?;
        statement.bind_text(2, &metadata_to_text(&event.metadata))?;
        statement.bind_optional_text(3, event.metadata.get("project_id").map(String::as_str))?;
        statement.bind_optional_text(4, event.metadata.get("session_id").map(String::as_str))?;
        statement.bind_optional_text(5, event.metadata.get("agent_run_id").map(String::as_str))?;
        statement.bind_optional_text(
            6,
            event
                .metadata
                .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
                .map(String::as_str),
        )?;
        statement.bind_optional_text(
            7,
            event.metadata.get("collaboration_id").map(String::as_str),
        )?;
        statement
            .bind_optional_text(8, event.metadata.get("prompt_profile").map(String::as_str))?;
        statement.bind_optional_text(9, event.metadata.get("queue_id").map(String::as_str))?;
        statement.bind_optional_text(10, event.metadata.get("tool_call_id").map(String::as_str))?;
        statement.bind_optional_text(11, event_effect_fingerprint(&event.metadata))?;
        statement.bind_text(12, &event.id.0)?;
        statement.expect_done()
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.exec_batch(
            "
            create table if not exists events (
              id text primary key not null,
              task_id text not null,
              sequence integer not null,
              timestamp_ms integer not null,
              kind text not null,
              summary text not null,
              metadata_text text not null,
              project_id text,
              session_id text,
              agent_run_id text,
              logical_agent_run_id text,
              collaboration_id text,
              prompt_profile text,
              queue_id text,
              tool_call_id text,
              effect_fingerprint text
            );

            create index if not exists idx_events_task_sequence
              on events(task_id, sequence);
            create index if not exists idx_events_task_kind_sequence
              on events(task_id, kind, sequence);

            create table if not exists permission_requests (
              id text primary key not null,
              task_id text not null,
              risk text not null,
              action text not null,
              reason text not null,
              scope text not null,
              metadata_text text not null,
              requested_at_ms integer not null,
              status text not null,
              session_id text,
              agent_run_id text,
              capability_key text
            );

            create table if not exists permission_resolutions (
              request_id text primary key not null,
              decision text not null,
              resolved_at_ms integer not null,
              resolved_by text not null,
              foreign key(request_id) references permission_requests(id)
            );

            create table if not exists storage_meta (
              key text primary key not null,
              value text not null
            );

            create table if not exists read_models (
              namespace text not null,
              model_key text not null,
              revision integer not null,
              payload text not null,
              primary key(namespace, model_key)
            );
            ",
        )?;
        self.ensure_event_scope_columns()?;
        if !self.table_has_column("events", "tool_call_id")? {
            self.exec_batch("alter table events add column tool_call_id text")?;
        }
        self.ensure_permission_scope_columns()?;
        if !self.table_has_column("permission_requests", "capability_key")? {
            self.exec_batch("alter table permission_requests add column capability_key text")?;
        }
        self.exec_batch(
            "
            create index if not exists idx_events_task_session_sequence
              on events(task_id, session_id, sequence);
            create index if not exists idx_events_task_project_sequence
              on events(task_id, project_id, sequence);
            create index if not exists idx_events_task_run_sequence
              on events(task_id, agent_run_id, sequence);
            create index if not exists idx_events_task_logical_run_sequence
              on events(task_id, logical_agent_run_id, sequence)
              where logical_agent_run_id is not null;
            create index if not exists idx_events_task_collaboration_sequence
              on events(task_id, collaboration_id, sequence);
            create index if not exists idx_events_task_prompt_profile_sequence
              on events(task_id, prompt_profile, sequence);
            create index if not exists idx_events_task_queue_sequence
              on events(task_id, queue_id, sequence);
            create index if not exists idx_events_task_tool_call_sequence
              on events(task_id, tool_call_id, sequence);
            create index if not exists idx_events_task_effect_fingerprint_sequence
              on events(task_id, effect_fingerprint, sequence);
            create index if not exists idx_permission_requests_task_session_time
              on permission_requests(task_id, session_id, requested_at_ms desc);
            create index if not exists idx_permission_requests_task_session_run
              on permission_requests(task_id, session_id, agent_run_id, requested_at_ms desc);
            create index if not exists idx_permission_requests_session_capability
              on permission_requests(task_id, session_id, risk, action, scope);
            create index if not exists idx_permission_requests_session_capability_key
              on permission_requests(task_id, session_id, risk, action, scope, capability_key);
            ",
        )?;
        self.backfill_event_scope_columns()?;
        self.backfill_event_logical_run_scope_column()?;
        self.backfill_event_queue_scope_column()?;
        self.backfill_permission_scope_columns()?;
        self.backfill_permission_capability_keys()
    }

    fn ensure_event_scope_columns(&self) -> Result<(), StorageError> {
        for (_, column) in EVENT_SCOPE_COLUMNS {
            if !self.table_has_column("events", column)? {
                self.exec_batch(&format!("alter table events add column {column} text"))?;
            }
        }
        Ok(())
    }

    fn ensure_permission_scope_columns(&self) -> Result<(), StorageError> {
        for (_, column) in PERMISSION_SCOPE_COLUMNS {
            if !self.table_has_column("permission_requests", column)? {
                self.exec_batch(&format!(
                    "alter table permission_requests add column {column} text"
                ))?;
            }
        }
        Ok(())
    }

    fn table_has_column(&self, table: &str, column: &str) -> Result<bool, StorageError> {
        let mut statement = self.prepare(&format!("pragma table_info({table})"))?;
        while statement.step()? == StepResult::Row {
            if statement.column_text(1)? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn storage_meta_value(&self, key: &str) -> Result<Option<String>, StorageError> {
        let mut statement = self.prepare("select value from storage_meta where key = ?1")?;
        statement.bind_text(1, key)?;
        if statement.step()? == StepResult::Row {
            Ok(Some(statement.column_text(0)?))
        } else {
            Ok(None)
        }
    }

    fn backfill_event_scope_columns(&self) -> Result<(), StorageError> {
        if self
            .storage_meta_value("event_scope_columns_v3")?
            .as_deref()
            == Some("complete")
        {
            return Ok(());
        }

        let mut statement = self.prepare(
            "select id, task_id, metadata_text from events
             order by task_id asc, sequence asc",
        )?;
        let mut rows = Vec::new();
        while statement.step()? == StepResult::Row {
            rows.push((
                statement.column_text(0)?,
                statement.column_text(1)?,
                statement.column_text(2)?,
            ));
        }
        drop(statement);

        self.exec_batch("begin immediate transaction")?;
        let result = (|| {
            let mut current_session_by_task = std::collections::BTreeMap::<String, String>::new();
            for (event_id, task_id, metadata_text) in rows {
                let metadata = metadata_from_text(&metadata_text)?;
                let session_id = if let Some(session_id) = metadata.get("session_id") {
                    current_session_by_task.insert(task_id.clone(), session_id.clone());
                    Some(session_id.as_str())
                } else {
                    current_session_by_task.get(&task_id).map(String::as_str)
                };
                let mut update = self.prepare(
                    "update events
                     set project_id = ?1,
                         session_id = ?2,
                         agent_run_id = ?3,
                         collaboration_id = ?4,
                         prompt_profile = ?5,
                         effect_fingerprint = ?6
                     where id = ?7",
                )?;
                update.bind_optional_text(1, metadata.get("project_id").map(String::as_str))?;
                update.bind_optional_text(2, session_id)?;
                update.bind_optional_text(3, metadata.get("agent_run_id").map(String::as_str))?;
                update
                    .bind_optional_text(4, metadata.get("collaboration_id").map(String::as_str))?;
                update.bind_optional_text(5, metadata.get("prompt_profile").map(String::as_str))?;
                update.bind_optional_text(6, event_effect_fingerprint(&metadata))?;
                update.bind_text(7, &event_id)?;
                update.expect_done()?;
            }
            let mut marker =
                self.prepare("insert or replace into storage_meta(key, value) values (?1, ?2)")?;
            marker.bind_text(1, "event_scope_columns_v3")?;
            marker.bind_text(2, "complete")?;
            marker.expect_done()
        })();
        match result {
            Ok(()) => self.exec_batch("commit"),
            Err(error) => {
                let _ = self.exec_batch("rollback");
                Err(error)
            }
        }
    }

    fn backfill_event_queue_scope_column(&self) -> Result<(), StorageError> {
        if self.storage_meta_value("event_queue_scope_v1")?.as_deref() == Some("complete") {
            return Ok(());
        }

        let queue_key_prefix = format!("{}\t", hex_encode(b"queue_id"));
        let mut statement = self.prepare(
            "select id, metadata_text from events
             where queue_id is null and
               instr(char(10) || metadata_text, char(10) || ?1) > 0
             order by task_id asc, sequence asc",
        )?;
        statement.bind_text(1, &queue_key_prefix)?;
        let mut rows = Vec::new();
        while statement.step()? == StepResult::Row {
            rows.push((statement.column_text(0)?, statement.column_text(1)?));
        }
        drop(statement);

        self.exec_batch("begin immediate transaction")?;
        let result = (|| {
            for (event_id, metadata_text) in rows {
                let metadata = metadata_from_text(&metadata_text)?;
                let mut update = self.prepare("update events set queue_id = ?1 where id = ?2")?;
                update.bind_optional_text(1, metadata.get("queue_id").map(String::as_str))?;
                update.bind_text(2, &event_id)?;
                update.expect_done()?;
            }
            let mut marker =
                self.prepare("insert or replace into storage_meta(key, value) values (?1, ?2)")?;
            marker.bind_text(1, "event_queue_scope_v1")?;
            marker.bind_text(2, "complete")?;
            marker.expect_done()
        })();
        match result {
            Ok(()) => self.exec_batch("commit"),
            Err(error) => {
                let _ = self.exec_batch("rollback");
                Err(error)
            }
        }
    }

    fn backfill_event_logical_run_scope_column(&self) -> Result<(), StorageError> {
        if self
            .storage_meta_value("event_logical_agent_run_scope_v1")?
            .as_deref()
            == Some("complete")
        {
            return Ok(());
        }

        let key_prefix = format!(
            "{}\t",
            hex_encode(LOGICAL_AGENT_RUN_ID_METADATA_KEY.as_bytes())
        );
        let mut statement = self.prepare(
            "select id, metadata_text from events
             where logical_agent_run_id is null and
               instr(char(10) || metadata_text, char(10) || ?1) > 0
             order by task_id asc, sequence asc",
        )?;
        statement.bind_text(1, &key_prefix)?;
        let mut rows = Vec::new();
        while statement.step()? == StepResult::Row {
            rows.push((statement.column_text(0)?, statement.column_text(1)?));
        }
        drop(statement);

        self.exec_batch("begin immediate transaction")?;
        let result = (|| {
            for (event_id, metadata_text) in rows {
                let metadata = metadata_from_text(&metadata_text)?;
                let mut update =
                    self.prepare("update events set logical_agent_run_id = ?1 where id = ?2")?;
                update.bind_optional_text(
                    1,
                    metadata
                        .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
                        .map(String::as_str),
                )?;
                update.bind_text(2, &event_id)?;
                update.expect_done()?;
            }
            let mut marker =
                self.prepare("insert or replace into storage_meta(key, value) values (?1, ?2)")?;
            marker.bind_text(1, "event_logical_agent_run_scope_v1")?;
            marker.bind_text(2, "complete")?;
            marker.expect_done()
        })();
        match result {
            Ok(()) => self.exec_batch("commit"),
            Err(error) => {
                let _ = self.exec_batch("rollback");
                Err(error)
            }
        }
    }

    fn backfill_permission_scope_columns(&self) -> Result<(), StorageError> {
        if self
            .storage_meta_value("permission_scope_columns_v1")?
            .as_deref()
            == Some("complete")
        {
            return Ok(());
        }
        let mut statement = self.prepare("select id, metadata_text from permission_requests")?;
        let mut rows = Vec::new();
        while statement.step()? == StepResult::Row {
            rows.push((statement.column_text(0)?, statement.column_text(1)?));
        }
        drop(statement);

        self.exec_batch("begin immediate transaction")?;
        let result = (|| {
            for (request_id, metadata_text) in rows {
                let metadata = metadata_from_text(&metadata_text)?;
                let mut update = self.prepare(
                    "update permission_requests
                     set session_id = ?1, agent_run_id = ?2
                     where id = ?3",
                )?;
                update.bind_optional_text(1, metadata.get("session_id").map(String::as_str))?;
                update.bind_optional_text(2, metadata.get("agent_run_id").map(String::as_str))?;
                update.bind_text(3, &request_id)?;
                update.expect_done()?;
            }
            let mut marker =
                self.prepare("insert or replace into storage_meta(key, value) values (?1, ?2)")?;
            marker.bind_text(1, "permission_scope_columns_v1")?;
            marker.bind_text(2, "complete")?;
            marker.expect_done()
        })();
        match result {
            Ok(()) => self.exec_batch("commit"),
            Err(error) => {
                let _ = self.exec_batch("rollback");
                Err(error)
            }
        }
    }

    fn backfill_permission_capability_keys(&self) -> Result<(), StorageError> {
        if self
            .storage_meta_value(SHELL_PERMISSION_CAPABILITY_MIGRATION)?
            .as_deref()
            == Some("complete")
        {
            return Ok(());
        }
        let mut statement = self.prepare(
            "select id, task_id, risk, action, reason, scope, metadata_text
             from permission_requests
             where action = 'shell.run'
               and (capability_key is null or capability_key not like 'shell-command-v1:%')",
        )?;
        let mut rows = Vec::new();
        while statement.step()? == StepResult::Row {
            rows.push(PermissionRequest {
                id: PermissionRequestId(statement.column_text(0)?),
                task_id: TaskId(statement.column_text(1)?),
                risk: str_to_permission_risk(&statement.column_text(2)?)?,
                action: statement.column_text(3)?,
                reason: statement.column_text(4)?,
                scope: statement.column_text(5)?,
                metadata: metadata_from_text(&statement.column_text(6)?)?,
            });
        }
        drop(statement);

        self.exec_batch("begin immediate transaction")?;
        let result = (|| {
            for request in rows {
                let capability_key = permission_capability_key(&request);
                let mut update = self
                    .prepare("update permission_requests set capability_key = ?1 where id = ?2")?;
                update.bind_optional_text(1, capability_key.as_deref())?;
                update.bind_text(2, &request.id.0)?;
                update.expect_done()?;
            }
            let mut marker =
                self.prepare("insert or replace into storage_meta(key, value) values (?1, ?2)")?;
            marker.bind_text(1, SHELL_PERMISSION_CAPABILITY_MIGRATION)?;
            marker.bind_text(2, "complete")?;
            marker.expect_done()
        })();
        match result {
            Ok(()) => self.exec_batch("commit"),
            Err(error) => {
                let _ = self.exec_batch("rollback");
                Err(error)
            }
        }
    }

    fn exec_batch(&self, sql: &str) -> Result<(), StorageError> {
        let c_sql = CString::new(sql).map_err(|error| StorageError::new(error.to_string()))?;
        let mut error_message = ptr::null_mut();
        let code = unsafe {
            sqlite3_exec(
                self.connection,
                c_sql.as_ptr(),
                None,
                ptr::null_mut(),
                &mut error_message,
            )
        };

        if code == SQLITE_OK {
            return Ok(());
        }

        let message = if error_message.is_null() {
            sqlite_error_message(self.connection)
        } else {
            let message = unsafe { CStr::from_ptr(error_message) }
                .to_string_lossy()
                .to_string();
            unsafe {
                sqlite3_free(error_message.cast());
            }
            message
        };

        Err(StorageError::new(message))
    }

    fn prepare(&self, sql: &str) -> Result<Statement<'_>, StorageError> {
        let c_sql = CString::new(sql).map_err(|error| StorageError::new(error.to_string()))?;
        let mut statement = ptr::null_mut();
        let code = unsafe {
            sqlite3_prepare_v2(
                self.connection,
                c_sql.as_ptr(),
                -1,
                &mut statement,
                ptr::null_mut(),
            )
        };

        if code != SQLITE_OK {
            return Err(StorageError::new(sqlite_error_message(self.connection)));
        }

        Ok(Statement {
            connection: self.connection,
            statement,
            text_bindings: Vec::new(),
            _owner: std::marker::PhantomData,
        })
    }
}

impl Drop for SqliteStore {
    fn drop(&mut self) {
        if !self.connection.is_null() {
            unsafe {
                sqlite3_close(self.connection);
            }
        }
    }
}

impl EventStore for SqliteStore {
    fn append(&mut self, event: Event) -> Result<(), StorageError> {
        let mut statement = self.prepare(
            "
            insert into events(
              id, task_id, sequence, timestamp_ms, kind, summary, metadata_text,
              project_id, session_id, agent_run_id, logical_agent_run_id,
              collaboration_id, prompt_profile, queue_id, tool_call_id, effect_fingerprint
            )
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ",
        )?;

        statement.bind_text(1, &event.id.0)?;
        statement.bind_text(2, &event.task_id.0)?;
        statement.bind_i64(3, event.sequence as i64)?;
        statement.bind_i64(4, event.timestamp_ms as i64)?;
        statement.bind_text(5, event_kind_to_str(&event.kind))?;
        statement.bind_text(6, &event.summary)?;
        statement.bind_text(7, &metadata_to_text(&event.metadata))?;
        statement.bind_optional_text(8, event.metadata.get("project_id").map(String::as_str))?;
        statement.bind_optional_text(9, event.metadata.get("session_id").map(String::as_str))?;
        statement.bind_optional_text(10, event.metadata.get("agent_run_id").map(String::as_str))?;
        statement.bind_optional_text(
            11,
            event
                .metadata
                .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
                .map(String::as_str),
        )?;
        statement.bind_optional_text(
            12,
            event.metadata.get("collaboration_id").map(String::as_str),
        )?;
        statement
            .bind_optional_text(13, event.metadata.get("prompt_profile").map(String::as_str))?;
        statement.bind_optional_text(14, event.metadata.get("queue_id").map(String::as_str))?;
        statement.bind_optional_text(15, event.metadata.get("tool_call_id").map(String::as_str))?;
        statement.bind_optional_text(16, event_effect_fingerprint(&event.metadata))?;
        statement.expect_done()
    }

    fn list_by_task(&self, task_id: &TaskId) -> Result<Vec<Event>, StorageError> {
        let mut statement = self.prepare(
            "
            select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
            from events
            where task_id = ?1
            order by sequence asc
            ",
        )?;
        statement.bind_text(1, &task_id.0)?;
        events_from_statement(&mut statement)
    }
}

fn events_from_statement(statement: &mut Statement<'_>) -> Result<Vec<Event>, StorageError> {
    let mut events = Vec::new();
    while statement.step()? == StepResult::Row {
        events.push(Event {
            id: EventId(statement.column_text(0)?),
            task_id: TaskId(statement.column_text(1)?),
            sequence: statement.column_i64(2) as u64,
            timestamp_ms: statement.column_i64(3) as u64,
            kind: str_to_event_kind(&statement.column_text(4)?)?,
            summary: statement.column_text(5)?,
            metadata: metadata_from_text(&statement.column_text(6)?)?,
        });
    }
    Ok(events)
}

fn event_revision_from_statement(
    statement: &mut Statement<'_>,
) -> Result<EventRevision, StorageError> {
    if statement.step()? != StepResult::Row {
        return Ok(EventRevision {
            event_count: 0,
            latest_sequence: 0,
            latest_timestamp_ms: 0,
        });
    }
    Ok(EventRevision {
        event_count: statement.column_i64(0) as u64,
        latest_sequence: statement.column_i64(1) as u64,
        latest_timestamp_ms: statement.column_i64(2) as u64,
    })
}

impl PermissionStore for SqliteStore {
    fn save_permission_request(
        &mut self,
        request: PermissionRequest,
        requested_at_ms: u64,
    ) -> Result<(), StorageError> {
        let capability_key = permission_capability_key(&request);
        let mut statement = self.prepare(
            "
            insert or replace into permission_requests(
              id, task_id, risk, action, reason, scope, metadata_text, requested_at_ms, status,
              session_id, agent_run_id, capability_key
            )
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11)
            ",
        )?;

        statement.bind_text(1, &request.id.0)?;
        statement.bind_text(2, &request.task_id.0)?;
        statement.bind_text(3, permission_risk_to_str(&request.risk))?;
        statement.bind_text(4, &request.action)?;
        statement.bind_text(5, &request.reason)?;
        statement.bind_text(6, &request.scope)?;
        statement.bind_text(7, &metadata_to_text(&request.metadata))?;
        statement.bind_i64(8, requested_at_ms as i64)?;
        statement.bind_optional_text(9, request.metadata.get("session_id").map(String::as_str))?;
        statement
            .bind_optional_text(10, request.metadata.get("agent_run_id").map(String::as_str))?;
        statement.bind_optional_text(11, capability_key.as_deref())?;
        statement.expect_done()
    }

    fn resolve_permission(&mut self, resolution: PermissionResolution) -> Result<(), StorageError> {
        self.with_immediate_transaction(|store| {
            store.resolve_permission_in_transaction(&resolution)
        })
    }

    fn get_permission_request(
        &self,
        request_id: &PermissionRequestId,
    ) -> Result<Option<PermissionRequest>, StorageError> {
        let mut statement = self.prepare(
            "
            select id, task_id, risk, action, reason, scope, metadata_text
            from permission_requests
            where id = ?1
            ",
        )?;
        statement.bind_text(1, &request_id.0)?;

        if statement.step()? == StepResult::Row {
            Ok(Some(PermissionRequest {
                id: PermissionRequestId(statement.column_text(0)?),
                task_id: TaskId(statement.column_text(1)?),
                risk: str_to_permission_risk(&statement.column_text(2)?)?,
                action: statement.column_text(3)?,
                reason: statement.column_text(4)?,
                scope: statement.column_text(5)?,
                metadata: metadata_from_text(&statement.column_text(6)?)?,
            }))
        } else {
            Ok(None)
        }
    }

    fn list_permission_audits(&self) -> Result<Vec<PermissionAuditRecord>, StorageError> {
        let mut statement = self.prepare(
            "
            select
              pr.id,
              pr.task_id,
              pr.risk,
              pr.action,
              pr.reason,
              pr.scope,
              pr.metadata_text,
              pr.requested_at_ms,
              rr.decision,
              rr.resolved_at_ms,
              rr.resolved_by
            from permission_requests pr
            left join permission_resolutions rr on rr.request_id = pr.id
            order by pr.requested_at_ms desc, pr.id desc
            ",
        )?;

        permission_audits_from_statement(&mut statement)
    }
}

fn permission_audits_from_statement(
    statement: &mut Statement<'_>,
) -> Result<Vec<PermissionAuditRecord>, StorageError> {
    let mut audits = Vec::new();
    while statement.step()? == StepResult::Row {
        let request_id = PermissionRequestId(statement.column_text(0)?);
        let decision = statement.column_optional_text(8)?;
        let resolution = if let Some(decision) = decision {
            Some(PermissionResolution {
                request_id: request_id.clone(),
                decision: str_to_permission_decision(&decision)?,
                resolved_at_ms: statement.column_i64(9) as u64,
                resolved_by: statement.column_text(10)?,
            })
        } else {
            None
        };
        audits.push(PermissionAuditRecord {
            request: PermissionRequest {
                id: request_id,
                task_id: TaskId(statement.column_text(1)?),
                risk: str_to_permission_risk(&statement.column_text(2)?)?,
                action: statement.column_text(3)?,
                reason: statement.column_text(4)?,
                scope: statement.column_text(5)?,
                metadata: metadata_from_text(&statement.column_text(6)?)?,
            },
            requested_at_ms: statement.column_i64(7) as u64,
            resolution,
        });
    }
    Ok(audits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepResult {
    Row,
    Done,
}

struct Statement<'a> {
    connection: *mut sqlite3,
    statement: *mut sqlite3_stmt,
    text_bindings: Vec<CString>,
    _owner: std::marker::PhantomData<&'a SqliteStore>,
}

impl Statement<'_> {
    fn bind_text(&mut self, index: c_int, value: &str) -> Result<(), StorageError> {
        let c_value = CString::new(value).map_err(|error| StorageError::new(error.to_string()))?;
        let code = unsafe {
            sqlite3_bind_text(
                self.statement,
                index,
                c_value.as_ptr(),
                -1,
                sqlite_transient(),
            )
        };
        if code != SQLITE_OK {
            return Err(StorageError::new(sqlite_error_message(self.connection)));
        }
        self.text_bindings.push(c_value);
        Ok(())
    }

    fn bind_optional_text(
        &mut self,
        index: c_int,
        value: Option<&str>,
    ) -> Result<(), StorageError> {
        if let Some(value) = value {
            return self.bind_text(index, value);
        }
        let code = unsafe { sqlite3_bind_null(self.statement, index) };
        if code == SQLITE_OK {
            Ok(())
        } else {
            Err(StorageError::new(sqlite_error_message(self.connection)))
        }
    }

    fn bind_i64(&mut self, index: c_int, value: i64) -> Result<(), StorageError> {
        let code = unsafe { sqlite3_bind_int64(self.statement, index, value) };
        if code == SQLITE_OK {
            Ok(())
        } else {
            Err(StorageError::new(sqlite_error_message(self.connection)))
        }
    }

    fn step(&mut self) -> Result<StepResult, StorageError> {
        match unsafe { sqlite3_step(self.statement) } {
            SQLITE_ROW => Ok(StepResult::Row),
            SQLITE_DONE => Ok(StepResult::Done),
            _ => Err(StorageError::new(sqlite_error_message(self.connection))),
        }
    }

    fn expect_done(&mut self) -> Result<(), StorageError> {
        match self.step()? {
            StepResult::Done => Ok(()),
            StepResult::Row => Err(StorageError::new("statement returned a row unexpectedly")),
        }
    }

    fn column_text(&self, index: c_int) -> Result<String, StorageError> {
        self.column_optional_text(index)?
            .ok_or_else(|| StorageError::new(format!("column {index} is null")))
    }

    fn column_optional_text(&self, index: c_int) -> Result<Option<String>, StorageError> {
        let value = unsafe { sqlite3_column_text(self.statement, index) };
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(
                unsafe { CStr::from_ptr(value.cast::<c_char>()) }
                    .to_string_lossy()
                    .to_string(),
            ))
        }
    }

    fn column_i64(&self, index: c_int) -> i64 {
        unsafe { sqlite3_column_int64(self.statement, index) }
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        if !self.statement.is_null() {
            unsafe {
                sqlite3_finalize(self.statement);
            }
        }
    }
}

fn sqlite_transient() -> SqliteDestructor {
    unsafe { std::mem::transmute::<isize, SqliteDestructor>(-1) }
}

fn sqlite_error_message(connection: *mut sqlite3) -> String {
    if connection.is_null() {
        return "sqlite connection is null".to_string();
    }

    unsafe { CStr::from_ptr(sqlite3_errmsg(connection)) }
        .to_string_lossy()
        .to_string()
}

fn metadata_to_text(metadata: &Metadata) -> String {
    let capacity = metadata.iter().fold(
        metadata.len().saturating_sub(1),
        |capacity, (key, value)| {
            capacity
                .saturating_add(key.len().saturating_add(value.len()).saturating_mul(2))
                .saturating_add(1)
        },
    );
    let mut encoded = String::with_capacity(capacity);
    for (index, (key, value)) in metadata.iter().enumerate() {
        if index > 0 {
            encoded.push('\n');
        }
        hex_encode_into(&mut encoded, key.as_bytes());
        encoded.push('\t');
        hex_encode_into(&mut encoded, value.as_bytes());
    }
    encoded
}

fn metadata_from_text(text: &str) -> Result<Metadata, StorageError> {
    let mut metadata = Metadata::new();
    if text.is_empty() {
        return Ok(metadata);
    }

    for line in text.lines() {
        let (key, value) = line
            .split_once('\t')
            .ok_or_else(|| StorageError::new("invalid metadata record"))?;
        metadata.insert(
            String::from_utf8(hex_decode(key)?)
                .map_err(|error| StorageError::new(error.to_string()))?,
            String::from_utf8(hex_decode(value)?)
                .map_err(|error| StorageError::new(error.to_string()))?,
        );
    }

    Ok(metadata)
}

fn permission_capability_key(request: &PermissionRequest) -> Option<String> {
    if request.action != "shell.run" {
        return None;
    }
    let command = request.metadata.get("command")?;
    let environment_policy = request
        .metadata
        .get("environment_policy")
        .map(String::as_str)
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    for field in [
        SHELL_PERMISSION_CAPABILITY_KEY_VERSION,
        request.action.as_str(),
        permission_risk_to_str(&request.risk),
        environment_policy,
        command.as_str(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    Some(format!(
        "{SHELL_PERMISSION_CAPABILITY_KEY_VERSION}:{}",
        hex_encode(&hasher.finalize())
    ))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    hex_encode_into(&mut encoded, bytes);
    encoded
}

fn hex_encode_into(encoded: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn hex_decode(value: &str) -> Result<Vec<u8>, StorageError> {
    if !value.len().is_multiple_of(2) {
        return Err(StorageError::new("hex value has odd length"));
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars = value.as_bytes();
    for chunk in chars.chunks_exact(2) {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_value(value: u8) -> Result<u8, StorageError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(StorageError::new("invalid hex digit")),
    }
}

fn event_kind_to_str(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::TaskCreated => "task_created",
        EventKind::TaskStatusChanged => "task_status_changed",
        EventKind::MessageAdded => "message_added",
        EventKind::ModelRequestStarted => "model_request_started",
        EventKind::ModelRequestFinished => "model_request_finished",
        EventKind::ToolCallProposed => "tool_call_proposed",
        EventKind::ToolCallStarted => "tool_call_started",
        EventKind::ToolCallFinished => "tool_call_finished",
        EventKind::PermissionRequested => "permission_requested",
        EventKind::PermissionResolved => "permission_resolved",
        EventKind::RetrievalPerformed => "retrieval_performed",
        EventKind::Error => "error",
    }
}

fn str_to_event_kind(value: &str) -> Result<EventKind, StorageError> {
    match value {
        "task_created" => Ok(EventKind::TaskCreated),
        "task_status_changed" => Ok(EventKind::TaskStatusChanged),
        "message_added" => Ok(EventKind::MessageAdded),
        "model_request_started" => Ok(EventKind::ModelRequestStarted),
        "model_request_finished" => Ok(EventKind::ModelRequestFinished),
        "tool_call_proposed" => Ok(EventKind::ToolCallProposed),
        "tool_call_started" => Ok(EventKind::ToolCallStarted),
        "tool_call_finished" => Ok(EventKind::ToolCallFinished),
        "permission_requested" => Ok(EventKind::PermissionRequested),
        "permission_resolved" => Ok(EventKind::PermissionResolved),
        "retrieval_performed" => Ok(EventKind::RetrievalPerformed),
        "error" => Ok(EventKind::Error),
        other => Err(StorageError::new(format!("unknown event kind: {other}"))),
    }
}

fn permission_risk_to_str(risk: &PermissionRisk) -> &'static str {
    match risk {
        PermissionRisk::Read => "read",
        PermissionRisk::Write => "write",
        PermissionRisk::Execute => "execute",
        PermissionRisk::Network => "network",
        PermissionRisk::Sensitive => "sensitive",
        PermissionRisk::Destructive => "destructive",
    }
}

fn str_to_permission_risk(value: &str) -> Result<PermissionRisk, StorageError> {
    match value {
        "read" => Ok(PermissionRisk::Read),
        "write" => Ok(PermissionRisk::Write),
        "execute" => Ok(PermissionRisk::Execute),
        "network" => Ok(PermissionRisk::Network),
        "sensitive" => Ok(PermissionRisk::Sensitive),
        "destructive" => Ok(PermissionRisk::Destructive),
        other => Err(StorageError::new(format!(
            "unknown permission risk: {other}"
        ))),
    }
}

fn permission_decision_to_str(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::AllowOnce => "allow_once",
        PermissionDecision::AllowForSession => "allow_for_session",
        PermissionDecision::Deny => "deny",
    }
}

fn str_to_permission_decision(value: &str) -> Result<PermissionDecision, StorageError> {
    match value {
        "allow_once" => Ok(PermissionDecision::AllowOnce),
        "allow_for_session" => Ok(PermissionDecision::AllowForSession),
        "deny" => Ok(PermissionDecision::Deny),
        other => Err(StorageError::new(format!(
            "unknown permission decision: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{PermissionRisk, TaskId};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn metadata_codec_keeps_the_existing_wire_format() {
        let metadata = [
            ("alpha".to_string(), "one\nline".to_string()),
            ("空".to_string(), "值🙂".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();

        let encoded = metadata_to_text(&metadata);
        assert_eq!(
            encoded,
            "616c706861\t6f6e650a6c696e65\ne7a9ba\te580bcf09f9982"
        );
        assert_eq!(
            metadata_from_text(&encoded).expect("metadata should decode"),
            metadata
        );
    }

    #[test]
    fn metadata_codec_round_trips_empty_keys_and_values() {
        assert_eq!(metadata_to_text(&Metadata::new()), "");
        assert_eq!(
            metadata_from_text("").expect("empty metadata should decode"),
            Metadata::new()
        );

        let metadata = [
            (String::new(), String::new()),
            ("line\nbreak".to_string(), "tab\tvalue".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        assert_eq!(
            metadata_from_text(&metadata_to_text(&metadata)).expect("metadata should round trip"),
            metadata
        );
    }

    #[test]
    fn appends_and_lists_events_by_task() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-1".to_string());
        let mut metadata = Metadata::new();
        metadata.insert("tool".to_string(), "shell.run".to_string());

        let sequence = store.next_sequence(&task_id).expect("sequence");
        store
            .append(Event {
                id: EventId("event-1".to_string()),
                task_id: task_id.clone(),
                sequence,
                timestamp_ms: 100,
                kind: EventKind::PermissionRequested,
                summary: "Permission requested".to_string(),
                metadata,
            })
            .expect("append should succeed");

        let events = store.list_by_task(&task_id).expect("events should load");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(
            events[0].metadata.get("tool"),
            Some(&"shell.run".to_string())
        );
    }

    #[test]
    fn append_next_event_assigns_monotonic_task_scoped_sequences() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_a = TaskId("task-a".to_string());
        let task_b = TaskId("task-b".to_string());

        for (event_id, task_id) in [
            ("event-a1", task_a.clone()),
            ("event-b1", task_b.clone()),
            ("event-a2", task_a.clone()),
        ] {
            store
                .append_next_event(
                    EventId(event_id.to_string()),
                    task_id,
                    100,
                    EventKind::MessageAdded,
                    "message".to_string(),
                    Metadata::new(),
                )
                .expect("event should append");
        }

        let task_a_sequences = store
            .list_by_task(&task_a)
            .expect("task A events should load")
            .into_iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        let task_b_sequences = store
            .list_by_task(&task_b)
            .expect("task B events should load")
            .into_iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();

        assert_eq!(task_a_sequences, vec![1, 2]);
        assert_eq!(task_b_sequences, vec![1]);
    }

    #[test]
    fn opens_an_independent_read_only_connection() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cindx-agent-storage-read-only-{}-{unique}.sqlite3",
            std::process::id()
        ));
        let task_id = TaskId("task-read-only".to_string());
        let mut writer = SqliteStore::open(&path).expect("store should open");
        let mut journal_mode = writer.prepare("pragma journal_mode").expect("journal mode");
        assert_eq!(
            journal_mode.step().expect("journal mode row"),
            StepResult::Row
        );
        assert_eq!(
            journal_mode.column_text(0).expect("journal mode text"),
            "wal"
        );
        drop(journal_mode);
        writer
            .append(Event {
                id: EventId("event-read-only".to_string()),
                task_id: task_id.clone(),
                sequence: 1,
                timestamp_ms: 100,
                kind: EventKind::MessageAdded,
                summary: "message".to_string(),
                metadata: Metadata::new(),
            })
            .expect("event should append");

        let reader = SqliteStore::open_read_only(&path).expect("read-only store should open");
        assert_eq!(
            reader
                .list_by_task(&task_id)
                .expect("events should load")
                .len(),
            1
        );
        writer
            .append(Event {
                id: EventId("event-with-reader".to_string()),
                task_id: task_id.clone(),
                sequence: 2,
                timestamp_ms: 200,
                kind: EventKind::MessageAdded,
                summary: "message while reader is open".to_string(),
                metadata: Metadata::new(),
            })
            .expect("writer should append while reader is open");
        assert_eq!(
            reader
                .list_by_task(&task_id)
                .expect("reader should observe the next event")
                .len(),
            2
        );
        drop(reader);
        drop(writer);
        fs::remove_file(path).expect("temporary database should be removed");
    }

    #[test]
    fn read_snapshot_stays_consistent_while_writer_appends() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cindx-agent-storage-read-snapshot-{}-{unique}.sqlite3",
            std::process::id()
        ));
        let task_id = TaskId("task-read-snapshot".to_string());
        let mut writer = SqliteStore::open(&path).expect("store should open");
        writer
            .append(Event {
                id: EventId("event-before-snapshot".to_string()),
                task_id: task_id.clone(),
                sequence: 1,
                timestamp_ms: 100,
                kind: EventKind::MessageAdded,
                summary: "before snapshot".to_string(),
                metadata: Metadata::new(),
            })
            .expect("initial event should append");

        let reader = SqliteStore::open_read_only(&path).expect("read-only store should open");
        reader
            .with_read_snapshot(|snapshot| {
                assert_eq!(
                    snapshot
                        .list_by_task(&task_id)
                        .expect("snapshot events should load")
                        .len(),
                    1
                );
                writer
                    .append(Event {
                        id: EventId("event-during-snapshot".to_string()),
                        task_id: task_id.clone(),
                        sequence: 2,
                        timestamp_ms: 200,
                        kind: EventKind::MessageAdded,
                        summary: "during snapshot".to_string(),
                        metadata: Metadata::new(),
                    })
                    .expect("writer should append during read snapshot");
                assert_eq!(
                    snapshot
                        .list_by_task(&task_id)
                        .expect("snapshot should remain stable")
                        .len(),
                    1
                );
                Ok(())
            })
            .expect("read snapshot should complete");
        assert_eq!(
            reader
                .list_by_task(&task_id)
                .expect("reader should advance after snapshot")
                .len(),
            2
        );

        drop(reader);
        drop(writer);
        fs::remove_file(path).expect("temporary database should be removed");
    }

    #[test]
    fn lists_and_updates_persisted_event_content() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let mut metadata = Metadata::new();
        metadata.insert("output".to_string(), "api_key=secret".to_string());
        metadata.insert("queue_id".to_string(), "queue-before".to_string());
        let mut event = Event {
            id: EventId("event-sensitive".to_string()),
            task_id: TaskId("task-sensitive".to_string()),
            sequence: 1,
            timestamp_ms: 200,
            kind: EventKind::ToolCallFinished,
            summary: "sensitive output".to_string(),
            metadata,
        };
        store.append(event.clone()).expect("append should succeed");

        event.summary = "redacted output".to_string();
        event
            .metadata
            .insert("output".to_string(), "api_key=[REDACTED]".to_string());
        event
            .metadata
            .insert("queue_id".to_string(), "queue-after".to_string());
        store
            .update_event_content(&event)
            .expect("update should succeed");
        let events = store.list_all_events().expect("events should load");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "redacted output");
        assert_eq!(
            events[0].metadata.get("output").map(String::as_str),
            Some("api_key=[REDACTED]")
        );
        assert!(store
            .list_by_task_and_metadata(&event.task_id, "queue_id", "queue-before")
            .expect("old queue scope should query")
            .is_empty());
        assert_eq!(
            store
                .list_by_task_and_metadata(&event.task_id, "queue_id", "queue-after")
                .expect("new queue scope should query")
                .len(),
            1
        );
    }

    #[test]
    fn persists_permission_request_and_resolution() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let request_id = PermissionRequestId("perm-1".to_string());

        store
            .save_permission_request(
                PermissionRequest {
                    id: request_id.clone(),
                    task_id: TaskId("task-1".to_string()),
                    risk: PermissionRisk::Execute,
                    action: "shell.run".to_string(),
                    reason: "Demonstrate approval flow".to_string(),
                    scope: "/tmp/project".to_string(),
                    metadata: Metadata::new(),
                },
                200,
            )
            .expect("request should save");

        store
            .resolve_permission(PermissionResolution {
                request_id: request_id.clone(),
                decision: PermissionDecision::AllowOnce,
                resolved_at_ms: 300,
                resolved_by: "user".to_string(),
            })
            .expect("resolution should save");

        let audits = store.list_permission_audits().expect("audits should list");

        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].request.id, request_id);
        assert_eq!(
            audits[0]
                .resolution
                .as_ref()
                .map(|resolution| &resolution.decision),
            Some(&PermissionDecision::AllowOnce)
        );
    }

    #[test]
    fn permission_resolution_can_share_and_rollback_a_caller_transaction() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let request_id = PermissionRequestId("perm-atomic".to_string());
        store
            .save_permission_request(
                PermissionRequest {
                    id: request_id.clone(),
                    task_id: TaskId("task-atomic".to_string()),
                    risk: PermissionRisk::Execute,
                    action: "shell.run".to_string(),
                    reason: "Verify atomic resolution".to_string(),
                    scope: "/tmp/project".to_string(),
                    metadata: Metadata::new(),
                },
                200,
            )
            .expect("request should save");
        let resolution = PermissionResolution {
            request_id: request_id.clone(),
            decision: PermissionDecision::Deny,
            resolved_at_ms: 300,
            resolved_by: "user".to_string(),
        };

        let failure = store.with_immediate_transaction(|transaction| {
            transaction.resolve_permission_in_transaction(&resolution)?;
            Err::<(), _>(StorageError::new("injected event failure"))
        });
        assert!(failure.is_err());
        assert!(
            store.list_permission_audits().expect("audits should load")[0]
                .resolution
                .is_none()
        );

        store
            .with_immediate_transaction(|transaction| {
                transaction.resolve_permission_in_transaction(&resolution)
            })
            .expect("resolution should commit with its caller");
        assert_eq!(
            store.list_permission_audits().expect("audits should load")[0]
                .resolution
                .as_ref()
                .map(|resolved| &resolved.decision),
            Some(&PermissionDecision::Deny)
        );
    }

    #[test]
    fn lists_permission_audits_by_indexed_session_and_run() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-agent".to_string());
        for (id, session_id, run_id, requested_at_ms) in [
            ("perm-a1", "session-a", "run-a1", 100),
            ("perm-a2", "session-a", "run-a2", 300),
            ("perm-b1", "session-b", "run-b1", 400),
        ] {
            store
                .save_permission_request(
                    PermissionRequest {
                        id: PermissionRequestId(id.to_string()),
                        task_id: task_id.clone(),
                        risk: PermissionRisk::Read,
                        action: "file.read".to_string(),
                        reason: "test".to_string(),
                        scope: ".".to_string(),
                        metadata: [
                            ("session_id".to_string(), session_id.to_string()),
                            ("agent_run_id".to_string(), run_id.to_string()),
                        ]
                        .into_iter()
                        .collect(),
                    },
                    requested_at_ms,
                )
                .expect("permission should save");
        }

        let session = store
            .list_permission_audits_for_session(&task_id, "session-a", None, 0)
            .expect("session audits should list");
        let active_run = store
            .list_permission_audits_for_session(&task_id, "session-a", Some("run-a2"), 0)
            .expect("run audits should list");
        let recent = store
            .list_permission_audits_for_session(&task_id, "session-a", None, 200)
            .expect("recent audits should list");

        assert_eq!(session.len(), 2);
        assert_eq!(active_run.len(), 1);
        assert_eq!(active_run[0].request.id.0, "perm-a2");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].request.id.0, "perm-a2");
    }

    #[test]
    fn permission_index_remains_bound_to_the_physical_attempt() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-permission-attempt".to_string());
        store
            .save_permission_request(
                PermissionRequest {
                    id: PermissionRequestId("perm-attempt".to_string()),
                    task_id: task_id.clone(),
                    risk: PermissionRisk::Read,
                    action: "file.read".to_string(),
                    reason: "test".to_string(),
                    scope: ".".to_string(),
                    metadata: [
                        ("session_id".to_string(), "session".to_string()),
                        ("agent_run_id".to_string(), "attempt-b".to_string()),
                        (
                            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                            "logical-a".to_string(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                100,
            )
            .expect("permission should save");

        assert_eq!(
            store
                .list_permission_audits_for_session(&task_id, "session", Some("attempt-b"), 0,)
                .expect("physical attempt should load")
                .len(),
            1
        );
        assert!(store
            .list_permission_audits_for_session(&task_id, "session", Some("logical-a"), 0)
            .expect("logical run must not become a permission scope")
            .is_empty());

        let mut columns = store
            .prepare("pragma table_info(permission_requests)")
            .unwrap();
        let mut names = Vec::new();
        while columns.step().unwrap() == StepResult::Row {
            names.push(columns.column_text(1).unwrap());
        }
        assert!(!names
            .iter()
            .any(|column| column == LOGICAL_AGENT_RUN_ID_METADATA_KEY));
    }

    #[test]
    fn checks_session_capabilities_with_risk_action_scope_and_key() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-agent".to_string());
        let request_id = PermissionRequestId("perm-shell".to_string());
        let request = PermissionRequest {
            id: request_id.clone(),
            task_id: task_id.clone(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "test".to_string(),
            scope: "/workspace".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("command".to_string(), "cargo test".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        store
            .save_permission_request(request.clone(), 100)
            .expect("permission should save");
        store
            .resolve_permission(PermissionResolution {
                request_id,
                decision: PermissionDecision::AllowForSession,
                resolved_at_ms: 110,
                resolved_by: "user".to_string(),
            })
            .expect("permission should resolve");

        assert!(store
            .has_session_permission_capability(&task_id, "session-a", &request, true, true,)
            .expect("capability query should succeed"));
        let mut another_scope = request.clone();
        another_scope.scope = "/other".to_string();
        assert!(!store
            .has_session_permission_capability(&task_id, "session-a", &another_scope, true, true,)
            .expect("scope query should succeed"));
        assert!(store
            .has_session_permission_capability(&task_id, "session-a", &another_scope, false, true,)
            .expect("unscoped query should succeed"));
        let mut another_command = request.clone();
        another_command
            .metadata
            .insert("command".to_string(), "cargo build".to_string());
        assert!(!store
            .has_session_permission_capability(&task_id, "session-a", &another_command, true, true,)
            .expect("capability-key query should succeed"));
        let mut another_risk = request;
        another_risk.risk = PermissionRisk::Write;
        assert!(!store
            .has_session_permission_capability(&task_id, "session-a", &another_risk, false, true,)
            .expect("risk query should succeed"));
    }

    #[test]
    fn shell_capability_keys_are_versioned_digests_without_raw_commands() {
        let request = PermissionRequest {
            id: PermissionRequestId("perm-shell-key".to_string()),
            task_id: TaskId("task-agent".to_string()),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "test".to_string(),
            scope: "/workspace".to_string(),
            metadata: [
                ("command".to_string(), "cargo test --workspace".to_string()),
                (
                    "environment_policy".to_string(),
                    "developer_safe_v1".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let key = permission_capability_key(&request).expect("shell command should hash");
        assert!(key.starts_with("shell-command-v1:"));
        assert_eq!(key.len(), "shell-command-v1:".len() + 64);
        assert!(!key.contains("cargo"));
        assert_eq!(permission_capability_key(&request), Some(key.clone()));

        let mut changed = request.clone();
        changed
            .metadata
            .insert("command".to_string(), "cargo build".to_string());
        assert_ne!(permission_capability_key(&changed), Some(key.clone()));
        changed.metadata.remove("command");
        assert_eq!(permission_capability_key(&changed), None);

        let mut other_policy = request;
        other_policy
            .metadata
            .insert("environment_policy".to_string(), "future_v2".to_string());
        assert_ne!(permission_capability_key(&other_policy), Some(key));
    }

    #[test]
    fn migration_backfills_legacy_shell_keys_as_digests() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-agent".to_string());
        let request_id = PermissionRequestId("legacy-shell".to_string());
        let request = PermissionRequest {
            id: request_id.clone(),
            task_id: task_id.clone(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "legacy".to_string(),
            scope: "/workspace".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("command".to_string(), "cargo test".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        store
            .save_permission_request(request.clone(), 100)
            .expect("permission should save");
        store
            .resolve_permission(PermissionResolution {
                request_id,
                decision: PermissionDecision::AllowForSession,
                resolved_at_ms: 110,
                resolved_by: "user".to_string(),
            })
            .expect("permission should resolve");
        store
            .exec_batch(
                "update permission_requests set capability_key = null where id = 'legacy-shell';
                 delete from storage_meta where key = 'permission_capability_key_shell_sha256_v1';",
            )
            .expect("legacy row should be staged");

        store
            .backfill_permission_capability_keys()
            .expect("legacy capability should backfill");

        let mut statement = store
            .prepare("select capability_key from permission_requests where id = ?1")
            .expect("capability query should prepare");
        statement
            .bind_text(1, "legacy-shell")
            .expect("request id should bind");
        assert_eq!(
            statement.step().expect("capability query should run"),
            StepResult::Row
        );
        let stored_key = statement.column_text(0).expect("key should load");
        assert!(stored_key.starts_with("shell-command-v1:"));
        assert!(!stored_key.contains("cargo test"));
        drop(statement);
        assert!(store
            .has_session_permission_capability(&task_id, "session-a", &request, true, true)
            .expect("backfilled grant should match"));
    }

    #[test]
    fn indexed_shell_capability_lookup_fails_closed_without_a_command() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-agent".to_string());
        let request_id = PermissionRequestId("missing-command".to_string());
        let request = PermissionRequest {
            id: request_id.clone(),
            task_id: task_id.clone(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "test".to_string(),
            scope: "/workspace".to_string(),
            metadata: [("session_id".to_string(), "session-a".to_string())]
                .into_iter()
                .collect(),
        };
        store
            .save_permission_request(request.clone(), 100)
            .expect("permission should save");
        store
            .resolve_permission(PermissionResolution {
                request_id,
                decision: PermissionDecision::AllowForSession,
                resolved_at_ms: 110,
                resolved_by: "user".to_string(),
            })
            .expect("permission should resolve");

        assert!(!store
            .has_session_permission_capability(&task_id, "session-a", &request, true, true)
            .expect("missing command should fail closed"));
    }

    #[test]
    fn shell_capability_lookup_uses_the_composite_key_index() {
        let store = SqliteStore::in_memory().expect("store should open");
        let mut statement = store
            .prepare(
                "explain query plan
                 select 1
                 from permission_requests pr
                 inner join permission_resolutions rr on rr.request_id = pr.id
                 where pr.task_id = ?1
                   and pr.session_id = ?2
                   and pr.risk = ?3
                   and pr.action = ?4
                   and pr.scope = ?5
                   and pr.capability_key = ?6
                   and rr.decision = 'allow_for_session'
                 limit 1",
            )
            .expect("query plan should prepare");
        for (index, value) in [
            "task-agent",
            "session-a",
            "execute",
            "shell.run",
            "/workspace",
            "shell-command-v1:digest",
        ]
        .into_iter()
        .enumerate()
        {
            statement
                .bind_text((index + 1) as c_int, value)
                .expect("query parameter should bind");
        }
        let mut details = Vec::new();
        while statement.step().expect("query plan should run") == StepResult::Row {
            details.push(statement.column_text(3).expect("plan detail should load"));
        }
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("idx_permission_requests_session_capability_key")),
            "query plan should use the composite capability-key index: {details:?}"
        );
    }

    #[test]
    fn deletes_events_and_permissions_for_one_session() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-1".to_string());
        for (event_id, session_id) in [("event-a", "session-a"), ("event-b", "session-b")] {
            let sequence = store.next_sequence(&task_id).expect("sequence");
            store
                .append(Event {
                    id: EventId(event_id.to_string()),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms: 100,
                    kind: EventKind::MessageAdded,
                    summary: "message".to_string(),
                    metadata: [("session_id".to_string(), session_id.to_string())]
                        .into_iter()
                        .collect(),
                })
                .expect("event should append");
        }
        for (request_id, session_id) in [("perm-a", "session-a"), ("perm-b", "session-b")] {
            store
                .save_permission_request(
                    PermissionRequest {
                        id: PermissionRequestId(request_id.to_string()),
                        task_id: task_id.clone(),
                        risk: PermissionRisk::Write,
                        action: "file.write".to_string(),
                        reason: "test".to_string(),
                        scope: ".".to_string(),
                        metadata: [("session_id".to_string(), session_id.to_string())]
                            .into_iter()
                            .collect(),
                    },
                    200,
                )
                .expect("permission should save");
        }
        store
            .resolve_permission(PermissionResolution {
                request_id: PermissionRequestId("perm-a".to_string()),
                decision: PermissionDecision::AllowOnce,
                resolved_at_ms: 210,
                resolved_by: "user".to_string(),
            })
            .expect("permission should resolve");

        store
            .delete_records_by_metadata("session_id", "session-a")
            .expect("session records should delete");

        let events = store.list_by_task(&task_id).expect("events should load");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].metadata.get("session_id").map(String::as_str),
            Some("session-b")
        );
        let audits = store.list_permission_audits().expect("audits should load");
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].request.id.0, "perm-b");
    }

    #[test]
    fn transaction_scoped_delete_rolls_back_events_permissions_and_read_models_together() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("delete-transaction-task".to_string());
        for (event_id, session_id) in [
            ("delete-event-a", "session-a"),
            ("keep-event-b", "session-b"),
        ] {
            store
                .append_next_event(
                    EventId(event_id.to_string()),
                    task_id.clone(),
                    100,
                    EventKind::MessageAdded,
                    "message".to_string(),
                    [("session_id".to_string(), session_id.to_string())]
                        .into_iter()
                        .collect(),
                )
                .expect("event should append");
        }
        let request_id = PermissionRequestId("delete-permission-a".to_string());
        store
            .save_permission_request(
                PermissionRequest {
                    id: request_id.clone(),
                    task_id: task_id.clone(),
                    risk: PermissionRisk::Write,
                    action: "file.write".to_string(),
                    reason: "test".to_string(),
                    scope: ".".to_string(),
                    metadata: [("session_id".to_string(), "session-a".to_string())]
                        .into_iter()
                        .collect(),
                },
                200,
            )
            .expect("permission should save");
        store
            .resolve_permission(PermissionResolution {
                request_id,
                decision: PermissionDecision::AllowOnce,
                resolved_at_ms: 210,
                resolved_by: "user".to_string(),
            })
            .expect("permission should resolve");
        store
            .save_read_model("session-snapshot", "session-a", 1, "snapshot")
            .expect("read model should save");

        let outside_transaction = store
            .delete_records_by_metadata_in_transaction("session_id", "session-a")
            .expect_err("composable delete must require a transaction");
        assert!(outside_transaction.message.contains("active transaction"));

        let rolled_back = store.with_immediate_transaction(|transaction| {
            transaction.delete_records_by_metadata_in_transaction("session_id", "session-a")?;
            transaction.delete_read_model("session-snapshot", "session-a")?;
            Err::<(), _>(StorageError::new("injected lifecycle failure"))
        });
        assert!(rolled_back.is_err());

        let events = store.list_by_task(&task_id).expect("events should load");
        assert_eq!(events.len(), 2);
        assert_eq!(
            store
                .list_permission_audits()
                .expect("permission audits should load")
                .len(),
            1
        );
        assert!(store
            .load_read_model("session-snapshot", "session-a")
            .expect("read model should load")
            .is_some());
    }

    #[test]
    fn reads_a_lightweight_revision_for_one_session() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-revision".to_string());
        for (sequence, session_id, timestamp_ms) in [
            (1, "session-a", 100),
            (2, "session-b", 200),
            (3, "session-a", 300),
        ] {
            store
                .append(Event {
                    id: EventId(format!("event-{sequence}")),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms,
                    kind: EventKind::MessageAdded,
                    summary: "message".to_string(),
                    metadata: [("session_id".to_string(), session_id.to_string())]
                        .into_iter()
                        .collect(),
                })
                .expect("event should append");
        }

        let revision = store
            .event_revision_by_metadata(&task_id, "session_id", "session-a")
            .expect("revision should load");
        assert_eq!(revision.event_count, 2);
        assert_eq!(revision.latest_sequence, 3);
        assert_eq!(revision.latest_timestamp_ms, 300);
    }

    #[test]
    fn reads_the_indexed_task_tail_without_counting_history() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-tail".to_string());
        assert_eq!(store.latest_sequence(&task_id).unwrap(), 0);
        for sequence in [1, 2, 7] {
            store
                .append(Event {
                    id: EventId(format!("tail-event-{sequence}")),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms: sequence,
                    kind: EventKind::TaskStatusChanged,
                    summary: "tail".to_string(),
                    metadata: Metadata::new(),
                })
                .expect("event should append");
        }

        assert_eq!(store.latest_sequence(&task_id).unwrap(), 7);

        let mut statement = store
            .prepare(
                "explain query plan
                 select sequence from events
                 where task_id = ?1
                 order by sequence desc limit 1",
            )
            .unwrap();
        statement.bind_text(1, &task_id.0).unwrap();
        let mut plan = Vec::new();
        while statement.step().unwrap() == StepResult::Row {
            plan.push(statement.column_text(3).unwrap());
        }
        assert!(plan
            .iter()
            .any(|step| step.contains("idx_events_task_sequence")));
    }

    #[test]
    fn reads_only_requested_event_kinds_in_sequence_order() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-kinds".to_string());
        for (sequence, kind) in [
            (1, EventKind::MessageAdded),
            (2, EventKind::TaskStatusChanged),
            (3, EventKind::ToolCallFinished),
            (4, EventKind::Error),
        ] {
            store
                .append(Event {
                    id: EventId(format!("event-{sequence}")),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms: sequence * 100,
                    kind,
                    summary: format!("event {sequence}"),
                    metadata: Metadata::new(),
                })
                .expect("event should append");
        }

        let events = store
            .list_by_task_and_kinds(&task_id, &[EventKind::TaskStatusChanged, EventKind::Error])
            .expect("filtered events should load");

        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 4]
        );
        assert!(store
            .list_by_task_and_kinds(&task_id, &[])
            .expect("empty filter should load")
            .is_empty());
    }

    #[test]
    fn reads_only_one_session_while_preserving_legacy_unscoped_events() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-session-scope".to_string());
        for (sequence, session_id) in [
            (1, Some("session-a")),
            (2, None),
            (3, Some("session-b")),
            (4, Some("session-a")),
        ] {
            let metadata = session_id
                .map(|session_id| {
                    [("session_id".to_string(), session_id.to_string())]
                        .into_iter()
                        .collect()
                })
                .unwrap_or_default();
            store
                .append(Event {
                    id: EventId(format!("event-{sequence}")),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms: sequence * 100,
                    kind: EventKind::MessageAdded,
                    summary: "message".to_string(),
                    metadata,
                })
                .expect("event should append");
        }

        let exact = store
            .list_by_task_and_metadata(&task_id, "session_id", "session-a")
            .expect("scoped events should load");
        assert_eq!(
            exact.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            vec![1, 4]
        );

        let compatible = store
            .list_by_task_and_metadata_or_unscoped(&task_id, "session_id", "session-a")
            .expect("compatible scoped events should load");
        assert_eq!(
            compatible
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 4]
        );

        let delta = store
            .list_by_task_and_metadata_or_unscoped_after(&task_id, "session_id", "session-a", 2)
            .expect("session delta should load");
        assert_eq!(
            delta.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            vec![4]
        );
    }

    #[test]
    fn backfills_scope_columns_for_legacy_event_rows() {
        let store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-legacy-scope".to_string());
        let metadata = [
            ("project_id".to_string(), "legacy-project".to_string()),
            ("session_id".to_string(), "legacy-session".to_string()),
            ("queue_id".to_string(), "legacy-queue".to_string()),
            (
                "result_input_fingerprint".to_string(),
                "legacy-effect".to_string(),
            ),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut statement = store
            .prepare(
                "insert into events(
                   id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                 ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .expect("legacy insert should prepare");
        statement.bind_text(1, "legacy-event").unwrap();
        statement.bind_text(2, &task_id.0).unwrap();
        statement.bind_i64(3, 1).unwrap();
        statement.bind_i64(4, 100).unwrap();
        statement.bind_text(5, "message_added").unwrap();
        statement.bind_text(6, "legacy message").unwrap();
        statement
            .bind_text(7, &metadata_to_text(&metadata))
            .unwrap();
        statement.expect_done().unwrap();
        drop(statement);
        store
            .exec_batch(
                "delete from storage_meta
                 where key in ('event_scope_columns_v3', 'event_queue_scope_v1')",
            )
            .unwrap();

        store.backfill_event_scope_columns().unwrap();
        store.backfill_event_queue_scope_column().unwrap();

        let events = store
            .list_by_task_and_metadata(&task_id, "session_id", "legacy-session")
            .unwrap();
        assert_eq!(events.len(), 1);
        let queue_events = store
            .list_by_task_and_metadata_values_in_scope(
                &task_id,
                "queue_id",
                &["legacy-queue".to_string()],
                "session_id",
                "legacy-session",
            )
            .unwrap();
        assert_eq!(queue_events.len(), 1);
        assert_eq!(events[0].id.0, "legacy-event");
        assert_eq!(
            store
                .list_by_task_and_metadata(&task_id, "project_id", "legacy-project")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_by_task_and_effect_fingerprint(&task_id, "legacy-effect")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn backfills_only_explicit_logical_run_metadata() {
        let store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-logical-backfill".to_string());
        for (event_id, sequence, metadata) in [
            (
                "explicit-logical",
                1,
                [
                    ("agent_run_id".to_string(), "attempt-b".to_string()),
                    (
                        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                        "logical-a".to_string(),
                    ),
                ]
                .into_iter()
                .collect::<Metadata>(),
            ),
            (
                "legacy-lineage",
                2,
                [
                    ("agent_run_id".to_string(), "attempt-c".to_string()),
                    ("source_agent_run_id".to_string(), "attempt-b".to_string()),
                ]
                .into_iter()
                .collect::<Metadata>(),
            ),
        ] {
            let mut statement = store
                .prepare(
                    "insert into events(
                       id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                     ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .expect("legacy insert should prepare");
            statement.bind_text(1, event_id).unwrap();
            statement.bind_text(2, &task_id.0).unwrap();
            statement.bind_i64(3, sequence).unwrap();
            statement.bind_i64(4, sequence * 100).unwrap();
            statement.bind_text(5, "task_status_changed").unwrap();
            statement.bind_text(6, "run event").unwrap();
            statement
                .bind_text(7, &metadata_to_text(&metadata))
                .unwrap();
            statement.expect_done().unwrap();
        }
        store
            .exec_batch("delete from storage_meta where key = 'event_logical_agent_run_scope_v1'")
            .unwrap();

        store.backfill_event_logical_run_scope_column().unwrap();
        store.backfill_event_logical_run_scope_column().unwrap();

        let explicit = store
            .list_by_task_and_metadata(&task_id, LOGICAL_AGENT_RUN_ID_METADATA_KEY, "logical-a")
            .unwrap();
        assert_eq!(explicit.len(), 1);
        assert_eq!(explicit[0].id.0, "explicit-logical");
        let mut legacy = store
            .prepare(
                "select count(*) from events
                 where id = 'legacy-lineage' and logical_agent_run_id is null",
            )
            .unwrap();
        assert_eq!(legacy.step().unwrap(), StepResult::Row);
        assert_eq!(legacy.column_i64(0), 1);
    }

    #[test]
    fn appends_updates_and_queries_logical_run_scope_independently_of_attempts() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-logical-run".to_string());
        let mut continued = Event {
            id: EventId("continued".to_string()),
            task_id: task_id.clone(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "continued".to_string(),
            metadata: [
                ("agent_run_id".to_string(), "attempt-b".to_string()),
                (
                    LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                    "logical-a".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        store
            .append(continued.clone())
            .expect("event should append");
        store
            .append_next_event(
                EventId("continued-next".to_string()),
                task_id.clone(),
                200,
                EventKind::MessageAdded,
                "continued message".to_string(),
                [
                    ("agent_run_id".to_string(), "attempt-c".to_string()),
                    (
                        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                        "logical-a".to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            )
            .expect("next event should append");

        assert_eq!(
            store
                .list_by_task_and_metadata(
                    &task_id,
                    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
                    "logical-a",
                )
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .list_by_task_and_metadata(&task_id, "agent_run_id", "attempt-b")
                .unwrap()
                .len(),
            1
        );

        continued.metadata.insert(
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            "logical-repaired".to_string(),
        );
        store
            .update_event_content(&continued)
            .expect("event scope should update");
        assert_eq!(
            store
                .list_by_task_and_metadata(
                    &task_id,
                    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
                    "logical-a",
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_by_task_and_metadata(
                    &task_id,
                    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
                    "logical-repaired",
                )
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn scoped_session_queries_use_the_composite_index() {
        let store = SqliteStore::in_memory().expect("store should open");
        let mut statement = store
            .prepare(
                "explain query plan
                 select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                 from events
                 where task_id = ?1 and session_id = ?2 and sequence > ?3
                 order by sequence asc",
            )
            .unwrap();
        statement.bind_text(1, "task").unwrap();
        statement.bind_text(2, "session").unwrap();
        statement.bind_i64(3, 0).unwrap();
        let mut plan = Vec::new();
        while statement.step().unwrap() == StepResult::Row {
            plan.push(statement.column_text(3).unwrap());
        }

        assert!(plan
            .iter()
            .any(|step| step.contains("idx_events_task_session_sequence")));
    }

    #[test]
    fn scoped_project_queries_use_the_composite_index() {
        let store = SqliteStore::in_memory().expect("store should open");
        let mut statement = store
            .prepare(
                "explain query plan
                 select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                 from events
                 where task_id = ?1 and project_id = ?2 and sequence > ?3
                 order by sequence asc",
            )
            .unwrap();
        statement.bind_text(1, "task").unwrap();
        statement.bind_text(2, "project").unwrap();
        statement.bind_i64(3, 0).unwrap();
        let mut plan = Vec::new();
        while statement.step().unwrap() == StepResult::Row {
            plan.push(statement.column_text(3).unwrap());
        }

        assert!(plan
            .iter()
            .any(|step| step.contains("idx_events_task_project_sequence")));
    }

    #[test]
    fn logical_run_queries_use_the_composite_index() {
        let store = SqliteStore::in_memory().expect("store should open");
        let mut definition = store
            .prepare(
                "select sql from sqlite_master
                 where type = 'index' and name = 'idx_events_task_logical_run_sequence'",
            )
            .unwrap();
        assert_eq!(definition.step().unwrap(), StepResult::Row);
        assert!(definition
            .column_text(0)
            .unwrap()
            .to_ascii_lowercase()
            .contains("where logical_agent_run_id is not null"));

        let mut statement = store
            .prepare(
                "explain query plan
                 select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                 from events
                 where task_id = ?1 and logical_agent_run_id = ?2 and sequence > ?3
                 order by sequence asc",
            )
            .unwrap();
        statement.bind_text(1, "task").unwrap();
        statement.bind_text(2, "logical-run").unwrap();
        statement.bind_i64(3, 0).unwrap();
        let mut plan = Vec::new();
        while statement.step().unwrap() == StepResult::Row {
            plan.push(statement.column_text(3).unwrap());
        }

        assert!(plan
            .iter()
            .any(|step| step.contains("idx_events_task_logical_run_sequence")));
    }

    #[test]
    fn tool_call_queries_use_the_dedicated_index() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-tool-call".to_string());
        store
            .append(Event {
                id: EventId("event-tool-call".to_string()),
                task_id: task_id.clone(),
                sequence: 1,
                timestamp_ms: 100,
                kind: EventKind::ToolCallFinished,
                summary: "tool finished".to_string(),
                metadata: [("tool_call_id".to_string(), "call-1".to_string())]
                    .into_iter()
                    .collect(),
            })
            .expect("event should append");

        let events = store
            .list_by_task_and_tool_call_id(&task_id, "call-1")
            .expect("tool call events should load");
        assert_eq!(events.len(), 1);

        let mut statement = store
            .prepare(
                "explain query plan
                 select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                 from events
                 where task_id = ?1 and tool_call_id = ?2
                 order by sequence asc",
            )
            .unwrap();
        statement.bind_text(1, "task").unwrap();
        statement.bind_text(2, "call").unwrap();
        let mut plan = Vec::new();
        while statement.step().unwrap() == StepResult::Row {
            plan.push(statement.column_text(3).unwrap());
        }
        assert!(plan
            .iter()
            .any(|step| step.contains("idx_events_task_tool_call_sequence")));
    }

    #[test]
    fn effect_fingerprint_queries_use_the_dedicated_index() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-effect".to_string());
        store
            .append(Event {
                id: EventId("event-effect".to_string()),
                task_id: task_id.clone(),
                sequence: 1,
                timestamp_ms: 100,
                kind: EventKind::ToolCallFinished,
                summary: "effect applied".to_string(),
                metadata: [(
                    "result_effect_fingerprint".to_string(),
                    "effect-1".to_string(),
                )]
                .into_iter()
                .collect(),
            })
            .expect("event should append");

        let events = store
            .list_by_task_and_effect_fingerprint(&task_id, "effect-1")
            .expect("effect events should load");
        assert_eq!(events.len(), 1);

        let mut statement = store
            .prepare(
                "explain query plan
                 select id, task_id, sequence, timestamp_ms, kind, summary, metadata_text
                 from events
                 where task_id = ?1 and effect_fingerprint = ?2
                 order by sequence asc",
            )
            .unwrap();
        statement.bind_text(1, "task").unwrap();
        statement.bind_text(2, "effect").unwrap();
        let mut plan = Vec::new();
        while statement.step().unwrap() == StepResult::Row {
            plan.push(statement.column_text(3).unwrap());
        }
        assert!(plan
            .iter()
            .any(|step| step.contains("idx_events_task_effect_fingerprint_sequence")));
    }

    #[test]
    fn pages_session_events_backwards_without_changing_display_order() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-history-page".to_string());
        for sequence in 1..=8 {
            store
                .append(Event {
                    id: EventId(format!("event-{sequence}")),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms: sequence * 100,
                    kind: EventKind::MessageAdded,
                    summary: "message".to_string(),
                    metadata: [("session_id".to_string(), "session-a".to_string())]
                        .into_iter()
                        .collect(),
                })
                .expect("event should append");
        }

        let latest = store
            .list_by_task_and_metadata_before(&task_id, "session_id", "session-a", u64::MAX, 3)
            .expect("latest page should load");
        assert_eq!(
            latest
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![6, 7, 8]
        );
        assert!(store
            .has_task_metadata_event_before(&task_id, "session_id", "session-a", 6)
            .expect("older event check should load"));

        let older = store
            .list_by_task_and_metadata_before(&task_id, "session_id", "session-a", 6, 3)
            .expect("older page should load");
        assert_eq!(
            older.iter().map(|event| event.sequence).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
    }

    #[test]
    fn bounded_history_omits_only_oversized_tool_metadata() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-bounded-history".to_string());
        for (sequence, kind, content) in [
            (1, EventKind::MessageAdded, "message content".to_string()),
            (2, EventKind::ToolCallFinished, "x".repeat(4_096)),
        ] {
            store
                .append(Event {
                    id: EventId(format!("event-{sequence}")),
                    task_id: task_id.clone(),
                    sequence,
                    timestamp_ms: sequence * 100,
                    kind,
                    summary: "history event".to_string(),
                    metadata: [
                        ("session_id".to_string(), "session-a".to_string()),
                        ("content".to_string(), content),
                    ]
                    .into_iter()
                    .collect(),
                })
                .expect("event should append");
        }

        let events = store
            .list_by_task_and_metadata_before_with_tool_metadata_limit(
                &task_id,
                "session_id",
                "session-a",
                u64::MAX,
                10,
                1_024,
            )
            .expect("bounded history should load");
        let delta = store
            .list_by_task_and_metadata_after_with_tool_metadata_limit(
                &task_id,
                "session_id",
                "session-a",
                1,
                1_024,
            )
            .expect("bounded delta should load");

        assert_eq!(
            events[0].metadata.get("content").map(String::as_str),
            Some("message content")
        );
        assert!(events[1].metadata.is_empty());
        assert!(delta[0].metadata.is_empty());
        assert_eq!(
            store
                .oversized_tool_event_ids(1_024)
                .expect("oversized ids should load"),
            vec!["event-2".to_string()]
        );
        assert_eq!(
            store
                .event_by_id("event-2")
                .expect("event should load")
                .expect("event should exist")
                .metadata
                .get("content")
                .map(String::len),
            Some(4_096)
        );
    }

    #[test]
    fn persists_versioned_read_models() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        assert!(store
            .load_read_model("agent-session-v1", "session-a")
            .expect("read model should load")
            .is_none());

        store
            .save_read_model(
                "agent-session-v1",
                "session-a",
                12,
                "{\"status\":\"running\"}",
            )
            .expect("read model should save");
        let stored = store
            .load_read_model("agent-session-v1", "session-a")
            .expect("read model should load")
            .expect("read model should exist");
        assert_eq!(stored.revision, 12);
        assert_eq!(stored.payload, "{\"status\":\"running\"}");

        store
            .save_read_model("agent-session-v1", "session-a", 13, "updated")
            .expect("read model should update");
        assert_eq!(
            store
                .load_read_model("agent-session-v1", "session-a")
                .unwrap()
                .unwrap(),
            StoredReadModel {
                revision: 13,
                payload: "updated".to_string()
            }
        );
        store
            .delete_read_model("agent-session-v1", "session-a")
            .expect("read model should delete");
        assert!(store
            .load_read_model("agent-session-v1", "session-a")
            .unwrap()
            .is_none());
    }

    #[test]
    fn compare_exchange_read_model_rejects_stale_cross_connection_publications() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "cindx-agent-storage-read-model-cas-{}-{unique}.sqlite3",
            std::process::id()
        ));
        let mut first = SqliteStore::open(&path).expect("first store should open");
        let mut second = SqliteStore::open(&path).expect("second store should open");

        assert!(first
            .compare_exchange_read_model("cas", "absent", None, 1, "initial")
            .expect("absent row should publish"));
        assert!(!second
            .compare_exchange_read_model("cas", "absent", None, 1, "competing")
            .expect("competing insert should report a conflict"));

        first
            .save_read_model("cas", "newer", 10, "old")
            .expect("baseline should save");
        let stale = second
            .load_read_model("cas", "newer")
            .expect("baseline should load")
            .expect("baseline should exist");
        assert!(first
            .compare_exchange_read_model("cas", "newer", Some(&stale), 11, "new")
            .expect("newer snapshot should publish"));
        assert!(!second
            .compare_exchange_read_model("cas", "newer", Some(&stale), 12, "stale")
            .expect("stale expected value should report a conflict"));
        assert_eq!(
            second.load_read_model("cas", "newer").unwrap().unwrap(),
            StoredReadModel {
                revision: 11,
                payload: "new".to_string(),
            }
        );

        first
            .save_read_model("cas", "same-revision", 20, "old-projection")
            .expect("old projection should save");
        let old_projection = first
            .load_read_model("cas", "same-revision")
            .unwrap()
            .unwrap();
        let competing_projection = second
            .load_read_model("cas", "same-revision")
            .unwrap()
            .unwrap();
        assert!(first
            .compare_exchange_read_model(
                "cas",
                "same-revision",
                Some(&old_projection),
                20,
                "repaired-projection",
            )
            .expect("the observed old projection should be replaceable"));
        assert!(!second
            .compare_exchange_read_model(
                "cas",
                "same-revision",
                Some(&competing_projection),
                20,
                "conflicting-projection",
            )
            .expect("same-revision payload conflict should be rejected"));
        assert_eq!(
            second
                .load_read_model("cas", "same-revision")
                .unwrap()
                .unwrap(),
            StoredReadModel {
                revision: 20,
                payload: "repaired-projection".to_string(),
            }
        );

        drop(second);
        drop(first);
        fs::remove_file(path).expect("temporary database should be removed");
    }

    #[test]
    fn immediate_transaction_commits_or_rolls_back_events_and_read_models_together() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("transaction-task".to_string());
        store
            .with_immediate_transaction(|transaction| {
                transaction.append_next_event(
                    EventId("committed-event".to_string()),
                    task_id.clone(),
                    1,
                    EventKind::MessageAdded,
                    "committed".to_string(),
                    Metadata::new(),
                )?;
                transaction.save_read_model("transaction-test", "committed", 1, "ok")
            })
            .expect("transaction should commit");
        assert_eq!(
            store
                .list_by_task(&task_id)
                .expect("events should load")
                .len(),
            1
        );

        let rolled_back = store.with_immediate_transaction(|transaction| {
            transaction.append_next_event(
                EventId("rolled-back-event".to_string()),
                task_id.clone(),
                2,
                EventKind::MessageAdded,
                "rolled back".to_string(),
                Metadata::new(),
            )?;
            transaction.save_read_model("transaction-test", "rolled-back", 2, "no")?;
            Err::<(), _>(StorageError::new("injected transaction failure"))
        });
        assert!(rolled_back.is_err());
        assert_eq!(
            store
                .list_by_task(&task_id)
                .expect("events should load")
                .len(),
            1
        );
        assert!(store
            .load_read_model("transaction-test", "rolled-back")
            .expect("read model should load")
            .is_none());
    }

    #[test]
    fn immediate_transaction_atomically_copies_selected_session_events_in_order() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("fork-transaction-task".to_string());
        for (event_id, session_id, summary) in [
            ("source-first", "source-session", "first"),
            ("other-middle", "other-session", "other"),
            ("source-second", "source-session", "second"),
        ] {
            store
                .append_next_event(
                    EventId(event_id.to_string()),
                    task_id.clone(),
                    100,
                    EventKind::MessageAdded,
                    summary.to_string(),
                    [("session_id".to_string(), session_id.to_string())]
                        .into_iter()
                        .collect(),
                )
                .expect("source event should append");
        }

        let rollback = store.with_immediate_transaction(|transaction| {
            let source =
                transaction.list_by_task_and_metadata(&task_id, "session_id", "source-session")?;
            assert_eq!(
                source
                    .iter()
                    .map(|event| event.summary.as_str())
                    .collect::<Vec<_>>(),
                vec!["first", "second"]
            );
            for (index, event) in source.into_iter().enumerate() {
                let mut metadata = event.metadata;
                metadata.insert("session_id".to_string(), "fork-session".to_string());
                metadata.insert(
                    "forked_from_session_id".to_string(),
                    "source-session".to_string(),
                );
                transaction.append_next_event(
                    EventId(format!("rolled-back-fork-{index}")),
                    task_id.clone(),
                    200,
                    event.kind,
                    event.summary,
                    metadata,
                )?;
            }
            Err::<(), _>(StorageError::new("injected fork failure"))
        });
        assert!(rollback.is_err());
        assert!(store
            .list_by_task_and_metadata(&task_id, "session_id", "fork-session")
            .expect("fork events should load")
            .is_empty());
        assert_eq!(
            store
                .list_by_task_and_metadata(&task_id, "session_id", "other-session")
                .expect("unrelated events should load")
                .len(),
            1
        );
    }

    #[test]
    fn immediate_transaction_rolls_back_after_commit_failure() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        store
            .exec_batch(
                "
                pragma foreign_keys = ON;
                create table transaction_parent(id integer primary key);
                create table transaction_child(
                  parent_id integer references transaction_parent(id)
                    deferrable initially deferred
                );
                ",
            )
            .expect("test schema should initialize");

        let commit_failed = store.with_immediate_transaction(|transaction| {
            transaction.exec_batch("insert into transaction_child(parent_id) values (1);")
        });
        assert!(commit_failed.is_err());

        store
            .with_immediate_transaction(|transaction| {
                transaction.exec_batch("insert into transaction_parent(id) values (1);")?;
                transaction.exec_batch("insert into transaction_child(parent_id) values (1);")
            })
            .expect("connection should accept a new transaction after failed commit");
    }

    #[test]
    fn immediate_transaction_rolls_back_before_resuming_a_panic() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("panic-transaction-task".to_string());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), StorageError> = store.with_immediate_transaction(|transaction| {
                transaction.append_next_event(
                    EventId("panic-event".to_string()),
                    task_id.clone(),
                    1,
                    EventKind::MessageAdded,
                    "must roll back".to_string(),
                    Metadata::new(),
                )?;
                panic!("injected transaction panic");
            });
        }));

        assert!(panic.is_err());
        assert!(store
            .list_by_task(&task_id)
            .expect("events should load")
            .is_empty());
        store
            .with_immediate_transaction(|transaction| {
                transaction.append_next_event(
                    EventId("after-panic-event".to_string()),
                    task_id.clone(),
                    2,
                    EventKind::MessageAdded,
                    "connection remains usable".to_string(),
                    Metadata::new(),
                )
            })
            .expect("connection should accept a transaction after panic rollback");
    }
}
