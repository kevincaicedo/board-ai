//! BoardAI desktop shell: opens the window and, as the runtime lands, wires the UI transport
//! to it. It holds no business logic (docs/architecture/HLD.md §3).
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    match tauri::Builder::default().run(tauri::generate_context!()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("boardai: the app failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}
