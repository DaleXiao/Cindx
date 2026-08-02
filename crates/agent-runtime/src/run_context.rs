use agent_core::Metadata;

pub fn run_context_steer_epoch(run_context: &Metadata) -> u64 {
    run_context
        .get("steer_epoch")
        .and_then(|epoch| epoch.parse::<u64>().ok())
        .unwrap_or_default()
}

pub fn effective_agent_objective<'a>(run_context: &'a Metadata, latest_prompt: &'a str) -> &'a str {
    run_context
        .get("effective_prompt_objective")
        .map(String::as_str)
        .filter(|objective| !objective.trim().is_empty())
        .unwrap_or(latest_prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_objective_prefers_non_empty_run_context_value() {
        let mut context = Metadata::new();
        assert_eq!(effective_agent_objective(&context, "latest"), "latest");

        context.insert("effective_prompt_objective".to_string(), "  ".to_string());
        assert_eq!(effective_agent_objective(&context, "latest"), "latest");

        context.insert(
            "effective_prompt_objective".to_string(),
            "initial plus steering".to_string(),
        );
        assert_eq!(
            effective_agent_objective(&context, "latest"),
            "initial plus steering"
        );
    }

    #[test]
    fn invalid_steer_epoch_fails_closed_to_zero() {
        let mut context = Metadata::new();
        context.insert("steer_epoch".to_string(), "invalid".to_string());
        assert_eq!(run_context_steer_epoch(&context), 0);

        context.insert("steer_epoch".to_string(), "7".to_string());
        assert_eq!(run_context_steer_epoch(&context), 7);
    }
}
