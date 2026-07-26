use super::*;

pub(crate) fn confirm_application_exit(app_handle: &tauri::AppHandle) -> bool {
    let state = app_handle.state::<AppState>();
    if state.allow_exit.load(Ordering::SeqCst) {
        return true;
    }
    if quit_confirmation_suppressed(app_handle) {
        state.allow_exit.store(true, Ordering::SeqCst);
        return true;
    }
    if state.quit_prompt_active.swap(true, Ordering::SeqCst) {
        return false;
    }

    let confirmation = show_native_quit_confirmation();
    state.quit_prompt_active.store(false, Ordering::SeqCst);
    if !confirmation.confirmed {
        return false;
    }
    if confirmation.suppress_future {
        if let Err(error) = persist_quit_confirmation_suppression(app_handle) {
            eprintln!("failed to save quit confirmation preference: {error}");
        }
    }
    state.allow_exit.store(true, Ordering::SeqCst);
    true
}

pub(crate) fn quit_confirmation_preference_path(
    app_handle: &tauri::AppHandle,
) -> Result<PathBuf, String> {
    app_handle
        .path()
        .app_config_dir()
        .map(|path| path.join("preferences.conf"))
        .map_err(|error| format!("failed to resolve app config directory: {error}"))
}

pub(crate) fn quit_confirmation_suppressed(app_handle: &tauri::AppHandle) -> bool {
    let Ok(path) = quit_confirmation_preference_path(app_handle) else {
        return false;
    };
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    quit_confirmation_suppressed_text(&text)
}

pub(crate) fn quit_confirmation_suppressed_text(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim() == "skip_quit_confirmation=true")
}

