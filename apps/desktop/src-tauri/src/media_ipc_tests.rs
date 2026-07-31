use crate::*;

fn upload_metadata(
    name: &str,
    batch_file_sizes: Vec<u64>,
    batch_index: usize,
) -> RawAttachmentUploadMetadata {
    let batch_total_bytes = batch_file_sizes.iter().sum();
    RawAttachmentUploadMetadata {
        session_id: "session-media".to_string(),
        batch_id: "batch-media".to_string(),
        name: name.to_string(),
        mime_type: "image/png".to_string(),
        batch_file_count: batch_file_sizes.len(),
        batch_file_sizes,
        batch_index,
        batch_total_bytes,
    }
}

#[test]
fn raw_attachment_metadata_round_trips_utf8_without_payload_base64() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(&serde_json::json!({
            "sessionId": "session-media",
            "batchId": "batch-media",
            "name": "截图.png",
            "mimeType": "image/png",
            "batchFileCount": 2,
            "batchFileSizes": [512, 512],
            "batchIndex": 1,
            "batchTotalBytes": 1024
        }))
        .expect("metadata should encode"),
    );

    let decoded =
        decode_raw_attachment_upload_metadata(&encoded).expect("metadata header should decode");

    assert_eq!(decoded.session_id, "session-media");
    assert_eq!(decoded.batch_id, "batch-media");
    assert_eq!(decoded.name, "截图.png");
    assert_eq!(decoded.mime_type, "image/png");
    assert_eq!(decoded.batch_file_count, 2);
    assert_eq!(decoded.batch_file_sizes, vec![512, 512]);
    assert_eq!(decoded.batch_index, 1);
    assert_eq!(decoded.batch_total_bytes, 1024);
}

#[test]
fn raw_attachment_batches_enforce_one_server_owned_manifest_and_unique_actual_bytes() {
    let mebibyte = 1024 * 1024_u64;
    let sizes = vec![20 * mebibyte, 20 * mebibyte, 10 * mebibyte];
    let mut batches = AttachmentUploadBatches::default();

    let first = upload_metadata("first.bin", sizes.clone(), 0);
    let first_reservation = batches
        .reserve(&first, sizes[0])
        .expect("first actual file should reserve");
    assert!(batches
        .reserve(&first, sizes[0])
        .unwrap_err()
        .contains("more than once"));
    batches
        .complete(first_reservation, PathBuf::from("first.bin"))
        .expect("first actual file should complete");
    assert!(batches
        .reserve(&first, sizes[0])
        .unwrap_err()
        .contains("more than once"));

    let changed = upload_metadata(
        "changed.bin",
        vec![20 * mebibyte, 15 * mebibyte, 15 * mebibyte],
        1,
    );
    assert!(batches
        .reserve(&changed, 15 * mebibyte)
        .unwrap_err()
        .contains("manifest changed"));

    for index in 1..sizes.len() {
        let metadata = upload_metadata("next.bin", sizes.clone(), index);
        let reservation = batches
            .reserve(&metadata, sizes[index])
            .expect("matching actual file should reserve");
        batches
            .complete(reservation, PathBuf::from(format!("file-{index}.bin")))
            .expect("matching actual file should complete");
    }
    assert_eq!(batches.active_count(), 0);

    let incomplete = upload_metadata("abort.bin", vec![1, 1], 0);
    let reservation = batches
        .reserve(&incomplete, 1)
        .expect("incomplete file should reserve");
    batches
        .complete(reservation, PathBuf::from("abort.bin"))
        .expect("incomplete file should stage");
    assert_eq!(
        batches.abort("session-media", "batch-media"),
        vec![PathBuf::from("abort.bin")]
    );
    assert_eq!(batches.active_count(), 0);
}

