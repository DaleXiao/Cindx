use crate::view_models::{ModelStreamDelta, RagOperationProgress};
use tauri::Emitter;

const MODEL_STREAM_DELTA_EVENT: &str = "model-stream-delta";
const RAG_OPERATION_PROGRESS_EVENT: &str = "rag-operation-progress";
const SESSION_TITLE_UPDATED_EVENT: &str = "session-title-updated";

pub(crate) trait DesktopEventSink: Send + Sync {
    fn emit_model_stream_delta(&self, payload: ModelStreamDelta);

    fn emit_rag_operation_progress(&self, payload: RagOperationProgress);

    fn emit_session_title_updated(&self, session_id: String);
}

impl DesktopEventSink for tauri::AppHandle {
    fn emit_model_stream_delta(&self, payload: ModelStreamDelta) {
        let _ = self.emit(MODEL_STREAM_DELTA_EVENT, payload);
    }

    fn emit_rag_operation_progress(&self, payload: RagOperationProgress) {
        let _ = self.emit(RAG_OPERATION_PROGRESS_EVENT, payload);
    }

    fn emit_session_title_updated(&self, session_id: String) {
        let _ = self.emit(SESSION_TITLE_UPDATED_EVENT, session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_query_commands::emit_agent_stream_delta;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedModelStreamDelta {
        task_id: String,
        request_id: String,
        session_id: Option<String>,
        delta: String,
        done: bool,
        reset: bool,
        error: Option<String>,
    }

    impl From<ModelStreamDelta> for RecordedModelStreamDelta {
        fn from(payload: ModelStreamDelta) -> Self {
            Self {
                task_id: payload.task_id,
                request_id: payload.request_id,
                session_id: payload.session_id,
                delta: payload.delta,
                done: payload.done,
                reset: payload.reset,
                error: payload.error,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum RecordedDesktopEvent {
        ModelStreamDelta(RecordedModelStreamDelta),
        RagOperationProgress(RagOperationProgress),
        SessionTitleUpdated(String),
    }

    #[derive(Default)]
    struct RecordingDesktopEventSink {
        events: Mutex<Vec<RecordedDesktopEvent>>,
    }

    impl DesktopEventSink for RecordingDesktopEventSink {
        fn emit_model_stream_delta(&self, payload: ModelStreamDelta) {
            self.events
                .lock()
                .expect("recording event sink lock should remain available")
                .push(RecordedDesktopEvent::ModelStreamDelta(payload.into()));
        }

        fn emit_rag_operation_progress(&self, payload: RagOperationProgress) {
            self.events
                .lock()
                .expect("recording event sink lock should remain available")
                .push(RecordedDesktopEvent::RagOperationProgress(payload));
        }

        fn emit_session_title_updated(&self, session_id: String) {
            self.events
                .lock()
                .expect("recording event sink lock should remain available")
                .push(RecordedDesktopEvent::SessionTitleUpdated(session_id));
        }
    }

    #[test]
    fn recording_sink_preserves_stream_payloads_and_cross_event_order() {
        let sink = RecordingDesktopEventSink::default();

        emit_agent_stream_delta(
            &sink,
            "request-1",
            Some("session-1"),
            "first delta",
            false,
            false,
            None,
        );
        emit_agent_stream_delta(&sink, "request-1", Some("session-1"), "", false, true, None);
        sink.emit_session_title_updated("session-1".to_string());
        emit_agent_stream_delta(
            &sink,
            "request-1",
            Some("session-1"),
            "",
            true,
            false,
            Some("provider unavailable".to_string()),
        );

        assert_eq!(
            *sink
                .events
                .lock()
                .expect("recorded event list should remain available"),
            vec![
                RecordedDesktopEvent::ModelStreamDelta(RecordedModelStreamDelta {
                    task_id: "phase-16-agent-loop".to_string(),
                    request_id: "request-1".to_string(),
                    session_id: Some("session-1".to_string()),
                    delta: "first delta".to_string(),
                    done: false,
                    reset: false,
                    error: None,
                }),
                RecordedDesktopEvent::ModelStreamDelta(RecordedModelStreamDelta {
                    task_id: "phase-16-agent-loop".to_string(),
                    request_id: "request-1".to_string(),
                    session_id: Some("session-1".to_string()),
                    delta: String::new(),
                    done: false,
                    reset: true,
                    error: None,
                }),
                RecordedDesktopEvent::SessionTitleUpdated("session-1".to_string()),
                RecordedDesktopEvent::ModelStreamDelta(RecordedModelStreamDelta {
                    task_id: "phase-16-agent-loop".to_string(),
                    request_id: "request-1".to_string(),
                    session_id: Some("session-1".to_string()),
                    delta: String::new(),
                    done: true,
                    reset: false,
                    error: Some("provider unavailable".to_string()),
                }),
            ]
        );
    }
}
