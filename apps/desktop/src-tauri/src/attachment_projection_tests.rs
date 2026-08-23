use super::*;

#[test]
fn attachment_paths_stay_inside_project_managed_storage() {
    let root = std::env::temp_dir().join(format!(
        "cindx-attachment-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let attachment_dir = root.join(".cindx/attachments/session-a");
    fs::create_dir_all(&attachment_dir).expect("attachment directory should exist");
    let attachment = attachment_dir.join("image.png");
    fs::write(&attachment, b"png").expect("attachment should write");
    let outside = root.join("outside.png");
    fs::write(&outside, b"png").expect("outside fixture should write");

    assert_eq!(
        validated_attachment_path(&root, &attachment.display().to_string())
            .expect("managed attachment should validate"),
        fs::canonicalize(&attachment).expect("attachment should resolve")
    );
    assert!(validated_attachment_path(&root, &outside.display().to_string()).is_err());
    assert_eq!(safe_attachment_name("../nested/screen.png"), "screen.png");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn user_message_projection_preserves_attachment_metadata() {
    let attachment = AgentAttachmentView {
        id: "attachment-1".to_string(),
        name: "screen.png".to_string(),
        path: "/tmp/cindx/screen.png".to_string(),
        mime_type: "image/png".to_string(),
        size_bytes: 1_024,
    };
    let mut metadata = [
        ("role".to_string(), "user".to_string()),
        ("content".to_string(), "Review this screenshot".to_string()),
        ("queue_id".to_string(), "steer-queue-1".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    add_attachment_metadata(&mut metadata, std::slice::from_ref(&attachment));
    let event = Event {
        id: EventId("message-with-attachment".to_string()),
        task_id: phase16_task_id(),
        sequence: 9,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata,
    };

    let message = message_view_from_event(&event).expect("message should project");
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].id, attachment.id);
    assert_eq!(message.attachments[0].name, attachment.name);
    assert_eq!(message.attachments[0].path, attachment.path);
    assert_eq!(message.attachments[0].mime_type, attachment.mime_type);
    assert_eq!(message.attachments[0].size_bytes, attachment.size_bytes);
    assert_eq!(message.queue_id.as_deref(), Some("steer-queue-1"));
}

#[test]
fn attachment_message_separates_display_and_model_content() {
    let event = Event {
        id: EventId("message-with-model-content".to_string()),
        task_id: phase16_task_id(),
        sequence: 10,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), "Review this file".to_string()),
            (
                "model_content".to_string(),
                "Review this file\n\nAttached files: /workspace/report.pdf".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let display = message_from_event(&event).expect("display transcript should project");
    let model = model_message_from_event(&event).expect("model transcript should project");
    let runtime = runtime_message_from_event(&event).expect("runtime transcript should project");

    assert_eq!(display.content, "Review this file");
    assert_eq!(model.content, runtime.content);
    assert!(model.content.contains("/workspace/report.pdf"));
}