pub(crate) fn persist_quit_confirmation_suppression(
    app_handle: &tauri::AppHandle,
) -> Result<(), String> {
    let path = quit_confirmation_preference_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create app config directory: {error}"))?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|error| format!("failed to open quit preference: {error}"))?;
    file.write_all(b"skip_quit_confirmation=true\n")
        .map_err(|error| format!("failed to write quit preference: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure quit preference: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn show_native_quit_confirmation() -> QuitConfirmation {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSControlStateValueOn};
    use objc2_foundation::NSString;

    let Some(main_thread) = MainThreadMarker::new() else {
        return QuitConfirmation {
            confirmed: false,
            suppress_future: false,
        };
    };
    let alert = NSAlert::new(main_thread);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setMessageText(&NSString::from_str("Quit Cindx?"));
    alert.setInformativeText(&NSString::from_str(
        "Any running agent work will stop when the app quits.",
    ));
    alert.addButtonWithTitle(&NSString::from_str("Quit Cindx"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.setShowsSuppressionButton(true);
    if let Some(button) = alert.suppressionButton() {
        button.setTitle(&NSString::from_str("Don't ask again"));
    }

    let response = alert.runModal();
    let suppress_future = alert
        .suppressionButton()
        .is_some_and(|button| button.state() == NSControlStateValueOn);
    QuitConfirmation {
        confirmed: response == NSAlertFirstButtonReturn,
        suppress_future,
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn show_native_quit_confirmation() -> QuitConfirmation {
    QuitConfirmation {
        confirmed: true,
        suppress_future: false,
    }
}

#[tauri::command]
pub(crate) async fn confirm_delete_action(
    app: tauri::AppHandle,
    input: ConfirmDeleteInput,
) -> Result<bool, String> {
    let kind = match input.kind.trim() {
        "project" => "project".to_string(),
        "session" => "session".to_string(),
        _ => return Err("unsupported delete target".to_string()),
    };
    let name = normalized_config_value(&input.name);
    if name.is_empty() {
        return Err("delete target name is empty".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        app.run_on_main_thread(move || {
            let _ = sender.send(show_native_delete_confirmation(&kind, &name));
        })
        .map_err(|error| format!("failed to show delete confirmation: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("delete confirmation closed unexpectedly: {error}"))
    })
    .await
    .map_err(|error| format!("delete confirmation task failed: {error}"))?
}

#[cfg(target_os = "macos")]
pub(crate) fn show_native_delete_confirmation(kind: &str, name: &str) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication};
    use objc2_foundation::NSString;

    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let target = if kind == "project" {
        "Project"
    } else {
        "Session"
    };
    let informative_text = if kind == "project" {
        "This permanently removes the project and its sessions from Cindx. Files in the workspace are not affected."
    } else {
        "This permanently removes the conversation and its agent history. This action cannot be undone."
    };
    let alert = NSAlert::new(main_thread);
    alert.setAlertStyle(NSAlertStyle::Critical);
    if let Some(icon) = NSApplication::sharedApplication(main_thread).applicationIconImage() {
        // NSAlert otherwise substitutes its critical-warning artwork for the app identity.
        unsafe { alert.setIcon(Some(&icon)) };
    }
    alert.setMessageText(&NSString::from_str(&format!("Delete {target} ‘{name}’?")));
    alert.setInformativeText(&NSString::from_str(informative_text));
    alert.addButtonWithTitle(&NSString::from_str(&format!("Delete {target}")));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.runModal() == NSAlertFirstButtonReturn
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn show_native_delete_confirmation(_kind: &str, _name: &str) -> bool {
    true
}

#[tauri::command]
pub(crate) fn read_artifact_image(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let canonical_path = validated_workspace_artifact_path(&state, &path)?;
    let metadata = fs::metadata(&canonical_path)
        .map_err(|error| format!("failed to inspect artifact image: {error}"))?;
    if metadata.len() > 24 * 1024 * 1024 {
        return Err("artifact image exceeds the 24 MB preview limit".to_string());
    }

    let extension = canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return Err("artifact is not a supported preview image".to_string()),
    };
    let bytes = fs::read(&canonical_path)
        .map_err(|error| format!("failed to read artifact image: {error}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

#[tauri::command]
pub(crate) fn read_artifact_preview(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<ArtifactPreviewView, String> {
    let canonical_path = validated_workspace_artifact_path(&state, &path)?;
    let metadata = fs::metadata(&canonical_path)
        .map_err(|error| format!("failed to inspect artifact: {error}"))?;
    if metadata.is_dir() {
        return Ok(ArtifactPreviewView {
            kind: "directory".to_string(),
            mime_type: "inode/directory".to_string(),
            content: None,
            data_url: None,
            size_bytes: 0,
        });
    }
    let extension = canonical_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let image_mime = match extension.as_str() {
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "gif" => Some("image/gif"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        _ => None,
    };
    if let Some(mime_type) = image_mime {
        if metadata.len() > 24 * 1024 * 1024 {
            return Err("artifact image exceeds the 24 MB preview limit".to_string());
        }
        let bytes = fs::read(&canonical_path)
            .map_err(|error| format!("failed to read artifact image: {error}"))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        return Ok(ArtifactPreviewView {
            kind: "image".to_string(),
            mime_type: mime_type.to_string(),
            content: None,
            data_url: Some(format!("data:{mime_type};base64,{encoded}")),
            size_bytes: metadata.len(),
        });
    }

    let (kind, mime_type) = match extension.as_str() {
        "html" | "htm" => ("html", "text/html"),
        "md" | "markdown" => ("markdown", "text/markdown"),
        "css" => ("text", "text/css"),
        "csv" => ("text", "text/csv"),
        "json" => ("text", "application/json"),
        "js" | "jsx" | "mjs" | "ts" | "tsx" => ("text", "text/javascript"),
        "log" | "rs" | "swift" | "toml" | "txt" | "xml" | "yaml" | "yml" => ("text", "text/plain"),
        _ => ("file", "application/octet-stream"),
    };
    if kind == "file" {
        return Ok(ArtifactPreviewView {
            kind: kind.to_string(),
            mime_type: mime_type.to_string(),
            content: None,
            data_url: None,
            size_bytes: metadata.len(),
        });
    }
    if metadata.len() > 2 * 1024 * 1024 {
        return Err("text artifact exceeds the 2 MB preview limit".to_string());
    }
    let content = fs::read_to_string(&canonical_path)
        .map_err(|error| format!("failed to read text artifact: {error}"))?;
    Ok(ArtifactPreviewView {
        kind: kind.to_string(),
        mime_type: mime_type.to_string(),
        content: Some(content),
        data_url: None,
        size_bytes: metadata.len(),
    })
}

#[tauri::command]
pub(crate) fn open_artifact(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    let canonical_path = validated_workspace_artifact_path(&state, &path)?;
    open_with_default_app(canonical_path.as_os_str())
}

#[tauri::command]
pub(crate) fn reveal_artifact(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let canonical_path = validated_workspace_artifact_path(&state, &path)?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(&canonical_path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(format!("/select,{}", canonical_path.display()));
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(canonical_path.parent().unwrap_or(&canonical_path));
        command
    };

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to reveal artifact: {error}"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = canonical_path;
        Err("revealing artifacts is not supported on this platform".to_string())
    }
}

#[tauri::command]
pub(crate) fn open_external_url(url: String) -> Result<(), String> {
    let url = url.trim();
    let normalized = url.to_ascii_lowercase();
    if url.len() > 8192
        || !(normalized.starts_with("https://")
            || normalized.starts_with("http://")
            || normalized.starts_with("mailto:"))
    {
        return Err("only http, https, and mailto links can be opened".to_string());
    }
    open_with_default_app(OsStr::new(url))
}

pub(crate) fn open_with_default_app(target: &OsStr) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(target);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(target);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(target);
        command
    };

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open with the default app: {error}"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("opening artifacts is not supported on this platform".to_string())
    }
}

pub(crate) fn validated_workspace_artifact_path(
    state: &tauri::State<'_, AppState>,
    value: &str,
) -> Result<PathBuf, String> {
    let workspace_root = active_workspace_root(state)?;
    let requested = PathBuf::from(value);
    let requested = if requested.is_absolute() {
        requested
    } else {
        workspace_root.join(requested)
    };
    let canonical_root = fs::canonicalize(&workspace_root)
        .map_err(|error| format!("failed to resolve workspace root: {error}"))?;
    let canonical_path = fs::canonicalize(&requested)
        .map_err(|error| format!("failed to resolve artifact: {error}"))?;
    if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
        return Err("artifact must be a file inside the active workspace".to_string());
    }
    Ok(canonical_path)
}
