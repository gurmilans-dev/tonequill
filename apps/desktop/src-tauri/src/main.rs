#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;
use tonequill_live::{
    audio, errors,
    events::*,
    service::{self, SessionManager},
};

#[derive(Default)]
struct Desktop {
    sessions: Arc<SessionManager>,
    closing: AtomicBool,
}
fn internal(detail: impl ToString) -> Failure {
    Failure {
        code: ErrorCode::Internal,
        detail: detail.to_string(),
    }
}
#[tauri::command]
async fn audio_devices() -> Result<Vec<DeviceInfo>, Failure> {
    tauri::async_runtime::spawn_blocking(|| audio::list_devices().map_err(|e| errors::failure(&e)))
        .await
        .map_err(internal)?
}
#[tauri::command]
async fn inspect_file(path: String) -> Result<FileInfo, Failure> {
    tauri::async_runtime::spawn_blocking(move || {
        service::inspect_file(Path::new(&path)).map_err(|e| errors::failure(&e))
    })
    .await
    .map_err(internal)?
}
#[tauri::command]
async fn start_session(
    request: SessionRequest,
    state: State<'_, Desktop>,
) -> Result<String, Failure> {
    if state.closing.load(Ordering::Acquire) {
        return Err(internal("Application is closing"));
    }
    let sessions = state.sessions.clone();
    tauri::async_runtime::spawn_blocking(move || {
        sessions.start(request).map_err(|e| errors::failure(&e))
    })
    .await
    .map_err(internal)?
}
#[tauri::command]
fn poll_session(after: u32, state: State<'_, Desktop>) -> SessionUpdate {
    state.sessions.poll(after)
}
#[tauri::command]
async fn cancel_session(id: String, state: State<'_, Desktop>) -> Result<(), Failure> {
    let sessions = state.sessions.clone();
    tauri::async_runtime::spawn_blocking(move || {
        sessions.cancel(&id).map_err(|e| errors::failure(&e))
    })
    .await
    .map_err(internal)?
}
#[tauri::command]
fn show_in_folder(path: String, app: tauri::AppHandle) -> Result<(), Failure> {
    if !Path::new(&path).is_file() {
        return Err(Failure {
            code: ErrorCode::Destination,
            detail: "This file has been moved or removed".into(),
        });
    }
    app.opener().reveal_item_in_dir(path).map_err(internal)
}
fn main() {
    tauri::Builder::default()
        .manage(Desktop::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            audio_devices,
            inspect_file,
            start_session,
            poll_session,
            cancel_session,
            show_in_folder
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let state = window.state::<Desktop>();
                if state.closing.swap(true, Ordering::AcqRel) {
                    return;
                }
                let sessions = state.sessions.clone();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = tauri::async_runtime::spawn_blocking(move || sessions.close()).await;
                    app.exit(0);
                });
            }
        })
        .build(tauri::generate_context!())
        .expect("Unable to start Tonequill")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<Desktop>().sessions.close();
            }
        });
}
