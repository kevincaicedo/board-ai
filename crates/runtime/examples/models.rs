//! `just models [name…]`: shows every model file's status and size; with names (or `all`),
//! downloads and verifies them into the app's data folder, resuming partial files.
//! `BOARDAI_MODELS_DIR` overrides the folder.
#![allow(clippy::print_stdout, reason = "a dev tool prints its findings")]

use std::path::PathBuf;
use std::time::Instant;

use boardai_runtime::models::{ModelId, ModelStatus, ModelStore};

fn root() -> Result<PathBuf, String> {
    if let Some(folder) = std::env::var_os("BOARDAI_MODELS_DIR") {
        return Ok(PathBuf::from(folder));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(ModelStore::default_root(&PathBuf::from(home)))
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let store = ModelStore::new(root()?).map_err(|error| error.to_string())?;
    let names: Vec<String> = std::env::args().skip(1).collect();
    let wanted: Vec<ModelId> = if names.iter().any(|name| name == "all") {
        ModelId::ALL.to_vec()
    } else {
        names
            .iter()
            .map(|name| ModelId::parse(name).ok_or(format!("unknown model {name}")))
            .collect::<Result<_, _>>()?
    };
    println!("models folder: {}", store.root().display());
    for id in ModelId::ALL {
        let file = id.file();
        let status = match store.status(&file) {
            ModelStatus::Missing { size_bytes } => {
                format!("missing ({:.1} MB)", megabytes(size_bytes))
            }
            ModelStatus::Partial {
                have_bytes,
                size_bytes,
            } => format!(
                "partial ({:.1} of {:.1} MB)",
                megabytes(have_bytes),
                megabytes(size_bytes)
            ),
            ModelStatus::Ready { .. } => "ready".to_owned(),
        };
        println!("  {:<24} {status:<28} {}", id.name(), file.license);
    }
    for id in wanted {
        let file = id.file();
        let started = Instant::now();
        let mut last_percent = None;
        let mut report = |progress: boardai_runtime::models::DownloadProgress| {
            let percent = progress.received_bytes * 100 / progress.total_bytes.max(1);
            if last_percent != Some(percent / 10) {
                last_percent = Some(percent / 10);
                println!("  {} {percent}%", id.name());
            }
        };
        let path = store
            .fetch(&file, &mut report)
            .await
            .map_err(|error| format!("{}: {error}", id.name()))?;
        println!(
            "{} verified (SHA-256 {}) in {:.1} s: {}",
            id.name(),
            file.sha256,
            started.elapsed().as_secs_f64(),
            path.display()
        );
    }
    Ok(())
}