#[test]
fn abort_cleanup_removes_every_completed_staged_file() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let first = directory.path().join("first.bin");
    let second = directory.path().join("second.bin");
    fs::write(&first, b"first").expect("first staged file should write");
    fs::write(&second, b"second").expect("second staged file should write");

    cleanup_staged_attachment_paths(vec![first.clone(), second.clone()]);

    assert!(!first.exists());
    assert!(!second.exists());
}

#[test]
fn raw_attachment_limits_preserve_file_count_size_and_total_contracts() {
    let mut valid_sizes = vec![0; 10];
    valid_sizes[9] = 50;
    assert!(validate_raw_attachment_upload(&upload_metadata("ok.png", valid_sizes, 9), 50).is_ok());
    assert!(
        validate_raw_attachment_upload(&upload_metadata("too-many.png", vec![1; 11], 0), 1)
            .unwrap_err()
            .contains("at most 10")
    );
    assert!(validate_raw_attachment_upload(
        &upload_metadata("too-large.png", vec![MAX_ATTACHMENT_BYTES as u64 + 1], 0),
        MAX_ATTACHMENT_BYTES + 1,
    )
    .unwrap_err()
    .contains("20 MB"));
    assert!(validate_raw_attachment_upload(
        &upload_metadata(
            "too-much.png",
            vec![
                MAX_ATTACHMENT_BYTES as u64,
                MAX_ATTACHMENT_BYTES as u64,
                10 * 1024 * 1024 + 1,
            ],
            0,
        ),
        MAX_ATTACHMENT_BYTES,
    )
    .unwrap_err()
    .contains("50 MB"));
    assert!(
        validate_raw_attachment_upload(&upload_metadata("bad-index.png", vec![1], 1), 1)
            .unwrap_err()
            .contains("index")
    );
    assert!(validate_raw_attachment_upload(
        &RawAttachmentUploadMetadata {
            batch_total_bytes: 2,
            ..upload_metadata("bad-manifest.png", vec![1], 0)
        },
        1,
    )
    .unwrap_err()
    .contains("manifest"));
    assert!(
        validate_raw_attachment_upload(&upload_metadata("wrong-size.png", vec![2], 0), 1,)
            .unwrap_err()
            .contains("manifest")
    );
}

#[test]
fn raw_attachment_staging_writes_exact_bytes_and_metadata() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let payload = vec![0, 1, 2, 3, 0xff];
    let staged = stage_raw_agent_attachment(
        directory.path(),
        upload_metadata("../截图.png", vec![payload.len() as u64], 0),
        payload.clone(),
    )
    .expect("attachment should stage");

    assert_eq!(staged.name, "截图.png");
    assert_eq!(staged.mime_type, "image/png");
    assert_eq!(staged.size_bytes, payload.len() as u64);
    assert_eq!(
        fs::read(&staged.path).expect("staged file should read"),
        payload
    );
    assert!(Path::new(&staged.path).starts_with(directory.path().join(".cindx/attachments")));
}

#[test]
fn artifact_image_preview_uses_raw_bounded_bytes_without_data_url() {
    let directory = tempfile::tempdir().expect("temporary workspace should open");
    let image = directory.path().join("preview.png");
    let payload = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    fs::write(&image, &payload).expect("image should write");

    let preview = read_artifact_preview_path(&image).expect("preview metadata should load");
    assert_eq!(preview.kind, "image");
    assert_eq!(preview.mime_type, "image/png");
    assert_eq!(preview.size_bytes, payload.len() as u64);
    assert!(preview.data_url.is_none());
    assert_eq!(
        read_artifact_image_bytes(&image).expect("raw image should load"),
        payload
    );

    let oversized = directory.path().join("oversized.png");
    let file = fs::File::create(&oversized).expect("sparse image should create");
    file.set_len(MAX_ARTIFACT_IMAGE_BYTES + 1)
        .expect("sparse image should resize");
    assert!(read_artifact_image_bytes(&oversized)
        .unwrap_err()
        .contains("24 MB"));
}
