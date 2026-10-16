//! Model downloads against the local test server: fresh, resumed, restarted, corrupt and
//! oversized. No network.
#![allow(clippy::expect_used, reason = "tests fail loudly")]

mod support;

use boardai_runtime::models::{DownloadProgress, ModelError, ModelFile, ModelStatus, ModelStore};
use ring::digest::{SHA256, digest};
use support::{Scripted, TempFolder, TestServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn body(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index * 31 % 251) as u8).collect()
}

fn sha256(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bytes(status: u16, chunks: Vec<Vec<u8>>) -> Scripted {
    Scripted {
        status,
        headers: vec![("content-type".into(), "application/octet-stream".into())],
        chunks,
        chunk_delay: std::time::Duration::ZERO,
        hang: false,
    }
}

struct Case {
    server: TestServer,
    folder: TempFolder,
    url: String,
    sha: String,
    size: u64,
}

impl Case {
    async fn new(content: &[u8], script: Vec<Scripted>) -> Self {
        let server = TestServer::start(script).await;
        let url = format!("{}/models/file.bin", server.base_url);
        Self {
            server,
            folder: TempFolder::new("models"),
            url,
            sha: sha256(content),
            size: content.len() as u64,
        }
    }

    fn file(&self) -> ModelFile<'_> {
        ModelFile {
            file_name: "file.bin",
            url: &self.url,
            size_bytes: self.size,
            sha256: &self.sha,
            license: "MIT",
        }
    }

    fn store(&self) -> ModelStore {
        ModelStore::new(self.folder.path.clone()).expect("client")
    }

    async fn fetch(
        &self,
    ) -> (
        Result<std::path::PathBuf, ModelError>,
        Vec<DownloadProgress>,
    ) {
        let mut reports = Vec::new();
        let result = self
            .store()
            .fetch(&self.file(), &mut |progress| reports.push(progress))
            .await;
        (result, reports)
    }
}

#[tokio::test]
async fn a_fresh_download_is_verified_and_reports_progress() {
    let content = body(3 * 1024 * 1024 + 7);
    let chunks = content.chunks(64 * 1024).map(<[u8]>::to_vec).collect();
    let case = Case::new(&content, vec![bytes(200, chunks)]).await;
    assert_eq!(
        case.store().status(&case.file()),
        ModelStatus::Missing {
            size_bytes: case.size
        }
    );
    let (result, reports) = case.fetch().await;
    let path = result.expect("downloaded");
    assert_eq!(std::fs::read(&path).expect("file"), content);
    assert!(matches!(
        case.store().status(&case.file()),
        ModelStatus::Ready { .. }
    ));
    assert_eq!(
        reports.len(),
        4,
        "three 1 MB steps and the end: {reports:?}"
    );
    assert_eq!(reports.last().map(|p| p.received_bytes), Some(case.size));
    // Already there: no second request.
    assert!(case.fetch().await.0.is_ok());
    assert_eq!(case.server.requests().len(), 1);
}

#[tokio::test]
async fn a_partial_download_resumes_with_a_range() {
    let content = body(10_000);
    let rest = bytes(206, vec![content[4_000..].to_vec()])
        .with_header("content-range", "bytes 4000-9999/10000");
    let case = Case::new(&content, vec![rest]).await;
    case.folder.write("file.bin.part", &content[..4_000]);
    assert_eq!(
        case.store().status(&case.file()),
        ModelStatus::Partial {
            have_bytes: 4_000,
            size_bytes: 10_000
        }
    );
    let path = case.fetch().await.0.expect("resumed");
    assert_eq!(std::fs::read(path).expect("file"), content);
    let requests = case.server.requests();
    assert_eq!(requests[0].header("range"), Some("bytes=4000-"));
    assert!(!case.folder.path.join("file.bin.part").exists());
}

#[tokio::test]
async fn a_server_that_ignores_the_range_restarts_the_file() {
    let content = body(10_000);
    let case = Case::new(&content, vec![bytes(200, vec![content.clone()])]).await;
    case.folder.write("file.bin.part", &[9; 4_000]);
    let path = case.fetch().await.0.expect("restarted");
    assert_eq!(std::fs::read(path).expect("file"), content);
}

#[tokio::test]
async fn a_corrupt_download_is_deleted() {
    let content = body(1_000);
    let mut tampered = content.clone();
    tampered[500] ^= 1;
    let case = Case::new(&content, vec![bytes(200, vec![tampered])]).await;
    let (result, _) = case.fetch().await;
    assert!(
        matches!(result, Err(ModelError::Corrupt { .. })),
        "{result:?}"
    );
    assert!(!case.folder.path.join("file.bin").exists());
    assert!(!case.folder.path.join("file.bin.part").exists());
}

#[tokio::test]
async fn more_bytes_than_the_manifest_says_fail() {
    let content = body(1_000);
    let case = Case::new(&content, vec![bytes(200, vec![body(1_500)])]).await;
    let (result, _) = case.fetch().await;
    assert!(
        matches!(result, Err(ModelError::BadResponse { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_short_download_keeps_its_partial_for_later() {
    let content = body(1_000);
    let case = Case::new(&content, vec![bytes(200, vec![content[..600].to_vec()])]).await;
    let (result, _) = case.fetch().await;
    assert!(
        matches!(result, Err(ModelError::Network { .. })),
        "{result:?}"
    );
    assert_eq!(
        case.store().status(&case.file()),
        ModelStatus::Partial {
            have_bytes: 600,
            size_bytes: 1_000
        }
    );
}

/// The flake in `a_short_download_keeps_its_partial_for_later`, made certain: tokio writes a
/// file on its blocking pool, and under load the write of the 600 bytes had not landed when the
/// failed fetch returned, so the partial read as missing. Here the only blocking thread is busy
/// while the body arrives; the fetch must still return with the bytes on disk.
#[test]
fn a_failed_download_returns_only_once_its_bytes_are_on_disk() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let content = body(1_000);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let served = content[..600].to_vec();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).await;
            // Holds the blocking pool while the client writes what it received.
            let _busy = tokio::task::spawn_blocking(|| {
                std::thread::sleep(std::time::Duration::from_millis(300));
            });
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                served.len()
            );
            socket.write_all(head.as_bytes()).await.expect("head");
            socket.write_all(&served).await.expect("body");
            socket.shutdown().await.expect("close");
        });
        let folder = TempFolder::new("models-pool");
        let url = format!("http://{address}/models/file.bin");
        let sha = sha256(&content);
        let file = ModelFile {
            file_name: "file.bin",
            url: &url,
            size_bytes: 1_000,
            sha256: &sha,
            license: "MIT",
        };
        let store = ModelStore::new(folder.path.clone()).expect("client");
        let result = store.fetch(&file, &mut |_| {}).await;
        assert!(
            matches!(result, Err(ModelError::Network { .. })),
            "{result:?}"
        );
        assert_eq!(
            store.status(&file),
            ModelStatus::Partial {
                have_bytes: 600,
                size_bytes: 1_000
            }
        );
        server.await.expect("server");
    });
}

#[tokio::test]
async fn an_error_status_keeps_its_code() {
    let case = Case::new(&body(10), vec![Scripted::status(404, "{}")]).await;
    let (result, _) = case.fetch().await;
    assert_eq!(result, Err(ModelError::Http { status: 404 }));
}
