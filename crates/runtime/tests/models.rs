//! Model downloads against the local test server: fresh, resumed, restarted, corrupt and
//! oversized. No network.
#![allow(clippy::expect_used, reason = "tests fail loudly")]

mod support;

use boardai_runtime::models::{DownloadProgress, ModelError, ModelFile, ModelStatus, ModelStore};
use ring::digest::{SHA256, digest};
use support::{Scripted, TempFolder, TestServer};

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

#[tokio::test]
async fn an_error_status_keeps_its_code() {
    let case = Case::new(&body(10), vec![Scripted::status(404, "{}")]).await;
    let (result, _) = case.fetch().await;
    assert_eq!(result, Err(ModelError::Http { status: 404 }));
}
