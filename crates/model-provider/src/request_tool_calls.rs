use agent_core::Message;

pub(super) fn assistant_tool_calls_json(message: &Message) -> Option<String> {
    let raw = message
        .metadata
        .get("raw_tool_calls_json")
        .map(String::as_str)?
        .trim();
    let parsed = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let calls = parsed.as_array()?;
    (!calls.is_empty() && calls.iter().all(valid_tool_call_value))
        .then(|| serde_json::to_string(&parsed).ok())
        .flatten()
}

fn valid_tool_call_value(value: &serde_json::Value) -> bool {
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
        && value
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| !name.trim().is_empty())
        && value
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|arguments| serde_json::from_str::<serde_json::Value>(arguments).is_ok())
}

pub(super) fn tool_call_ids_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|call| {
            call.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}
