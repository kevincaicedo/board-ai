//! BoardAI desktop shell: the window, the Keychain, and the UI transport to the runtime's
//! session (D-027). It holds no business logic (docs/architecture/HLD.md §3).
#![forbid(unsafe_code)]

mod commands;
mod dev;
mod secrets;
mod session;
mod sink;

use std::process::ExitCode;

use session::AppState;

fn main() -> ExitCode {
    if boardai_runtime::logging::init_stderr(tracing::Level::INFO).is_err() {
        eprintln!("boardai: logging was already set up");
    }
    let built = tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::command,
            commands::session_log,
            #[cfg(debug_assertions)]
            dev::dev_say,
        ])
        .setup(|app| {
            dev::grant(app.handle());
            session::start(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!());
    match built {
        Ok(app) => {
            app.run(|handle, event| {
                if let tauri::RunEvent::Exit = event {
                    session::close(handle);
                }
            });
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("boardai: the app failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}
