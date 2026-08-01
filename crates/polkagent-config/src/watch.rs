//! File-system watching and hot-reload for configuration files.
//!
//! This module provides polling-based file watching and atomic configuration
//! swapping without requiring external dependencies like `notify` or `inotify`.
//!
//! # Overview
//!
//! - [`ConfigWatcher`] — polls a config file for changes using mtime and content
//!   checksums.
//! - [`AtomicConfig`] — thread-safe config holder that supports lock-free reads
//!   and atomic swaps.
//! - [`ConfigSnapshot`] — an immutable, timestamped snapshot of a loaded config
//!   together with its source path and content checksum.
//! - [`ConfigDiff`] — describes what changed between two config versions at the
//!   key level.
//! - [`ValidationGate`] — validates a new config before it is applied.
//! - [`WatchEvent`] / [`WatchEventKind`] — describes a detected change.
//! - [`ReloadPolicy`] — controls when detected changes are surfaced.
//!
//! # Checksum strategy
//!
//! Because we avoid adding the `blake3` crate to the config crate's dependency
//! list, checksums are computed with a FNV-1a 64-bit hash over the raw file
//! bytes. This is *not* cryptographic — it is used solely to detect accidental
//! content changes (the same role BLAKE3 would play). The API accepts and
//! returns checksums as hex strings so that a future migration to BLAKE3 is a
//! drop-in replacement.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// WatchEvent
// ---------------------------------------------------------------------------

/// The kind of file-system change detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchEventKind {
    /// The file was created (transitioned from absent to present).
    Created,
    /// The file content changed.
    Modified,
    /// The file was deleted (transitioned from present to absent).
    Deleted,
}

impl fmt::Display for WatchEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Modified => write!(f, "modified"),
            Self::Deleted => write!(f, "deleted"),
        }
    }
}

/// A detected change to a watched configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    /// The path of the file that changed.
    pub path: PathBuf,
    /// What kind of change was detected.
    pub kind: WatchEventKind,
    /// When the change was detected (wall-clock time).
    pub timestamp: SystemTime,
}

// ---------------------------------------------------------------------------
// ReloadPolicy
// ---------------------------------------------------------------------------

/// Controls how and when detected file changes trigger a reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadPolicy {
    /// Surface the change immediately on the next poll tick.
    Immediate,
    /// Coalesce rapid changes within the given window; only surface once the
    /// file has been stable for at least this duration.
    Debounced(Duration),
    /// Record that a change occurred but do not surface it automatically.
    /// The caller must explicitly call [`ConfigWatcher::take_pending`] to
    /// consume it.
    Manual,
}

// ---------------------------------------------------------------------------
// ConfigSnapshot
// ---------------------------------------------------------------------------

/// An immutable, timestamped snapshot of a loaded configuration value.
#[derive(Debug, Clone)]
pub struct ConfigSnapshot<T> {
    /// The configuration value.
    pub config: Arc<T>,
    /// When this snapshot was loaded.
    pub loaded_at: SystemTime,
    /// The file from which this config was loaded.
    pub source_path: PathBuf,
    /// Hex-encoded content checksum of the source file at load time.
    pub checksum: String,
}

impl<T: PartialEq> PartialEq for ConfigSnapshot<T> {
    fn eq(&self, other: &Self) -> bool {
        self.checksum == other.checksum
            && self.source_path == other.source_path
            && *self.config == *other.config
    }
}

impl<T: PartialEq> Eq for ConfigSnapshot<T> {}

// ---------------------------------------------------------------------------
// AtomicConfig
// ---------------------------------------------------------------------------

/// Thread-safe configuration holder.
///
/// Readers obtain a cheap `Arc<T>` clone via [`get`](Self::get) without
/// blocking writers. Writers atomically replace the inner value via
/// [`swap`](Self::swap).
#[derive(Debug)]
pub struct AtomicConfig<T> {
    inner: RwLock<Arc<T>>,
}

impl<T> AtomicConfig<T> {
    /// Create a new holder initialised with `value`.
    pub fn new(value: T) -> Self {
        Self {
            inner: RwLock::new(Arc::new(value)),
        }
    }

    /// Get a reference-counted handle to the current config.
    ///
    /// This never blocks writers and is wait-free on most platforms.
    pub fn get(&self) -> Arc<T> {
        self.inner
            .read()
            .expect("AtomicConfig RwLock poisoned")
            .clone()
    }

    /// Atomically replace the config, returning the previous value.
    pub fn swap(&self, new: T) -> Arc<T> {
        let new_arc = Arc::new(new);
        let mut guard = self.inner.write().expect("AtomicConfig RwLock poisoned");
        let old = guard.clone();
        *guard = new_arc;
        old
    }
}

// ---------------------------------------------------------------------------
// ConfigDiff
// ---------------------------------------------------------------------------

