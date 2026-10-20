use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use vfs::OpendalFileStorage;

use crate::config::DownloaderConfig;
use crate::error::{DownloadError, Result};
use crate::integrity::{TransferDigest, verify_sha256};
use crate::policy::DownloadPolicy;
use crate::progress::ProgressThrottle;
use crate::uri::ResourceUri;

/// Successful transfer receipt returned to tools and programmatic callers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DownloadReceipt {
    pub id: String,
    pub source: String,
    pub destination: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum DownloadSource {
    Uri {
        uri: String,
    },
    ObjectStore {
        profile: String,
        bucket: String,
        key: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DownloadRequest {
    pub source: DownloadSource,
    pub destination: String,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
}

impl Default for DownloadRequest {
    fn default() -> Self {
        Self {
            source: DownloadSource::Uri { uri: String::new() },
            destination: "/downloads/download".to_string(),
            overwrite: false,
            expected_sha256: None,
        }
    }
}

impl DownloadRequest {
    pub fn from_uri(uri: impl Into<String>, destination: impl Into<String>) -> Self {
        Self {
            source: DownloadSource::Uri { uri: uri.into() },
            destination: destination.into(),
            overwrite: false,
            expected_sha256: None,
        }
    }
}

#[derive(Clone)]
pub struct DownloadManager {
    config: DownloaderConfig,
    storage: Arc<OpendalFileStorage>,
    #[cfg(feature = "http")]
    client: reqwest::Client,
    #[cfg(test)]
    allow_direct_ip_for_tests: bool,
}

impl DownloadManager {
    #[cfg(feature = "http")]
    pub fn new(config: DownloaderConfig, storage: Arc<OpendalFileStorage>) -> Result<Self> {
        if config.concurrency == 0 {
            return Err(DownloadError::Denied {
                resource: "downloader config".to_string(),
                message: "concurrency must be greater than zero".to_string(),
            });
        }
        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                DownloadError::request(
                    "downloader client",
                    format!("cannot build HTTP client: {error}"),
                )
            })?;
        Ok(Self {
            config,
            storage,
            client,
            #[cfg(test)]
            allow_direct_ip_for_tests: false,
        })
    }

    /// Build a manager that trusts a mock server's self-signed certificate.
    /// This is unavailable outside `cargo test`, including test-only feature
    /// combinations used by dependent crates.
    #[cfg(all(test, feature = "http"))]
    fn new_for_local_http_mock(
        config: DownloaderConfig,
        storage: Arc<OpendalFileStorage>,
    ) -> Result<Self> {
        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|error| {
                DownloadError::request(
                    "downloader client",
                    format!("cannot build HTTP client: {error}"),
                )
            })?;
        Ok(Self {
            config,
            storage,
            client,
            #[cfg(test)]
            allow_direct_ip_for_tests: true,
        })
    }

    #[cfg(not(feature = "http"))]
    pub fn new(config: DownloaderConfig, storage: Arc<OpendalFileStorage>) -> Result<Self> {
        if config.concurrency == 0 {
            return Err(DownloadError::Denied {
                resource: "downloader config".to_string(),
                message: "concurrency must be greater than zero".to_string(),
            });
        }
        Ok(Self {
            config,
            storage,
            #[cfg(test)]
            allow_direct_ip_for_tests: false,
        })
    }

    pub fn policy(&self) -> &DownloadPolicy {
        &self.config.policy
    }

    #[cfg(feature = "http")]
    pub async fn download(&self, request: DownloadRequest) -> Result<DownloadReceipt> {
        self.download_with_progress(request, |_downloaded, _total| {})
            .await
    }

    #[cfg(feature = "http")]
    pub async fn download_with_progress<F>(
        &self,
        request: DownloadRequest,
        on_progress: F,
    ) -> Result<DownloadReceipt>
    where
        F: FnMut(u64, Option<u64>),
    {
        self.transfer(request, on_progress).await
    }

    #[cfg(test)]
    async fn download_with_progress_for_tests<F>(
        &self,
        request: DownloadRequest,
        on_progress: F,
    ) -> Result<DownloadReceipt>
    where
        F: FnMut(u64, Option<u64>),
    {
        self.transfer(request, on_progress).await
    }

    #[cfg(all(feature = "object-store", feature = "http"))]
    async fn download_object<F>(
        &self,
        profile_id: &str,
        bucket: &str,
        key: &str,
        request: &DownloadRequest,
        mut on_progress: F,
    ) -> Result<DownloadReceipt>
    where
        F: FnMut(u64, Option<u64>),
    {
        let profile = self
            .config
            .object_store_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                DownloadError::denied(
                    format!("object-store profile `{profile_id}`"),
                    "profile does not exist",
                )
            })?;
        if profile.bucket != bucket {
            return Err(DownloadError::denied(
                format!("object-store profile `{profile_id}`"),
                format!("bucket `{bucket}` is not authorized"),
            ));
        }
        profile.check_key(key)?;
        self.config.policy.check_destination(&request.destination)?;
        self.check_destination_absent(&request.destination, request.overwrite)
            .await?;
        let operator = profile.build_operator().map_err(|error| {
            DownloadError::request(format!("{profile_id}:{bucket}"), error.to_string())
        })?;
        let object_key = key.trim_start_matches('/');
        let meta = operator.stat(object_key).await.map_err(|error| {
            DownloadError::request(format!("{profile_id}:{bucket}/{key}"), error.to_string())
        })?;
        let total = meta.content_length();
        self.config
            .policy
            .check_size(&format!("{profile_id}:{bucket}/{key}"), Some(total))?;
        let mut stream = operator
            .reader(object_key)
            .await
            .map_err(|error| {
                DownloadError::request(format!("{profile_id}:{bucket}/{key}"), error.to_string())
            })?
            .into_stream(0..total)
            .await
            .map_err(|error| {
                DownloadError::request(format!("{profile_id}:{bucket}/{key}"), error.to_string())
            })?;
        let mut sink = self
            .storage
            .atomic_writer(&request.destination)
            .await
            .map_err(|error| DownloadError::Write {
                path: request.destination.clone(),
                message: error.to_string(),
            })?;
        let mut digest = TransferDigest::default();
        let mut downloaded = 0_u64;
        on_progress(0, Some(total));
        while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
            let chunk = chunk.map_err(|error| {
                DownloadError::request(format!("{profile_id}:{bucket}/{key}"), error.to_string())
            })?;
            let chunk = chunk.to_vec();
            downloaded += chunk.len() as u64;
            if downloaded > total || downloaded > self.config.policy.max_bytes {
                let _ = sink.abort().await;
                return Err(DownloadError::TooLarge {
                    resource: format!("{profile_id}:{bucket}/{key}"),
                    actual: downloaded,
                    limit: self.config.policy.max_bytes,
                });
            }
            digest.update(&chunk);
            sink.write(chunk)
                .await
                .map_err(|error| DownloadError::Write {
                    path: request.destination.clone(),
                    message: error.to_string(),
                })?;
            on_progress(downloaded, Some(total));
        }
        if downloaded != total {
            let _ = sink.abort().await;
            return Err(DownloadError::request(
                format!("{profile_id}:{bucket}/{key}"),
                format!("short read: expected {total} bytes, got {downloaded}"),
            ));
        }
        let sha256 = digest.finish();
        if let Some(expected) = &request.expected_sha256 {
            verify_sha256(expected, &sha256, downloaded).map_err(|message| {
                DownloadError::Integrity {
                    resource: format!("{profile_id}:{bucket}/{key}"),
                    message,
                }
            })?;
        }
        sink.commit().await.map_err(|error| DownloadError::Write {
            path: request.destination.clone(),
            message: error.to_string(),
        })?;
        Ok(DownloadReceipt {
            id: uuid::Uuid::now_v7().to_string(),
            source: format!("{profile_id}:{bucket}/{key}"),
            destination: request.destination.clone(),
            bytes: downloaded,
            sha256,
        })
    }

    async fn check_destination_absent(&self, destination: &str, overwrite: bool) -> Result<()> {
        if overwrite {
            return Ok(());
        }
        match self.storage.content_length(destination).await {
            Ok(_) => Err(DownloadError::DestinationExists {
                path: destination.to_string(),
            }),
            Err(error) if error.kind() != opendal::ErrorKind::NotFound => {
                Err(DownloadError::Write {
                    path: destination.to_string(),
                    message: error.to_string(),
                })
            }
            Err(_) => Ok(()),
        }
    }

    async fn transfer<F>(&self, request: DownloadRequest, on_progress: F) -> Result<DownloadReceipt>
    where
        F: FnMut(u64, Option<u64>),
    {
        match request.source {
            DownloadSource::ObjectStore {
                ref profile,
                ref bucket,
                ref key,
            } => {
                #[cfg(all(feature = "object-store", feature = "http"))]
                {
                    let result = self
                        .download_object(profile, bucket, key, &request, on_progress)
                        .await?;
                    Ok(result)
                }
                #[cfg(not(all(feature = "object-store", feature = "http")))]
                {
                    let _ = (profile, bucket, key, on_progress);
                    return Err(DownloadError::denied(
                        format!("object-store profile `{profile}`"),
                        "object-store support is not enabled",
                    ));
                }
            }
            DownloadSource::Uri { ref uri } => self.download_uri(uri, &request, on_progress).await,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_uri<F>(
        &self,
        uri_text: &str,
        request: &DownloadRequest,
        mut on_progress: F,
    ) -> Result<DownloadReceipt>
    where
        F: FnMut(u64, Option<u64>),
    {
        #[cfg(test)]
        let allow_direct_ip_for_tests = self.allow_direct_ip_for_tests;
        #[cfg(not(test))]
        let allow_direct_ip_for_tests = false;

        let uri = ResourceUri::parse_with_options(uri_text, allow_direct_ip_for_tests)?;

        self.config.policy.check_source(&uri)?;
        self.config.policy.check_destination(&request.destination)?;
        self.check_destination_absent(&request.destination, request.overwrite)
            .await?;

        let mut current = uri;
        let mut redirects = 0usize;
        let response = loop {
            let response = self
                .client
                .get(current.display())
                .send()
                .await
                .map_err(|error| DownloadError::request(current.display(), error.to_string()))?;

            if response.status().is_redirection() {
                redirects += 1;
                if redirects > self.config.policy.max_redirects {
                    return Err(DownloadError::request(
                        current.display(),
                        format!(
                            "too many redirects (limit {})",
                            self.config.policy.max_redirects
                        ),
                    ));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        DownloadError::request(
                            current.display(),
                            "redirect response has no valid Location header",
                        )
                    })?;
                let next_raw = current
                    .display()
                    .parse::<url::Url>()
                    .expect("validated URL")
                    .join(location)
                    .map_err(|error| {
                        DownloadError::request(
                            current.display(),
                            format!("invalid redirect target `{location}`: {error}"),
                        )
                    })?
                    .to_string();
                let next = ResourceUri::parse_with_options(&next_raw, allow_direct_ip_for_tests)?;
                self.config.policy.check_source(&next)?;
                current = next;
                continue;
            }

            if let Err(error) = response.error_for_status_ref() {
                return Err(DownloadError::request(
                    current.display(),
                    format!("HTTP {}", error.status().unwrap_or_default()),
                ));
            }
            break response;
        };

        let source_display = current.display();
        let total = response.content_length();
        self.config.policy.check_size(&source_display, total)?;

        let mut stream = response.bytes_stream();
        let mut sink = self
            .storage
            .atomic_writer(&request.destination)
            .await
            .map_err(|error| DownloadError::Write {
                path: request.destination.clone(),
                message: error.to_string(),
            })?;
        let mut digest = TransferDigest::default();
        let mut downloaded: u64 = 0;
        let mut last_progress = 0u64;
        let mut throttle = ProgressThrottle::new(Duration::from_millis(500));
        on_progress(0, total);

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                DownloadError::request(source_display.clone(), error.to_string())
            })?;
            downloaded += chunk.len() as u64;
            if downloaded > self.config.policy.max_bytes {
                let _ = sink.abort().await;
                return Err(DownloadError::TooLarge {
                    resource: source_display.clone(),
                    actual: downloaded,
                    limit: self.config.policy.max_bytes,
                });
            }
            digest.update(&chunk);
            sink.write(chunk.clone())
                .await
                .map_err(|error| DownloadError::Write {
                    path: request.destination.clone(),
                    message: error.to_string(),
                })?;
            if downloaded == total.unwrap_or_default()
                || downloaded - last_progress >= 1024 * 1024
                || throttle.should_emit()
            {
                last_progress = downloaded;
                on_progress(downloaded, total);
            }
        }
        self.config
            .policy
            .check_size(&source_display, Some(downloaded))?;

        let sha256 = digest.finish();
        if let Some(expected) = &request.expected_sha256 {
            verify_sha256(expected, &sha256, downloaded).map_err(|message| {
                DownloadError::Integrity {
                    resource: source_display.clone(),
                    message,
                }
            })?;
        }

        sink.commit().await.map_err(|error| DownloadError::Write {
            path: request.destination.clone(),
            message: error.to_string(),
        })?;

        Ok(DownloadReceipt {
            id: uuid::Uuid::now_v7().to_string(),
            source: source_display,
            destination: request.destination.clone(),
            bytes: downloaded,
            sha256,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> Arc<OpendalFileStorage> {
        Arc::new(OpendalFileStorage::new(tempfile::tempdir().unwrap().keep()))
    }

    #[cfg(feature = "http")]
    async fn write_file(storage: &OpendalFileStorage, path: &str, bytes: &[u8]) {
        storage.write_bytes(path, bytes.to_vec()).await.unwrap();
    }

    #[test]
    fn rejects_zero_concurrency() {
        let mut config = DownloaderConfig::default();
        config.concurrency = 0;
        assert!(DownloadManager::new(config, storage()).is_err());
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn rejects_policy_and_destination_before_network_access() {
        let mut config = DownloaderConfig::default();
        config.policy = DownloadPolicy::allow_https("example.com", "/data/");
        let manager = DownloadManager::new_for_local_http_mock(config, storage()).unwrap();
        let denied = manager
            .download(DownloadRequest {
                source: DownloadSource::Uri {
                    uri: "https://other.example/file".to_string(),
                },
                destination: "/downloads/file".to_string(),
                overwrite: false,
                expected_sha256: None,
            })
            .await
            .unwrap_err();
        assert!(denied.to_string().contains("allowlist"));
        let bad_destination = manager
            .download(DownloadRequest {
                source: DownloadSource::Uri {
                    uri: "https://example.com/data/file".to_string(),
                },
                destination: "/etc/passwd".to_string(),
                overwrite: false,
                expected_sha256: None,
            })
            .await
            .unwrap_err();
        assert!(bad_destination.to_string().contains("/downloads"));
    }

    #[cfg(feature = "http")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_stream_download_and_rewrites_to_vfs_atomically() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let host = server.uri().trim_start_matches("http://").to_string();

        let mut config = DownloaderConfig::default();
        config.policy = DownloadPolicy::allow_https(host.clone(), "/");
        config.policy.max_bytes = 100;

        let target = storage();
        let manager = DownloadManager::new_for_local_http_mock(config, target.clone()).unwrap();
        let payload = b"central downloader";
        Mock::given(method("GET"))
            .and(path("/resource.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "text/plain")
                    .set_body_bytes(payload.as_slice()),
            )
            .mount(&server)
            .await;

        let mut events = Vec::new();
        let receipt = manager
            .download_with_progress_for_tests(
                DownloadRequest {
                    source: DownloadSource::Uri {
                        uri: format!("http://{host}/resource.txt"),
                    },
                    destination: "/downloads/resource.txt".to_string(),
                    overwrite: false,
                    expected_sha256: Some(
                        "840db73a04f5ec9ac7e3dc47d0e11b8962a383996ba0973526b6e22a8a90d998"
                            .to_string(),
                    ),
                },
                |bytes, total| events.push((bytes, total)),
            )
            .await
            .unwrap();

        assert_eq!(receipt.bytes, payload.len() as u64);
        assert_eq!(receipt.destination, "/downloads/resource.txt");
        assert_eq!(
            events.last().copied(),
            Some((payload.len() as u64, Some(payload.len() as u64)))
        );
        let stored = target
            .read_range("/downloads/resource.txt", 0..payload.len() as u64)
            .await
            .unwrap();
        assert_eq!(stored.to_vec(), payload);
        write_file(&target, "/downloads/resource.txt", b"sentinel").await;
    }

    #[cfg(feature = "http")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redirect_target_must_pass_policy_and_max_size_is_enforced() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let host = server.uri().trim_start_matches("http://").to_string();

        let mut config = DownloaderConfig::default();
        config.policy = DownloadPolicy::allow_https(host.clone(), "/");
        config.policy.max_bytes = 1;

        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/large"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/large"))
            .respond_with(ResponseTemplate::new(200).set_body_string("abc"))
            .mount(&server)
            .await;

        let manager = DownloadManager::new_for_local_http_mock(config, storage()).unwrap();
        let error = manager
            .download_with_progress_for_tests(
                DownloadRequest {
                    source: DownloadSource::Uri {
                        uri: format!("http://{host}/redirect"),
                    },
                    destination: "/downloads/x".to_string(),
                    overwrite: false,
                    expected_sha256: None,
                },
                |_, _| {},
            )
            .await
            .unwrap_err();
        assert!(matches!(error, DownloadError::TooLarge { .. }));
    }
}
