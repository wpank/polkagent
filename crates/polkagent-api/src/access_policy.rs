//! Exact public-path policy for operational HTTP endpoints.
//!
//! These endpoints must remain reachable by orchestrator probes and API
//! tooling even when authentication and rate limiting are enabled. Matching
//! is deliberately exact: adding a route does not make it public by sharing a
//! prefix with one of these paths.

/// Paths that bypass authentication and per-client rate limiting.
pub(crate) const PUBLIC_OPERATIONAL_PATHS: [&str; 5] = [
    "/health/live",
    "/health/ready",
    "/health/startup",
    "/openapi.json",
    "/v1/compat/pca/health",
];

/// Return whether `path` is an explicitly public operational endpoint.
#[must_use]
pub(crate) fn is_public_operational_path(path: &str) -> bool {
    PUBLIC_OPERATIONAL_PATHS.contains(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_matching_is_exact() {
        for path in PUBLIC_OPERATIONAL_PATHS {
            assert!(is_public_operational_path(path));
        }

        for path in [
            "/health",
            "/health/live/",
            "/metrics",
            "/api/v1alpha1/system/info",
            "/v1/compat/pca/inbound",
        ] {
            assert!(!is_public_operational_path(path), "{path}");
        }
    }
}
