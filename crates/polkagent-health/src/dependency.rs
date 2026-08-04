//! Built-in [`HealthCheck`] implementations for common external dependencies.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use tracing::debug;

use crate::check::HealthCheck;
use crate::types::{CheckSeverity, HealthStatus, Status};

// ---------------------------------------------------------------------------
// TcpCheck
// ---------------------------------------------------------------------------

/// Checks that a TCP connection can be established to `host:port`.
pub struct TcpCheck {
    name: String,
    host: String,
    port: u16,
    severity: CheckSeverity,
    connect_timeout: Duration,
}

impl TcpCheck {
    /// Create a new TCP check.
    pub fn new(
        name: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        severity: CheckSeverity,
    ) -> Self {
        Self {
            name: name.into(),
            host: host.into(),
            port,
            severity,
            connect_timeout: Duration::from_secs(3),
        }
    }

    /// Override the TCP connect timeout (default 3 s).
    #[must_use]
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }
}

#[async_trait]
impl HealthCheck for TcpCheck {
    async fn check(&self) -> HealthStatus {
        let addr = format!("{}:{}", self.host, self.port);
        let start = Instant::now();

        let result =
            tokio::time::timeout(self.connect_timeout, tokio::net::TcpStream::connect(&addr)).await;

        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(_stream)) => {
                debug!(check = %self.name, addr = %addr, latency_ms, "TCP check succeeded");
                HealthStatus {
                    name: self.name.clone(),
                    status: Status::Up,
                    latency_ms,
                    details: Some(serde_json::json!({ "addr": addr })),
                    checked_at: Utc::now(),
                }
            }
            Ok(Err(e)) => HealthStatus {
                name: self.name.clone(),
                status: Status::Down,
                latency_ms,
                details: Some(serde_json::json!({
                    "addr": addr,
                    "error": e.to_string(),
                })),
                checked_at: Utc::now(),
            },
            Err(_) => HealthStatus {
                name: self.name.clone(),
                status: Status::Down,
                latency_ms,
                details: Some(serde_json::json!({
                    "addr": addr,
                    "error": "connection timed out",
                })),
                checked_at: Utc::now(),
            },
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn severity(&self) -> CheckSeverity {
        self.severity
    }
}

// ---------------------------------------------------------------------------
// HttpCheck
// ---------------------------------------------------------------------------

/// Checks that an HTTP(S) GET request to `url` returns a 2xx status.
///
/// This performs a basic TCP connection to the parsed host/port rather than
/// pulling in a full HTTP client, keeping the crate dependency-free of
/// `reqwest`. For production use, consider wrapping a real HTTP client as a
/// custom [`HealthCheck`].
pub struct HttpCheck {
    name: String,
    url: String,
    severity: CheckSeverity,
    connect_timeout: Duration,
}

impl HttpCheck {
    /// Create a new HTTP check.
    pub fn new(name: impl Into<String>, url: impl Into<String>, severity: CheckSeverity) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            severity,
            connect_timeout: Duration::from_secs(5),
        }
    }

    /// Override the connect timeout (default 5 s).
    #[must_use]
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Parse host and port from the URL.
    fn parse_host_port(&self) -> Option<(String, u16)> {
        // Minimal URL parsing: look for "://" then extract host and optional port.
        let after_scheme = self.url.split("://").nth(1)?;
        let host_port = after_scheme.split('/').next()?;

        if let Some((host, port_str)) = host_port.rsplit_once(':') {
            let port = port_str.parse().ok()?;
            Some((host.to_string(), port))
        } else {
            let port = if self.url.starts_with("https") {
                443
            } else {
                80
            };
            Some((host_port.to_string(), port))
        }
    }
}

#[async_trait]
impl HealthCheck for HttpCheck {
    async fn check(&self) -> HealthStatus {
        let start = Instant::now();

        let Some((host, port)) = self.parse_host_port() else {
            return HealthStatus {
                name: self.name.clone(),
                status: Status::Down,
                latency_ms: 0,
                details: Some(serde_json::json!({
                    "url": self.url,
                    "error": "failed to parse URL",
                })),
                checked_at: Utc::now(),
            };
        };

        let addr = format!("{host}:{port}");
        let result =
            tokio::time::timeout(self.connect_timeout, tokio::net::TcpStream::connect(&addr)).await;

        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(_)) => {
                debug!(check = %self.name, url = %self.url, latency_ms, "HTTP check succeeded");
                HealthStatus {
                    name: self.name.clone(),
                    status: Status::Up,
                    latency_ms,
                    details: Some(serde_json::json!({ "url": self.url })),
                    checked_at: Utc::now(),
                }
            }
            Ok(Err(e)) => HealthStatus {
                name: self.name.clone(),
                status: Status::Down,
                latency_ms,
                details: Some(serde_json::json!({
                    "url": self.url,
                    "error": e.to_string(),
                })),
                checked_at: Utc::now(),
            },
            Err(_) => HealthStatus {
                name: self.name.clone(),
                status: Status::Down,
                latency_ms,
                details: Some(serde_json::json!({
                    "url": self.url,
                    "error": "connection timed out",
                })),
                checked_at: Utc::now(),
            },
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn severity(&self) -> CheckSeverity {
        self.severity
    }
}