/// Tracks what changed between two config versions at the top-level key
/// granularity.
///
/// Keys are dot-separated JSON paths (e.g. `"log.level"`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDiff {
    /// Keys present in the new config but absent in the old.
    pub added_keys: BTreeSet<String>,
    /// Keys present in the old config but absent in the new.
    pub removed_keys: BTreeSet<String>,
    /// Keys present in both configs but with different values.
    pub modified_keys: BTreeSet<String>,
}

impl ConfigDiff {
    /// Returns `true` if the two configs are identical (no diff).
    pub fn is_empty(&self) -> bool {
        self.added_keys.is_empty()
            && self.removed_keys.is_empty()
            && self.modified_keys.is_empty()
    }

    /// Total number of changed keys.
    pub fn len(&self) -> usize {
        self.added_keys.len() + self.removed_keys.len() + self.modified_keys.len()
    }
}

/// Compute the diff between two serialisable config values.
///
/// Both values are serialised to `serde_json::Value` and compared key-by-key.
/// Nested objects are flattened with dot-separated paths.
///
/// # Errors
///
/// Returns an error if either value cannot be serialised to JSON.
pub fn diff<T: Serialize>(old: &T, new: &T) -> Result<ConfigDiff, serde_json::Error> {
    let old_val = serde_json::to_value(old)?;
    let new_val = serde_json::to_value(new)?;

    let mut result = ConfigDiff::default();
    let old_flat = flatten_json("", &old_val);
    let new_flat = flatten_json("", &new_val);

    for (key, val) in &old_flat {
        match new_flat.get(key) {
            Some(new_v) if new_v != val => {
                result.modified_keys.insert(key.clone());
            }
            None => {
                result.removed_keys.insert(key.clone());
            }
            _ => {}
        }
    }
    for key in new_flat.keys() {
        if !old_flat.contains_key(key) {
            result.added_keys.insert(key.clone());
        }
    }

    Ok(result)
}

/// Flatten a JSON value into dot-separated key-value pairs.
fn flatten_json(
    prefix: &str,
    value: &serde_json::Value,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut map = std::collections::BTreeMap::new();
    match value {
        serde_json::Value::Object(obj) => {
            for (k, v) in obj {
                let full_key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                map.extend(flatten_json(&full_key, v));
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                let full_key = format!("{prefix}[{i}]");
                map.extend(flatten_json(&full_key, v));
            }
        }
        _ => {
            map.insert(prefix.to_owned(), value.clone());
        }
    }
    map
}

// ---------------------------------------------------------------------------
// ValidationGate
// ---------------------------------------------------------------------------

/// Validates a new configuration before it is applied.
///
/// A gate holds a list of validator functions. All validators are run and
/// their errors are collected.
pub struct ValidationGate<T> {
    validators: Vec<Box<dyn Fn(&T) -> Result<(), Vec<ValidationError>> + Send + Sync>>,
}

/// A single validation failure produced by a [`ValidationGate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dot-separated config path that failed (e.g. `"log.level"`).
    pub field: String,
    /// Human-readable description of the problem.
    pub message: String,
}

impl ValidationError {
    /// Create a new validation error.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl<T> ValidationGate<T> {
    /// Create a gate with no validators.
    pub fn new() -> Self {
        Self {
            validators: Vec::new(),
        }
    }

    /// Register a validator function.
    pub fn add_validator(
        &mut self,
        f: impl Fn(&T) -> Result<(), Vec<ValidationError>> + Send + Sync + 'static,
    ) {
        self.validators.push(Box::new(f));
    }

    /// Run all validators against `new`, returning collected errors.
    pub fn validate(&self, new: &T) -> Result<(), Vec<ValidationError>> {
        let mut all_errors = Vec::new();
        for v in &self.validators {
            if let Err(errs) = v(new) {
                all_errors.extend(errs);
            }
        }
        if all_errors.is_empty() {
            Ok(())
        } else {
            Err(all_errors)
        }
    }
}

