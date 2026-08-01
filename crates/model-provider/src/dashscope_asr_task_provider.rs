use super::dashscope_realtime_guard::{enforce_transcript_limit, server_error};
use super::dashscope_realtime_provider::text_message_value;
use super::ModelError;
use futures_util::{future::try_join, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::timeout;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

const AUDIO_CHUNK_BYTES: usize = 32 * 1024;
static TASK_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) async fn transcribe_inference_task<S>(
    socket: &mut WebSocketStream<S>,
    pcm: &[u8],
    model: &str,
    connect_timeout: Duration,
    api_key: &str,
) -> Result<String, ModelError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let task_id = inference_task_id();
    send_json(
        socket,
        inference_task_event("run-task", &task_id, Some(model)),
    )
    .await?;
    wait_until_task_started(socket, connect_timeout, api_key).await?;

    let (mut writer, mut reader) = socket.split();
    let send_audio = async {
        for chunk in pcm.chunks(AUDIO_CHUNK_BYTES) {
            writer
                .send(Message::Binary(chunk.to_vec().into()))
                .await
                .map_err(|error| {
                    ModelError::new(format!("DashScope ASR audio send failed: {error}"))
                })?;
        }
        writer
            .send(Message::Text(
                inference_task_event("finish-task", &task_id, None)
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|error| {
                ModelError::new(format!("DashScope ASR finish send failed: {error}"))
            })?;
        Ok::<(), ModelError>(())
    };
    let receive_results = async {
        let mut sentences = BTreeMap::new();
        let mut unkeyed = None;
        while let Some(message) = reader.next().await {
            let value = text_message_value(message, api_key)?;
            match value.pointer("/header/event").and_then(Value::as_str) {
                Some("result-generated") => {
                    update_transcript(&mut sentences, &mut unkeyed, &value)?;
                }
                Some("task-finished") => return assemble_transcript(sentences, unkeyed),
                Some("task-failed") => return Err(server_error(&value, api_key)),
                _ => {}
            }
        }
        Err(ModelError::new(
            "DashScope ASR connection closed before transcription completed",
        ))
    };
    let (_, transcript) = try_join(send_audio, receive_results).await?;
    Ok(transcript)
}

async fn wait_until_task_started<S>(
    socket: &mut WebSocketStream<S>,
    wait: Duration,
    api_key: &str,
) -> Result<(), ModelError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    timeout(wait, async {
        while let Some(message) = socket.next().await {
            let value = text_message_value(message, api_key)?;
            match value.pointer("/header/event").and_then(Value::as_str) {
                Some("task-started") => return Ok(()),
                Some("task-failed") => return Err(server_error(&value, api_key)),
                _ => {}
            }
        }
        Err(ModelError::new(
            "DashScope ASR connection closed before the task started",
        ))
    })
    .await
    .map_err(|_| ModelError::new("DashScope ASR task setup timed out"))?
}

async fn send_json<S>(socket: &mut WebSocketStream<S>, value: Value) -> Result<(), ModelError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|error| ModelError::new(format!("DashScope ASR command send failed: {error}")))
}

fn inference_task_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let sequence = TASK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{timestamp:016x}{sequence:016x}")
}

fn inference_task_event(action: &str, task_id: &str, model: Option<&str>) -> Value {
    let mut value = json!({
        "header": { "action": action, "task_id": task_id, "streaming": "duplex" },
        "payload": { "input": {} }
    });
    if let Some(model) = model {
        value["payload"] = json!({
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "model": model,
            "parameters": { "sample_rate": 16000, "format": "pcm" },
            "input": {}
        });
    }
    value
}

fn update_transcript(
    sentences: &mut BTreeMap<i64, String>,
    unkeyed: &mut Option<String>,
    value: &Value,
) -> Result<(), ModelError> {
    let Some(sentence) = value.pointer("/payload/output/sentence") else {
        return Ok(());
    };
    let text = sentence
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if text.is_empty() {
        return Ok(());
    }
    if let Some(begin_time) = sentence.get("begin_time").and_then(Value::as_i64) {
        sentences.insert(begin_time, text.to_string());
    } else {
        *unkeyed = Some(text.to_string());
    }
    let bytes = sentences.values().map(String::len).sum::<usize>()
        + unkeyed.as_ref().map_or(0, String::len);
    enforce_transcript_limit(bytes)
}

fn assemble_transcript(
    sentences: BTreeMap<i64, String>,
    unkeyed: Option<String>,
) -> Result<String, ModelError> {
    let mut transcript = String::new();
    for sentence in sentences.into_values() {
        push_sentence(&mut transcript, &sentence);
    }
    if let Some(unkeyed) = unkeyed.filter(|text| !transcript.ends_with(text)) {
        push_sentence(&mut transcript, &unkeyed);
    }
    enforce_transcript_limit(transcript.len())?;
    Ok(transcript)
}

fn push_sentence(transcript: &mut String, sentence: &str) {
    let sentence = sentence.trim();
    if transcript
        .chars()
        .next_back()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        && sentence
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
    {
        transcript.push(' ');
    }
    transcript.push_str(sentence);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_commands_follow_the_dashscope_asr_protocol() {
        let run = inference_task_event("run-task", "task-1", Some("fun-asr-realtime"));
        assert_eq!(run["header"]["streaming"], "duplex");
        assert_eq!(run["payload"]["task_group"], "audio");
        assert_eq!(run["payload"]["task"], "asr");
        assert_eq!(run["payload"]["function"], "recognition");
        assert_eq!(run["payload"]["model"], "fun-asr-realtime");
        assert_eq!(run["payload"]["parameters"]["sample_rate"], 16_000);
        assert_eq!(run["payload"]["parameters"]["format"], "pcm");
        assert!(
            inference_task_event("finish-task", "task-1", None)["payload"]
                .get("model")
                .is_none()
        );
    }

    #[test]
    fn partial_results_replace_the_same_sentence_and_keep_order() {
        let mut sentences = BTreeMap::new();
        let mut unkeyed = None;
        for value in [
            json!({"payload":{"output":{"sentence":{"begin_time":900,"text":"world"}}}}),
            json!({"payload":{"output":{"sentence":{"begin_time":100,"text":"hel"}}}}),
            json!({"payload":{"output":{"sentence":{"begin_time":100,"text":"hello "}}}}),
        ] {
            update_transcript(&mut sentences, &mut unkeyed, &value).unwrap();
        }
        assert_eq!(
            assemble_transcript(sentences, unkeyed).unwrap(),
            "hello world"
        );
    }
}