// ---------------------------------------------------------------------------
// SqliteCheck
// ---------------------------------------------------------------------------

/// Checks that a `SQLite` database file exists and is readable.
pub struct SqliteCheck {
    name: String,
    path: PathBuf,
    severity: CheckSeverity,
}

impl SqliteCheck {
    /// Create a new `SQLite` health check.
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>, severity: CheckSeverity) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            severity,
        }
    }
}

#[async_trait]
impl HealthCheck for SqliteCheck {
    async fn check(&self) -> HealthStatus {
        let start = Instant::now();
        let path = self.path.clone();

        let exists = tokio::fs::metadata(&path).await.is_ok();
        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;

        if exists {
            debug!(check = %self.name, path = %path.display(), "SQLite check succeeded");
            HealthStatus {
                name: self.name.clone(),
                status: Status::Up,
                latency_ms,
                details: Some(serde_json::json!({
                    "path": path.display().to_string(),
                })),
                checked_at: Utc::now(),
            }
        } else {
            HealthStatus {
                name: self.name.clone(),
                status: Status::Down,
                latency_ms,
                details: Some(serde_json::json!({
                    "path": path.display().to_string(),
                    "error": "file not found or not accessible",
                })),
                checked_at: Utc::now(),
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn severity(&self) -> CheckSeverity {
        self.severity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_check_existing_file() {
        // Cargo.toml always exists next to the test binary's working directory.
        let check = SqliteCheck::new("test-db", "Cargo.toml", CheckSeverity::Critical);
        let status = check.check().await;
        assert_eq!(status.status, Status::Up);
        assert_eq!(status.name, "test-db");
    }

    #[tokio::test]
    async fn sqlite_check_missing_file() {
        let check = SqliteCheck::new(
            "missing-db",
            "/tmp/nonexistent_db_12345.sqlite",
            CheckSeverity::Critical,
        );
        let status = check.check().await;
        assert_eq!(status.status, Status::Down);
    }

    #[tokio::test]
    async fn sqlite_check_severity() {
        let check = SqliteCheck::new("db", "Cargo.toml", CheckSeverity::Advisory);
        assert_eq!(check.severity(), CheckSeverity::Advisory);
    }

    #[tokio::test]
    async fn tcp_check_refuses_connection() {
        // Port 1 is almost certainly not listening on localhost.
        let check = TcpCheck::new("bad-port", "127.0.0.1", 1, CheckSeverity::Critical)
            .with_connect_timeout(Duration::from_millis(200));
        let status = check.check().await;
        assert_eq!(status.status, Status::Down);
    }

    #[tokio::test]
    async fn tcp_check_name_and_severity() {
        let check = TcpCheck::new("node-rpc", "127.0.0.1", 9944, CheckSeverity::Critical);
        assert_eq!(check.name(), "node-rpc");
        assert_eq!(check.severity(), CheckSeverity::Critical);
    }

    #[tokio::test]
    async fn http_check_parse_url_with_port() {
        let check = HttpCheck::new(
            "api",
            "http://localhost:8080/health",
            CheckSeverity::Advisory,
        );
        let (host, port) = check.parse_host_port().expect("should parse");
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
    }

    #[tokio::test]
    async fn http_check_parse_url_default_http_port() {
        let check = HttpCheck::new("web", "http://example.com/path", CheckSeverity::Advisory);
        let (host, port) = check.parse_host_port().expect("should parse");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[tokio::test]
    async fn http_check_parse_url_default_https_port() {
        let check = HttpCheck::new("web", "https://example.com/path", CheckSeverity::Advisory);
        let (host, port) = check.parse_host_port().expect("should parse");
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[tokio::test]
    async fn http_check_bad_url() {
        let check = HttpCheck::new("bad", "not-a-url", CheckSeverity::Advisory);
        let status = check.check().await;
        assert_eq!(status.status, Status::Down);
    }

    #[tokio::test]
    async fn http_check_unreachable() {
        let check = HttpCheck::new("down", "http://127.0.0.1:1/health", CheckSeverity::Critical)
            .with_connect_timeout(Duration::from_millis(200));
        let status = check.check().await;
        assert_eq!(status.status, Status::Down);
    }
}