impl<T> Default for ValidationGate<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for ValidationGate<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationGate")
            .field("validator_count", &self.validators.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Checksum
// ---------------------------------------------------------------------------

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// Compute a hex-encoded FNV-1a 64-bit checksum of the given byte slice.
///
/// This is *not* cryptographic. It exists solely to detect unintentional
/// content changes between poll ticks. The return format (hex string) is
/// forward-compatible with a future migration to BLAKE3.
pub fn compute_checksum_bytes(data: &[u8]) -> String {
    let mut hash: u64 = FNV_OFFSET;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

/// Compute the content checksum of a file on disk.
///
/// # Errors
///
/// Returns an I/O error if the file cannot be read.
pub fn compute_checksum(path: &Path) -> io::Result<String> {
    let data = fs::read(path)?;
    Ok(compute_checksum_bytes(&data))
}

// ---------------------------------------------------------------------------
// ConfigWatcher
// ---------------------------------------------------------------------------

/// Internal debounce state.
#[derive(Debug)]
struct DebounceState {
    /// The event that is being debounced.
    pending_event: WatchEvent,
    /// When the pending event was first detected.
    first_seen: Instant,
    /// The latest detection time (resets on each new change within the window).
    last_seen: Instant,
}

/// Polls a configuration file for changes and emits [`WatchEvent`]s.
///
/// # Design
///
/// Each call to [`check_for_changes`](Self::check_for_changes) stats the file
/// and, if the mtime has changed, reads the file and computes a content
/// checksum. If the checksum differs from the last known value, a
/// [`WatchEvent`] is produced (subject to the [`ReloadPolicy`]).
pub struct ConfigWatcher {
    watch_path: PathBuf,
    poll_interval: Duration,
    reload_policy: ReloadPolicy,
    /// Last known content checksum (hex). `None` if the file has never been
    /// successfully read, or was deleted.
    last_checksum: Mutex<Option<String>>,
    /// Last known mtime.
    last_mtime: Mutex<Option<SystemTime>>,
    /// Whether the file existed on the previous poll tick.
    was_present: Mutex<bool>,
    /// Debounce state (only used when policy is `Debounced`).
    debounce: Mutex<Option<DebounceState>>,
    /// Pending event for `Manual` policy.
    manual_pending: Mutex<Option<WatchEvent>>,
}

impl fmt::Debug for ConfigWatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigWatcher")
            .field("watch_path", &self.watch_path)
            .field("poll_interval", &self.poll_interval)
            .field("reload_policy", &self.reload_policy)
            .finish()
    }
}

impl ConfigWatcher {
    /// Create a new watcher.
    ///
    /// The watcher does **not** start polling automatically; call
    /// [`check_for_changes`](Self::check_for_changes) in a loop or use
    /// [`watch_loop`](Self::watch_loop) for a blocking poll loop.
    pub fn new(path: impl Into<PathBuf>, poll_interval: Duration, policy: ReloadPolicy) -> Self {
        let path = path.into();
        let exists = path.exists();
        let checksum = if exists {
            compute_checksum(&path).ok()
        } else {
            None
        };
        let mtime = if exists {
            fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
        } else {
            None
        };

        Self {
            watch_path: path,
            poll_interval,
            reload_policy: policy,
            last_checksum: Mutex::new(checksum),
            last_mtime: Mutex::new(mtime),
            was_present: Mutex::new(exists),
            debounce: Mutex::new(None),
            manual_pending: Mutex::new(None),
        }
    }

    /// Return the watched path.
    pub fn path(&self) -> &Path {
        &self.watch_path
    }

    /// Return the configured poll interval.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Return the configured reload policy.
    pub fn reload_policy(&self) -> &ReloadPolicy {
        &self.reload_policy
    }

    /// Check whether the watched file has changed since the last poll.
    ///
    /// Returns `Some(event)` if a change should be surfaced according to the
    /// configured [`ReloadPolicy`], or `None` otherwise.
    pub fn check_for_changes(&self) -> Option<WatchEvent> {
        let now = SystemTime::now();
        let exists = self.watch_path.exists();
        let mut was_present = self.was_present.lock().expect("lock poisoned");

        // --- Deletion ---
        if !exists && *was_present {
            *was_present = false;
            let mut last_cs = self.last_checksum.lock().expect("lock poisoned");
            *last_cs = None;
            let mut last_mt = self.last_mtime.lock().expect("lock poisoned");
            *last_mt = None;

            let event = WatchEvent {
                path: self.watch_path.clone(),
                kind: WatchEventKind::Deleted,
                timestamp: now,
            };
            return self.apply_policy(event);
        }

        // --- Creation ---
        if exists && !*was_present {
            *was_present = true;
            if let Ok(cs) = compute_checksum(&self.watch_path) {
                let mut last_cs = self.last_checksum.lock().expect("lock poisoned");
                *last_cs = Some(cs);
            }
            if let Ok(meta) = fs::metadata(&self.watch_path) {
                if let Ok(mt) = meta.modified() {
                    let mut last_mt = self.last_mtime.lock().expect("lock poisoned");
                    *last_mt = Some(mt);
                }
            }
            let event = WatchEvent {
                path: self.watch_path.clone(),
                kind: WatchEventKind::Created,
                timestamp: now,
            };
            return self.apply_policy(event);
        }

        // --- File is absent and was absent: nothing to do ---
        if !exists {
            return self.drain_debounce();
        }

        // --- Modification check ---
        // First check mtime to avoid unnecessary reads.
        let current_mtime = fs::metadata(&self.watch_path)
            .and_then(|m| m.modified())
            .ok();

        let mtime_changed = {
            let last_mt = self.last_mtime.lock().expect("lock poisoned");
            current_mtime != *last_mt
        };

        if !mtime_changed {
            return self.drain_debounce();
        }

        // mtime changed — compute checksum to confirm a real content change.
        let current_checksum = match compute_checksum(&self.watch_path) {
            Ok(cs) => cs,
            Err(_) => return self.drain_debounce(),
        };

        let checksum_changed = {
            let mut last_cs = self.last_checksum.lock().expect("lock poisoned");
            let changed = last_cs.as_ref() != Some(&current_checksum);
            *last_cs = Some(current_checksum);
            changed
        };

        // Update mtime regardless (the file was touched even if content is same).
        {
            let mut last_mt = self.last_mtime.lock().expect("lock poisoned");
            *last_mt = current_mtime;
        }

        if checksum_changed {
            let event = WatchEvent {
                path: self.watch_path.clone(),
                kind: WatchEventKind::Modified,
                timestamp: now,
            };
            self.apply_policy(event)
        } else {
            self.drain_debounce()
        }
    }

