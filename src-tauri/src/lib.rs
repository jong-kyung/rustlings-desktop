#[cfg(target_os = "windows")]
compile_error!("lustlings-desktop currently supports macOS local development only");

pub mod commands;
pub mod curriculum;
pub mod diagnostics;
pub mod process;
pub mod session;
pub mod toolchain;
pub mod validator;
pub mod workspace;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use std::{fs, sync::Arc};
    use tauri::Manager;

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            },
        ))
        .setup(|app| {
            let resource_root = app.path().resource_dir()?.join("resources/rustlings-6.5.0");
            let curriculum = curriculum::Curriculum::load(resource_root)?;
            let app_data = app.path().app_data_dir()?;
            fs::create_dir_all(&app_data)?;
            let owner = workspace::WorkspaceOwner::acquire(&app_data)?;
            let workspace = workspace::Workspace::open(owner, &curriculum)?;
            let runner = process::ProcessRunner::new();
            let toolchain = tauri::async_runtime::block_on(toolchain::Toolchain::discover())
                .map_err(|error| error.to_string());
            let session = Arc::new(session::Session::new(
                curriculum,
                workspace,
                runner.clone(),
                toolchain,
            ));
            app.manage(runner);
            app.manage(session);

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session_snapshot,
            commands::retry_preflight,
            commands::save_source,
            commands::select_exercise,
            commands::reveal_hint,
            commands::run_exercise,
            commands::run_result,
            commands::cancel_run,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            tauri::async_runtime::block_on(handle.state::<Arc<session::Session>>().shutdown());
        }
    });
}
