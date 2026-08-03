use crate::collaboration_models::PromptAutoTeacherCase;
use agent_core::Event;
use orchestrator::{AutoTeacherSourceContextV1, PromptTransferProvenance};

#[derive(Clone, Copy)]
pub(crate) struct CanonicalPromptAutoTeacher<'a> {
    pub(crate) source_run_id: &'a str,
    pub(crate) steer_epoch: u64,
    pub(crate) profile_id: &'a str,
    pub(crate) profile_sha256: &'a str,
    pub(crate) output_sha256: &'a str,
    pub(crate) source_context: &'a AutoTeacherSourceContextV1,
}

impl<'a> From<&'a PromptAutoTeacherCase> for CanonicalPromptAutoTeacher<'a> {
    fn from(teacher: &'a PromptAutoTeacherCase) -> Self {
        Self {
            source_run_id: &teacher.source_run_id,
            steer_epoch: teacher.steer_epoch,
            profile_id: &teacher.profile_id,
            profile_sha256: &teacher.profile_sha256,
            output_sha256: &teacher.output_sha256,
            source_context: &teacher.source_context,
        }
    }
}

pub(crate) fn prompt_auto_teacher_source_key(event: &Event) -> Option<(String, String)> {
    (event.summary == "Conductor Auto transfer evaluation").then_some((
        event.metadata.get("project_id")?.clone(),
        event.metadata.get("auto_teacher_run_id")?.clone(),
    ))
}

pub(crate) fn prompt_transfer_matches_canonical_teacher(
    transfer: &PromptTransferProvenance,
    teacher: CanonicalPromptAutoTeacher<'_>,
) -> bool {
    teacher.source_context.validate().is_ok()
        && teacher
            .source_context
            .digest()
            .is_ok_and(|digest| digest == transfer.source_context_sha256)
        && transfer.source_run_id == teacher.source_run_id
        && transfer.source_steer_epoch == Some(teacher.steer_epoch)
        && transfer.source_profile_id == teacher.profile_id
        && transfer.source_profile_sha256 == teacher.profile_sha256
        && transfer.source_output_sha256 == teacher.output_sha256
        && transfer.evaluator_receipt_sha256 == teacher.source_context.evaluator_receipt_sha256
        && transfer.checkpoint_sha256 == teacher.source_context.checkpoint_sha256
        && transfer.learning_receipt_sha256 == teacher.source_context.learning_receipt_sha256
}