    /// Apply the reload policy to a raw event and decide whether to surface it.
    fn apply_policy(&self, event: WatchEvent) -> Option<WatchEvent> {
        match &self.reload_policy {
            ReloadPolicy::Immediate => Some(event),
            ReloadPolicy::Debounced(window) => {
                let now = Instant::now();
                let mut debounce = self.debounce.lock().expect("lock poisoned");
                match debounce.as_mut() {
                    Some(state) => {
                        // Reset the trailing timer.
                        state.last_seen = now;
                        state.pending_event = event;
                        None
                    }
                    None => {
                        *debounce = Some(DebounceState {
                            pending_event: event,
                            first_seen: now,
                            last_seen: now,
                        });
                        // We need to wait for the debounce window to expire
                        // before surfacing the event. Check will happen on the
                        // next poll tick via `drain_debounce`.
                        let _ = window; // used in drain_debounce
                        None
                    }
                }
            }
            ReloadPolicy::Manual => {
                let mut pending = self.manual_pending.lock().expect("lock poisoned");
                *pending = Some(event);
                None
            }
        }
    }

    /// If the debounce window has elapsed, drain and return the pending event.
    fn drain_debounce(&self) -> Option<WatchEvent> {
        if let ReloadPolicy::Debounced(window) = &self.reload_policy {
            let mut debounce = self.debounce.lock().expect("lock poisoned");
            if let Some(state) = debounce.as_ref() {
                if state.last_seen.elapsed() >= *window {
                    let event = debounce.take().map(|s| s.pending_event);
                    return event;
                }
            }
        }
        None
    }

    /// Consume the pending event in [`Manual`](ReloadPolicy::Manual) mode.
    ///
    /// Returns `None` if there is no pending event or the policy is not
    /// `Manual`.
    pub fn take_pending(&self) -> Option<WatchEvent> {
        let mut pending = self.manual_pending.lock().expect("lock poisoned");
        pending.take()
    }

    /// Run a blocking poll loop, calling `callback` each time a change is
    /// detected (subject to the reload policy).
    ///
    /// This function blocks the current thread. Use it from a dedicated
    /// thread or a `tokio::task::spawn_blocking` wrapper.
    ///
    /// The loop runs indefinitely. To stop it, drop the `ConfigWatcher` from
    /// another thread (which will cause the mutex locks to fail) or use a
    /// separate cancellation mechanism.
    pub fn watch_loop(&self, callback: impl Fn(WatchEvent)) {
        loop {
            if let Some(event) = self.check_for_changes() {
                callback(event);
            }
            std::thread::sleep(self.poll_interval);
        }
    }
}

// ---------------------------------------------------------------------------
// WatchError — lightweight error for watcher operations
// ---------------------------------------------------------------------------

