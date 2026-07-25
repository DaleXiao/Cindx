use super::*;

#[tauri::command]
pub(crate) fn get_runtime_status(
    state: tauri::State<'_, AppState>,
) -> Result<RuntimeStatus, String> {
    runtime_status(&state)
}

#[cfg(target_os = "macos")]
pub(crate) fn resize_macos_sidebar_material(ns_window: usize, width: f64) -> Result<(), String> {
    unsafe {
        let window = &*(ns_window as *mut NSWindow);
        let content_view = window
            .contentView()
            .ok_or_else(|| "native window content view is unavailable".to_string())?;
        let material_view = content_view
            .viewWithTag(MACOS_SIDEBAR_MATERIAL_TAG)
            .ok_or_else(|| "native sidebar material view is unavailable".to_string())?;
        let superview = material_view
            .superview()
            .ok_or_else(|| "native sidebar material parent is unavailable".to_string())?;
        let bounds = NSView::bounds(&superview);
        let mut frame = bounds;
        frame.size.width = width.clamp(0.0, 320.0).min(bounds.size.width);
        material_view.setFrame(frame);
        material_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_sidebar_material(window: &tauri::WebviewWindow) -> Result<(), String> {
    window_vibrancy::apply_vibrancy(
        window,
        window_vibrancy::NSVisualEffectMaterial::Sidebar,
        Some(window_vibrancy::NSVisualEffectState::Active),
        None,
    )
    .map_err(|error| format!("failed to install native sidebar material: {error}"))?;
    let ns_window = window
        .ns_window()
        .map_err(|error| format!("failed to access native window: {error}"))?
        as usize;
    if let Err(error) = resize_macos_sidebar_material(ns_window, MACOS_SIDEBAR_DEFAULT_WIDTH) {
        let _ = window_vibrancy::clear_vibrancy(window);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn install_macos_sidebar_material(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn update_macos_sidebar_material_width(
    window: &tauri::WebviewWindow,
    width: f64,
) -> Result<(), String> {
    let ns_window = window
        .ns_window()
        .map_err(|error| format!("failed to access native window: {error}"))?
        as usize;
    window
        .run_on_main_thread(move || {
            if let Err(error) = resize_macos_sidebar_material(ns_window, width) {
                append_startup_log(&error);
            }
        })
        .map_err(|error| format!("failed to resize native sidebar material: {error}"))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn update_macos_sidebar_material_width(
    _window: &tauri::WebviewWindow,
    _width: f64,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub(crate) fn set_sidebar_material_width(app: tauri::AppHandle, width: f64) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    update_macos_sidebar_material_width(&window, width)
}

#[cfg(target_os = "macos")]
pub(crate) fn repair_macos_traffic_light_position(
    window: &tauri::WebviewWindow,
) -> Result<(), String> {
    // Repeat tao's inset after the initially hidden window has its final frame.
    let ns_window = window
        .ns_window()
        .map_err(|error| format!("failed to access native window: {error}"))?
        as usize;

    window
        .run_on_main_thread(move || unsafe {
            let window = &*(ns_window as *mut NSWindow);
            let Some(close) = window.standardWindowButton(NSWindowButton::CloseButton) else {
                return;
            };
            let Some(miniaturize) = window.standardWindowButton(NSWindowButton::MiniaturizeButton)
            else {
                return;
            };
            let Some(zoom) = window.standardWindowButton(NSWindowButton::ZoomButton) else {
                return;
            };
            let Some(title_bar_view) = close.superview().and_then(|view| view.superview()) else {
                return;
            };

            let close_frame = NSView::frame(&close);
            let title_bar_height = close_frame.size.height + MACOS_TRAFFIC_LIGHT_Y;
            let mut title_bar_frame = NSView::frame(&title_bar_view);
            title_bar_frame.size.height = title_bar_height;
            title_bar_frame.origin.y = window.frame().size.height - title_bar_height;
            title_bar_view.setFrame(title_bar_frame);

            let spacing = NSView::frame(&miniaturize).origin.x - close_frame.origin.x;
            for (index, button) in [close, miniaturize, zoom].into_iter().enumerate() {
                let mut origin = NSView::frame(&button).origin;
                origin.x = MACOS_TRAFFIC_LIGHT_X + index as f64 * spacing;
                // AppKit can retain the pre-reveal vertical origin after it lays out the
                // initially hidden titlebar. Wry only reapplies x, so make the inset complete.
                origin.y = 0.0;
                button.setFrameOrigin(origin);
            }
        })
        .map_err(|error| format!("failed to repair traffic light position: {error}"))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn repair_macos_traffic_light_position(
    _window: &tauri::WebviewWindow,
) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn schedule_macos_traffic_light_position_repair(
    app: &tauri::AppHandle,
    window_label: &str,
) {
    let app = app.clone();
    let window_label = window_label.to_string();
    let generation = MACOS_TRAFFIC_LIGHT_REPAIR_GENERATION
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(MACOS_TRAFFIC_LIGHT_REPAIR_DELAY_MS));
        if MACOS_TRAFFIC_LIGHT_REPAIR_GENERATION.load(Ordering::Relaxed) != generation {
            return;
        }
        if let Some(window) = app.get_webview_window(&window_label) {
            let _ = repair_macos_traffic_light_position(&window);
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn schedule_macos_traffic_light_position_repair(
    _app: &tauri::AppHandle,
    _window_label: &str,
) {
}

#[tauri::command]
pub(crate) fn reveal_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_string())?;
    window
        .show()
        .map_err(|error| format!("failed to reveal main window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus main window: {error}"))?;
    repair_macos_traffic_light_position(&window)?;
    schedule_macos_traffic_light_position_repair(&app, window.label());
    append_startup_log("main window revealed by frontend");
    Ok(())
}

pub(crate) fn schedule_main_window_reveal_fallback(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(MAIN_WINDOW_REVEAL_FALLBACK_MS));
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        if window.is_visible().unwrap_or(false) {
            return;
        }
        append_startup_log("main window reveal fallback used");
        let _ = window.show();
        let _ = window.set_focus();
        let _ = repair_macos_traffic_light_position(&window);
        schedule_macos_traffic_light_position_repair(&app, window.label());
    });
}
