use agent_core::{
    Event, EventId, EventKind, Metadata, PermissionDecision, PermissionRequest, PermissionRequestId,
    PermissionResolution, PermissionRisk, TaskId,
};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::path::Path;
use std::ptr;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;

#[allow(non_camel_case_types)]
enum sqlite3 {}

#[allow(non_camel_case_types)]
enum sqlite3_stmt {}

type SqliteDestructor = Option<unsafe extern "C" fn(*mut c_void)>;

#[link(name = "sqlite3")]
unsafe extern "C" {
    fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut sqlite3) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
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
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut sqlite3_stmt,
        index: c_int,
        value: *const c_char,
        n: c_int,
        destructor: SqliteDestructor,
    ) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut sqlite3_stmt, index: c_int, value: i64) -> c_int;
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
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        Self::open(":memory:")
    }

    pub fn next_sequence(&self, task_id: &TaskId) -> Result<u64, StorageError> {
        let mut statement = self.prepare(
            "select coalesce(max(sequence), 0) + 1 from events where task_id = ?1",
        )?;
        statement.bind_text(1, &task_id.0)?;

        if statement.step()? == StepResult::Row {
            Ok(statement.column_i64(0) as u64)
        } else {
            Ok(1)
        }
    }

    pub fn delete_records_by_metadata(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<(), StorageError> {
        let row = format!("{}\t{}", hex_encode(key.as_bytes()), hex_encode(value.as_bytes()));
        self.exec_batch("begin immediate transaction")?;

        let result = (|| {
            let predicate =
                "instr(char(10) || metadata_text || char(10), char(10) || ?1 || char(10)) > 0";
            let mut delete_resolutions = self.prepare(&format!(
                "delete from permission_resolutions where request_id in (select id from permission_requests where {predicate})"
            ))?;
            delete_resolutions.bind_text(1, &row)?;
            delete_resolutions.expect_done()?;

            let mut delete_requests = self.prepare(&format!(
                "delete from permission_requests where {predicate}"
            ))?;
            delete_requests.bind_text(1, &row)?;
            delete_requests.expect_done()?;

            let mut delete_events =
                self.prepare(&format!("delete from events where {predicate}"))?;
            delete_events.bind_text(1, &row)?;
            delete_events.expect_done()?;
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

    pub fn update_event_content(&mut self, event: &Event) -> Result<(), StorageError> {
        let mut statement = self.prepare(
            "update events set summary = ?1, metadata_text = ?2 where id = ?3",
        )?;
        statement.bind_text(1, &event.summary)?;
        statement.bind_text(2, &metadata_to_text(&event.metadata))?;
        statement.bind_text(3, &event.id.0)?;
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
              metadata_text text not null
            );

            create index if not exists idx_events_task_sequence
              on events(task_id, sequence);

            create table if not exists permission_requests (
              id text primary key not null,
              task_id text not null,
              risk text not null,
              action text not null,
              reason text not null,
              scope text not null,
              metadata_text text not null,
              requested_at_ms integer not null,
              status text not null
            );

            create table if not exists permission_resolutions (
              request_id text primary key not null,
              decision text not null,
              resolved_at_ms integer not null,
              resolved_by text not null,
              foreign key(request_id) references permission_requests(id)
            );
            ",
        )
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
            insert into events(id, task_id, sequence, timestamp_ms, kind, summary, metadata_text)
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ",
        )?;

        statement.bind_text(1, &event.id.0)?;
        statement.bind_text(2, &event.task_id.0)?;
        statement.bind_i64(3, event.sequence as i64)?;
        statement.bind_i64(4, event.timestamp_ms as i64)?;
        statement.bind_text(5, event_kind_to_str(&event.kind))?;
        statement.bind_text(6, &event.summary)?;
        statement.bind_text(7, &metadata_to_text(&event.metadata))?;
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
}

impl PermissionStore for SqliteStore {
    fn save_permission_request(
        &mut self,
        request: PermissionRequest,
        requested_at_ms: u64,
    ) -> Result<(), StorageError> {
        let mut statement = self.prepare(
            "
            insert or replace into permission_requests(
              id, task_id, risk, action, reason, scope, metadata_text, requested_at_ms, status
            )
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending')
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
        statement.expect_done()
    }

    fn resolve_permission(&mut self, resolution: PermissionResolution) -> Result<(), StorageError> {
        self.exec_batch("begin immediate transaction")?;

        let result = (|| {
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
            update.expect_done()?;

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
    metadata
        .iter()
        .map(|(key, value)| format!("{}\t{}", hex_encode(key.as_bytes()), hex_encode(value.as_bytes())))
        .collect::<Vec<_>>()
        .join("\n")
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
            String::from_utf8(hex_decode(key)?).map_err(|error| StorageError::new(error.to_string()))?,
            String::from_utf8(hex_decode(value)?)
                .map_err(|error| StorageError::new(error.to_string()))?,
        );
    }

    Ok(metadata)
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

fn hex_decode(value: &str) -> Result<Vec<u8>, StorageError> {
    if value.len() % 2 != 0 {
        return Err(StorageError::new("hex value has odd length"));
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars: Vec<u8> = value.as_bytes().to_vec();
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
        other => Err(StorageError::new(format!("unknown permission risk: {other}"))),
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
        other => Err(StorageError::new(format!("unknown permission decision: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{PermissionRisk, TaskId};

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
        assert_eq!(events[0].metadata.get("tool"), Some(&"shell.run".to_string()));
    }

    #[test]
    fn lists_and_updates_persisted_event_content() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let mut metadata = Metadata::new();
        metadata.insert("output".to_string(), "api_key=secret".to_string());
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

        let audits = store
            .list_permission_audits()
            .expect("audits should list");

        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].request.id, request_id);
        assert_eq!(
            audits[0].resolution.as_ref().map(|resolution| &resolution.decision),
            Some(&PermissionDecision::AllowOnce)
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
        store
            .save_permission_request(
                PermissionRequest {
                    id: PermissionRequestId("perm-a".to_string()),
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
            .delete_records_by_metadata("session_id", "session-a")
            .expect("session records should delete");

        let events = store.list_by_task(&task_id).expect("events should load");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].metadata.get("session_id").map(String::as_str),
            Some("session-b")
        );
        assert!(store
            .list_permission_audits()
            .expect("audits should load")
            .is_empty());
    }
}