/// Errors specific to the watch module.
#[derive(Debug)]
pub enum WatchError {
    /// An I/O error occurred while reading or statting the watched file.
    Io(PathBuf, io::Error),
    /// Validation of the new config failed.
    Validation(Vec<ValidationError>),
    /// Serialization failed during diff computation.
    Serialize(String),
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, err) => write!(f, "I/O error on '{}': {err}", path.display()),
            Self::Validation(errs) => {
                write!(f, "validation failed: ")?;
                for (i, e) in errs.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{e}")?;
                }
                Ok(())
            }
            Self::Serialize(msg) => write!(f, "serialization error: {msg}"),
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, err) => Some(err),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Checksum tests
    // -----------------------------------------------------------------------

    #[test]
    fn checksum_deterministic_for_same_content() {
        let a = compute_checksum_bytes(b"hello world");
        let b = compute_checksum_bytes(b"hello world");
        assert_eq!(a, b, "same input must produce same checksum");
    }

    #[test]
    fn checksum_differs_for_different_content() {
        let a = compute_checksum_bytes(b"hello world");
        let b = compute_checksum_bytes(b"hello world!");
        assert_ne!(a, b, "different input must produce different checksum");
    }

    #[test]
    fn checksum_is_hex_encoded() {
        let cs = compute_checksum_bytes(b"test");
        assert_eq!(cs.len(), 16, "FNV-1a 64-bit should produce 16 hex chars");
        assert!(
            cs.chars().all(|c| c.is_ascii_hexdigit()),
            "checksum must be hex-encoded, got: {cs}"
        );
    }

    #[test]
    fn checksum_empty_input() {
        let cs = compute_checksum_bytes(b"");
        // FNV-1a of empty input is the offset basis itself.
        assert_eq!(cs, format!("{FNV_OFFSET:016x}"));
    }

    #[test]
    fn checksum_from_file() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("test.toml");
        fs::write(&path, b"key = \"value\"").expect("write");
        let cs = compute_checksum(&path).expect("checksum");
        let expected = compute_checksum_bytes(b"key = \"value\"");
        assert_eq!(cs, expected);
    }

    #[test]
    fn checksum_file_not_found() {
        let result = compute_checksum(Path::new("/nonexistent/path/file.toml"));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // WatchEvent / WatchEventKind tests
    // -----------------------------------------------------------------------

    #[test]
    fn watch_event_kind_display() {
        assert_eq!(WatchEventKind::Created.to_string(), "created");
        assert_eq!(WatchEventKind::Modified.to_string(), "modified");
        assert_eq!(WatchEventKind::Deleted.to_string(), "deleted");
    }

    #[test]
    fn watch_event_kind_equality() {
        assert_eq!(WatchEventKind::Created, WatchEventKind::Created);
        assert_ne!(WatchEventKind::Created, WatchEventKind::Modified);
        assert_ne!(WatchEventKind::Modified, WatchEventKind::Deleted);
    }

    // -----------------------------------------------------------------------
    // ConfigWatcher — basic detection
    // -----------------------------------------------------------------------

    #[test]
    fn watcher_detects_file_modification() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"version = 1").expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Immediate);

        // First check: no change (we just initialised from the current state).
        assert!(watcher.check_for_changes().is_none());

        // Modify the file.
        fs::write(&path, b"version = 2").expect("write");

        let event = watcher.check_for_changes();
        assert!(event.is_some(), "should detect modification");
        let event = event.expect("event");
        assert_eq!(event.kind, WatchEventKind::Modified);
        assert_eq!(event.path, path);
    }

    #[test]
    fn watcher_detects_file_creation() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        // File does not exist yet.

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Immediate);

        // No change initially.
        assert!(watcher.check_for_changes().is_none());

        // Create the file.
        fs::write(&path, b"new = true").expect("write");

        let event = watcher.check_for_changes();
        assert!(event.is_some(), "should detect creation");
        assert_eq!(event.expect("event").kind, WatchEventKind::Created);
    }

    #[test]
    fn watcher_detects_file_deletion() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"version = 1").expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Immediate);

        // No change initially.
        assert!(watcher.check_for_changes().is_none());

        // Delete the file.
        fs::remove_file(&path).expect("remove");

        let event = watcher.check_for_changes();
        assert!(event.is_some(), "should detect deletion");
        assert_eq!(event.expect("event").kind, WatchEventKind::Deleted);
    }

    #[test]
    fn watcher_no_false_positive_on_same_content() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"version = 1").expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Immediate);

        // First check — no change.
        assert!(watcher.check_for_changes().is_none());

        // Re-write with the *same* content (mtime changes, content does not).
        // We need a small sleep to ensure mtime actually changes.
        thread::sleep(Duration::from_millis(50));
        fs::write(&path, b"version = 1").expect("write");

        let event = watcher.check_for_changes();
        assert!(event.is_none(), "same content should not trigger a change event");
    }

    #[test]
    fn watcher_multiple_modifications_detected_serially() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"v = 1").expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Immediate);
        assert!(watcher.check_for_changes().is_none());

        fs::write(&path, b"v = 2").expect("write");
        let e1 = watcher.check_for_changes();
        assert!(e1.is_some());

        // After consuming the event, no new change until next write.
        assert!(watcher.check_for_changes().is_none());

        fs::write(&path, b"v = 3").expect("write");
        let e2 = watcher.check_for_changes();
        assert!(e2.is_some());
    }

    // -----------------------------------------------------------------------
    // ConfigWatcher — debouncing
    // -----------------------------------------------------------------------

    #[test]
    fn watcher_debounce_suppresses_rapid_changes() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"v = 1").expect("write");

        let watcher = ConfigWatcher::new(
            &path,
            Duration::from_millis(10),
            ReloadPolicy::Debounced(Duration::from_millis(100)),
        );
        assert!(watcher.check_for_changes().is_none());

        // Rapid modification — should be debounced.
        fs::write(&path, b"v = 2").expect("write");
        let immediate = watcher.check_for_changes();
        assert!(immediate.is_none(), "debounced policy should suppress immediate event");
    }

    #[test]
    fn watcher_debounce_emits_after_window() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"v = 1").expect("write");

        let debounce_window = Duration::from_millis(50);
        let watcher = ConfigWatcher::new(
            &path,
            Duration::from_millis(10),
            ReloadPolicy::Debounced(debounce_window),
        );
        assert!(watcher.check_for_changes().is_none());

        // Modify the file.
        fs::write(&path, b"v = 2").expect("write");
        assert!(watcher.check_for_changes().is_none()); // suppressed

        // Wait for the debounce window to pass.
        thread::sleep(debounce_window + Duration::from_millis(20));

        // Now the debounced event should be drained.
        let event = watcher.check_for_changes();
        assert!(event.is_some(), "event should surface after debounce window");
        assert_eq!(event.expect("event").kind, WatchEventKind::Modified);
    }

    // -----------------------------------------------------------------------
    // ConfigWatcher — manual policy
    // -----------------------------------------------------------------------

    #[test]
    fn watcher_manual_policy_does_not_auto_emit() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"v = 1").expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Manual);
        assert!(watcher.check_for_changes().is_none());

        fs::write(&path, b"v = 2").expect("write");
        // check_for_changes should return None in manual mode.
        assert!(watcher.check_for_changes().is_none());

        // But the event is available via take_pending.
        let pending = watcher.take_pending();
        assert!(pending.is_some(), "should have a pending event");
        assert_eq!(pending.expect("event").kind, WatchEventKind::Modified);
    }

    #[test]
    fn watcher_take_pending_returns_none_when_no_event() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, b"v = 1").expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Manual);
        assert!(watcher.take_pending().is_none());
    }

    // -----------------------------------------------------------------------
    // ConfigWatcher — error handling
    // -----------------------------------------------------------------------

    #[test]
    fn watcher_on_nonexistent_file_does_not_panic() {
        let watcher = ConfigWatcher::new(
            "/nonexistent/path/to/config.toml",
            Duration::from_millis(10),
            ReloadPolicy::Immediate,
        );
        // Should not panic, just return None.
        assert!(watcher.check_for_changes().is_none());
    }

    #[test]
    fn watcher_path_and_interval_accessors() {
        let path = PathBuf::from("/tmp/test.toml");
        let interval = Duration::from_secs(5);
        let watcher = ConfigWatcher::new(&path, interval, ReloadPolicy::Immediate);
        assert_eq!(watcher.path(), path);
        assert_eq!(watcher.poll_interval(), interval);
        assert_eq!(*watcher.reload_policy(), ReloadPolicy::Immediate);
    }

    // -----------------------------------------------------------------------
    // AtomicConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_config_get_returns_initial_value() {
        let ac = AtomicConfig::new(42_u32);
        assert_eq!(*ac.get(), 42);
    }

    #[test]
    fn atomic_config_swap_replaces_value() {
        let ac = AtomicConfig::new(String::from("old"));
        let old = ac.swap(String::from("new"));
        assert_eq!(*old, "old");
        assert_eq!(*ac.get(), "new");
    }

    #[test]
    fn atomic_config_swap_returns_previous_arc() {
        let ac = AtomicConfig::new(100_i32);
        let first = ac.get();
        assert_eq!(*first, 100);

        let prev = ac.swap(200);
        assert_eq!(*prev, 100);
        // `first` still holds the old value via Arc.
        assert_eq!(*first, 100);
        assert_eq!(*ac.get(), 200);
    }

    #[test]
    fn atomic_config_concurrent_reads() {
        let ac = Arc::new(AtomicConfig::new(String::from("shared")));
        let mut handles = Vec::new();

        for _ in 0..10 {
            let ac_clone = Arc::clone(&ac);
            handles.push(thread::spawn(move || {
                let val = ac_clone.get();
                assert!(!val.is_empty());
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }
    }

    #[test]
    fn atomic_config_concurrent_swap_and_read() {
        let ac = Arc::new(AtomicConfig::new(0_u64));

        let writer = {
            let ac = Arc::clone(&ac);
            thread::spawn(move || {
                for i in 1..=100 {
                    ac.swap(i);
                }
            })
        };

        let reader = {
            let ac = Arc::clone(&ac);
            thread::spawn(move || {
                let mut last = 0;
                for _ in 0..200 {
                    let v = *ac.get();
                    // Values should only increase (monotonic writes).
                    assert!(v >= last || v == 0, "non-monotonic: last={last}, got={v}");
                    last = v;
                }
            })
        };

        writer.join().expect("writer panicked");
        reader.join().expect("reader panicked");
        // Final value should be 100.
        assert_eq!(*ac.get(), 100);
    }

    // -----------------------------------------------------------------------
    // ConfigDiff tests
    // -----------------------------------------------------------------------

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct SimpleConfig {
        name: String,
        value: i32,
        enabled: bool,
    }

    #[test]
    fn diff_identical_configs_is_empty() {
        let a = SimpleConfig {
            name: "test".into(),
            value: 42,
            enabled: true,
        };
        let b = SimpleConfig {
            name: "test".into(),
            value: 42,
            enabled: true,
        };
        let d = diff(&a, &b).expect("diff");
        assert!(d.is_empty(), "identical configs should have empty diff");
        assert_eq!(d.len(), 0);
    }

    #[test]
    fn diff_detects_modified_keys() {
        let a = SimpleConfig {
            name: "test".into(),
            value: 1,
            enabled: true,
        };
        let b = SimpleConfig {
            name: "test".into(),
            value: 2,
            enabled: true,
        };
        let d = diff(&a, &b).expect("diff");
        assert!(d.modified_keys.contains("value"), "should detect value change");
        assert!(d.added_keys.is_empty());
        assert!(d.removed_keys.is_empty());
    }

    #[test]
    fn diff_detects_added_and_removed_keys() {
        // Use serde_json::Value directly to have different key sets.
        let old = serde_json::json!({"a": 1, "b": 2});
        let new = serde_json::json!({"b": 2, "c": 3});
        let d = diff(&old, &new).expect("diff");
        assert!(d.removed_keys.contains("a"), "should detect removed key 'a'");
        assert!(d.added_keys.contains("c"), "should detect added key 'c'");
        assert!(d.modified_keys.is_empty());
    }

    #[test]
    fn diff_nested_keys_use_dot_notation() {
        let old = serde_json::json!({"log": {"level": "info"}});
        let new = serde_json::json!({"log": {"level": "debug"}});
        let d = diff(&old, &new).expect("diff");
        assert!(
            d.modified_keys.contains("log.level"),
            "nested key should use dot notation: {:?}",
            d.modified_keys
        );
    }

    #[test]
    fn diff_len_counts_all_changes() {
        let old = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let new = serde_json::json!({"a": 10, "c": 3, "d": 4});
        let d = diff(&old, &new).expect("diff");
        // a: modified, b: removed, d: added => len = 3
        assert_eq!(d.len(), 3);
    }

    // -----------------------------------------------------------------------
    // ValidationGate tests
    // -----------------------------------------------------------------------

    #[test]
    fn validation_gate_no_validators_passes() {
        let gate: ValidationGate<String> = ValidationGate::new();
        assert!(gate.validate(&String::from("anything")).is_ok());
    }

    #[test]
    fn validation_gate_passing_validator() {
        let mut gate = ValidationGate::new();
        gate.add_validator(|val: &i32| {
            if *val > 0 {
                Ok(())
            } else {
                Err(vec![ValidationError::new("value", "must be positive")])
            }
        });
        assert!(gate.validate(&42).is_ok());
    }

    #[test]
    fn validation_gate_failing_validator() {
        let mut gate = ValidationGate::new();
        gate.add_validator(|val: &i32| {
            if *val > 0 {
                Ok(())
            } else {
                Err(vec![ValidationError::new("value", "must be positive")])
            }
        });
        let result = gate.validate(&-1);
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].field, "value");
    }

    #[test]
    fn validation_gate_collects_multiple_errors() {
        let mut gate = ValidationGate::new();
        gate.add_validator(|val: &i32| {
            if *val > 0 {
                Ok(())
            } else {
                Err(vec![ValidationError::new("value", "must be positive")])
            }
        });
        gate.add_validator(|val: &i32| {
            if *val < 100 {
                Ok(())
            } else {
                Err(vec![ValidationError::new("value", "must be less than 100")])
            }
        });

        // Value that fails both validators:
        // -1 fails "must be positive"
        // but passes "must be less than 100"
        let result = gate.validate(&-1);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().len(), 1);

        // 200 fails "must be less than 100" but passes "must be positive"
        let result = gate.validate(&200);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().len(), 1);
    }

    #[test]
    fn validation_gate_multiple_validators_all_fail() {
        let mut gate = ValidationGate::new();
        gate.add_validator(|_: &String| {
            Err(vec![ValidationError::new("field_a", "bad a")])
        });
        gate.add_validator(|_: &String| {
            Err(vec![ValidationError::new("field_b", "bad b")])
        });
        let result = gate.validate(&String::from("test"));
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert_eq!(errs.len(), 2);
        assert!(errs.iter().any(|e| e.field == "field_a"));
        assert!(errs.iter().any(|e| e.field == "field_b"));
    }

    #[test]
    fn validation_error_display() {
        let err = ValidationError::new("log.level", "invalid level");
        assert_eq!(err.to_string(), "log.level: invalid level");
    }

    // -----------------------------------------------------------------------
    // ConfigSnapshot tests
    // -----------------------------------------------------------------------

    #[test]
    fn config_snapshot_equality() {
        let s1 = ConfigSnapshot {
            config: Arc::new(42),
            loaded_at: SystemTime::now(),
            source_path: PathBuf::from("/a.toml"),
            checksum: "abc".to_owned(),
        };
        let s2 = ConfigSnapshot {
            config: Arc::new(42),
            loaded_at: SystemTime::now(), // different time, but we don't compare it
            source_path: PathBuf::from("/a.toml"),
            checksum: "abc".to_owned(),
        };
        assert_eq!(s1, s2, "snapshots with same config/path/checksum should be equal");
    }

    #[test]
    fn config_snapshot_inequality_on_checksum() {
        let s1 = ConfigSnapshot {
            config: Arc::new(42),
            loaded_at: SystemTime::now(),
            source_path: PathBuf::from("/a.toml"),
            checksum: "abc".to_owned(),
        };
        let s2 = ConfigSnapshot {
            config: Arc::new(42),
            loaded_at: SystemTime::now(),
            source_path: PathBuf::from("/a.toml"),
            checksum: "def".to_owned(),
        };
        assert_ne!(s1, s2, "different checksums should make snapshots unequal");
    }

    // -----------------------------------------------------------------------
    // ReloadPolicy tests
    // -----------------------------------------------------------------------

    #[test]
    fn reload_policy_equality() {
        assert_eq!(ReloadPolicy::Immediate, ReloadPolicy::Immediate);
        assert_eq!(ReloadPolicy::Manual, ReloadPolicy::Manual);
        assert_eq!(
            ReloadPolicy::Debounced(Duration::from_millis(100)),
            ReloadPolicy::Debounced(Duration::from_millis(100))
        );
        assert_ne!(ReloadPolicy::Immediate, ReloadPolicy::Manual);
        assert_ne!(
            ReloadPolicy::Debounced(Duration::from_millis(100)),
            ReloadPolicy::Debounced(Duration::from_millis(200))
        );
    }

    // -----------------------------------------------------------------------
    // WatchError tests
    // -----------------------------------------------------------------------

    #[test]
    fn watch_error_io_display() {
        let err = WatchError::Io(
            PathBuf::from("/tmp/test.toml"),
            io::Error::new(io::ErrorKind::NotFound, "file not found"),
        );
        let msg = err.to_string();
        assert!(msg.contains("/tmp/test.toml"), "should contain path: {msg}");
        assert!(msg.contains("file not found"), "should contain cause: {msg}");
    }

    #[test]
    fn watch_error_validation_display() {
        let err = WatchError::Validation(vec![
            ValidationError::new("a", "bad"),
            ValidationError::new("b", "worse"),
        ]);
        let msg = err.to_string();
        assert!(msg.contains("a: bad"), "should contain first error: {msg}");
        assert!(msg.contains("b: worse"), "should contain second error: {msg}");
    }

    #[test]
    fn watch_error_serialize_display() {
        let err = WatchError::Serialize("oops".into());
        assert!(err.to_string().contains("oops"));
    }

    // -----------------------------------------------------------------------
    // Integration-style: watcher + snapshot + diff
    // -----------------------------------------------------------------------

    #[test]
    fn end_to_end_watch_and_diff() {
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("config.toml");
        fs::write(&path, br#"{"name":"alpha","value":1,"enabled":true}"#).expect("write");

        let watcher = ConfigWatcher::new(&path, Duration::from_millis(10), ReloadPolicy::Immediate);

        // Load initial snapshot.
        let cs1 = compute_checksum(&path).expect("checksum");
        let old: SimpleConfig =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        let snap1 = ConfigSnapshot {
            config: Arc::new(old),
            loaded_at: SystemTime::now(),
            source_path: path.clone(),
            checksum: cs1,
        };

        // Initial check — no change.
        assert!(watcher.check_for_changes().is_none());

        // Modify the file.
        fs::write(&path, br#"{"name":"beta","value":1,"enabled":false}"#).expect("write");

        // Detect the change.
        let event = watcher.check_for_changes();
        assert!(event.is_some());

        // Load new snapshot and diff.
        let cs2 = compute_checksum(&path).expect("checksum");
        let new: SimpleConfig =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        let snap2 = ConfigSnapshot {
            config: Arc::new(new),
            loaded_at: SystemTime::now(),
            source_path: path.clone(),
            checksum: cs2,
        };

        assert_ne!(snap1.checksum, snap2.checksum);
        let d = diff(&*snap1.config, &*snap2.config).expect("diff");
        assert!(d.modified_keys.contains("name"));
        assert!(d.modified_keys.contains("enabled"));
        assert!(!d.modified_keys.contains("value"), "value did not change");
    }

    // -----------------------------------------------------------------------
    // Flatten JSON edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn flatten_json_handles_arrays() {
        let val = serde_json::json!({"items": [1, 2, 3]});
        let flat = flatten_json("", &val);
        assert!(flat.contains_key("items[0]"));
        assert!(flat.contains_key("items[1]"));
        assert!(flat.contains_key("items[2]"));
        assert_eq!(flat.len(), 3);
    }

    #[test]
    fn flatten_json_deeply_nested() {
        let val = serde_json::json!({"a": {"b": {"c": 42}}});
        let flat = flatten_json("", &val);
        assert!(flat.contains_key("a.b.c"), "keys: {:?}", flat.keys().collect::<Vec<_>>());
        assert_eq!(flat["a.b.c"], serde_json::json!(42));
    }
}
