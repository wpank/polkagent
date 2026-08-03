//! Prometheus-compatible metrics exposition for the Polkagent platform.
//!
//! This module provides a self-contained metrics registry that renders
//! metrics in the [Prometheus text exposition format][prom-fmt]. It is
//! designed to be used alongside the existing [`MetricRecorder`] tracing-
//! based metrics, giving operators a familiar `/metrics` endpoint they
//! can scrape with any Prometheus-compatible collector.
//!
//! # Thread safety
//!
//! Every metric primitive uses `parking_lot::RwLock` for interior
//! mutability, making the registry safe to share across threads
//! without external synchronisation.
//!
//! [prom-fmt]: https://prometheus.io/docs/instrumenting/exposition_formats/

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use parking_lot::RwLock;

// ---------------------------------------------------------------------------
// Label
// ---------------------------------------------------------------------------

/// A single Prometheus label (key-value pair).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Label {
    /// Label name (e.g. `"agent"`).
    pub name: String,
    /// Label value (e.g. `"alpha"`).
    pub value: String,
}

impl Label {
    /// Create a new label.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Escape backslashes, double-quotes, and newlines in the value.
        let escaped = self
            .value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        write!(f, "{}=\"{}\"", self.name, escaped)
    }
}

/// Convenience: format a slice of labels as `{a="1",b="2"}` or empty string.
fn format_labels(labels: &[Label]) -> String {
    if labels.is_empty() {
        String::new()
    } else {
        let inner: Vec<String> = labels.iter().map(ToString::to_string).collect();
        format!("{{{}}}", inner.join(","))
    }
}

// ---------------------------------------------------------------------------
// MetricValue
// ---------------------------------------------------------------------------

/// The value carried by a single time-series.
#[derive(Debug, Clone)]
pub enum MetricValue {
    /// Monotonically increasing counter.
    Counter(f64),
    /// Gauge that can go up and down.
    Gauge(f64),
    /// Distribution of observed values.
    Histogram {
        /// Total number of observations.
        count: u64,
        /// Sum of all observed values.
        sum: f64,
        /// Upper-bound / cumulative-count pairs.
        buckets: Vec<(f64, u64)>,
    },
}

// ---------------------------------------------------------------------------
// MetricType (for HELP / TYPE lines)
// ---------------------------------------------------------------------------

/// Prometheus metric type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    /// Counter.
    Counter,
    /// Gauge.
    Gauge,
    /// Histogram.
    Histogram,
}

impl fmt::Display for MetricType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Counter => write!(f, "counter"),
            Self::Gauge => write!(f, "gauge"),
            Self::Histogram => write!(f, "histogram"),
        }
    }
}

// ---------------------------------------------------------------------------
// Counter
// ---------------------------------------------------------------------------

/// A monotonically increasing counter with optional labels.
#[derive(Debug)]
pub struct Counter {
    /// Series keyed by sorted label set.
    series: RwLock<BTreeMap<Vec<Label>, f64>>,
}

impl Counter {
    /// Create a new counter.
    pub fn new() -> Self {
        Self {
            series: RwLock::new(BTreeMap::new()),
        }
    }

    /// Increment the counter for the given label set by `delta`.
    ///
    /// Negative deltas are silently ignored (counters only go up).
    pub fn increment(&self, labels: &[Label], delta: f64) {
        if delta < 0.0 {
            return;
        }
        let key = sorted_labels(labels);
        let mut map = self.series.write();
        let entry = map.entry(key).or_insert(0.0);
        *entry += delta;
    }

    /// Return the current value for the given label set, or `0.0`.
    pub fn get(&self, labels: &[Label]) -> f64 {
        let key = sorted_labels(labels);
        let map = self.series.read();
        map.get(&key).copied().unwrap_or(0.0)
    }

    /// Return all series.
    pub fn series(&self) -> BTreeMap<Vec<Label>, f64> {
        self.series.read().clone()
    }

    /// Reset all series to zero.
    pub fn reset(&self) {
        self.series.write().clear();
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Gauge
// ---------------------------------------------------------------------------

/// A gauge whose value can go up and down.
#[derive(Debug)]
pub struct Gauge {
    /// Series keyed by sorted label set.
    series: RwLock<BTreeMap<Vec<Label>, f64>>,
}

impl Gauge {
    /// Create a new gauge.
    pub fn new() -> Self {
        Self {
            series: RwLock::new(BTreeMap::new()),
        }
    }

    /// Set the gauge to an absolute value.
    pub fn set(&self, labels: &[Label], value: f64) {
        let key = sorted_labels(labels);
        let mut map = self.series.write();
        map.insert(key, value);
    }

    /// Increment the gauge by `delta`.
    pub fn inc(&self, labels: &[Label], delta: f64) {
        let key = sorted_labels(labels);
        let mut map = self.series.write();
        let entry = map.entry(key).or_insert(0.0);
        *entry += delta;
    }

    /// Decrement the gauge by `delta`.
    pub fn dec(&self, labels: &[Label], delta: f64) {
        let key = sorted_labels(labels);
        let mut map = self.series.write();
        let entry = map.entry(key).or_insert(0.0);
        *entry -= delta;
    }

    /// Return the current value for the given label set, or `0.0`.
    pub fn get(&self, labels: &[Label]) -> f64 {
        let key = sorted_labels(labels);
        let map = self.series.read();
        map.get(&key).copied().unwrap_or(0.0)
    }

    /// Return all series.
    pub fn series(&self) -> BTreeMap<Vec<Label>, f64> {
        self.series.read().clone()
    }

    /// Reset all series.
    pub fn reset(&self) {
        self.series.write().clear();
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Histogram
// ---------------------------------------------------------------------------

/// Internal state of one histogram series (one set of labels).
#[derive(Debug, Clone)]
struct HistogramSeries {
    count: u64,
    sum: f64,
    /// Upper-bound -> cumulative count.
    bucket_counts: Vec<(f64, u64)>,
}

/// A histogram that tracks the distribution of observed values.
#[derive(Debug)]
pub struct Histogram {
    /// Default upper bounds shared by every series.
    upper_bounds: Vec<f64>,
    /// Series keyed by sorted label set.
    series: RwLock<BTreeMap<Vec<Label>, HistogramSeries>>,
}

/// Default histogram buckets (similar to Prometheus client defaults).
pub const DEFAULT_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

impl Histogram {
    /// Create a histogram with the given bucket upper bounds.
    ///
    /// The bounds are sorted and de-duplicated internally. A `+Inf`
    /// bucket is always appended automatically.
    pub fn with_buckets(bounds: &[f64]) -> Self {
        let mut sorted: Vec<f64> = bounds.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        sorted.dedup();
        Self {
            upper_bounds: sorted,
            series: RwLock::new(BTreeMap::new()),
        }
    }

    /// Create a histogram with the [`DEFAULT_BUCKETS`].
    pub fn new() -> Self {
        Self::with_buckets(DEFAULT_BUCKETS)
    }

    /// Record an observation for the given label set.
    pub fn observe(&self, labels: &[Label], value: f64) {
        let key = sorted_labels(labels);
        let mut map = self.series.write();
        let entry = map.entry(key).or_insert_with(|| HistogramSeries {
            count: 0,
            sum: 0.0,
            bucket_counts: self.upper_bounds.iter().map(|&b| (b, 0u64)).collect(),
        });
        entry.count += 1;
        entry.sum += value;
        // Increment only the first (smallest) bucket that the value fits into.
        // The render method accumulates cumulative counts for Prometheus output.
        for bucket in &mut entry.bucket_counts {
            if value <= bucket.0 {
                bucket.1 += 1;
                break;
            }
        }
    }

    /// Return the count/sum for the given label set.
    pub fn get(&self, labels: &[Label]) -> (u64, f64) {
        let key = sorted_labels(labels);
        let map = self.series.read();
        map.get(&key)
            .map(|s| (s.count, s.sum))
            .unwrap_or((0, 0.0))
    }

    /// Return the bucket boundaries configured for this histogram.
    pub fn upper_bounds(&self) -> &[f64] {
        &self.upper_bounds
    }

    /// Return all series.
    pub fn all_series(&self) -> BTreeMap<Vec<Label>, (u64, f64, Vec<(f64, u64)>)> {
        let map = self.series.read();
        map.iter()
            .map(|(k, v)| (k.clone(), (v.count, v.sum, v.bucket_counts.clone())))
            .collect()
    }

    /// Reset all series.
    pub fn reset(&self) {
        self.series.write().clear();
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MetricFamily
// ---------------------------------------------------------------------------

/// A group of metrics sharing the same name/help/type but with different labels.
#[derive(Debug)]
pub struct MetricFamily {
    /// Metric name (e.g. `polkagent_runs_total`).
    pub name: String,
    /// HELP string.
    pub help: String,
    /// Metric type.
    pub metric_type: MetricType,
    /// The underlying metric value.
    pub inner: MetricInner,
}

/// Type-safe wrapper around the metric primitives.
#[derive(Debug)]
pub enum MetricInner {
    /// Counter family.
    Counter(Counter),
    /// Gauge family.
    Gauge(Gauge),
    /// Histogram family.
    Histogram(Histogram),
}

impl MetricFamily {
    /// Render this family in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!("# HELP {} {}\n", self.name, self.help));
        out.push_str(&format!("# TYPE {} {}\n", self.name, self.metric_type));

        match &self.inner {
            MetricInner::Counter(c) => {
                for (labels, value) in &c.series() {
                    out.push_str(&format!(
                        "{}{} {}\n",
                        self.name,
                        format_labels(labels),
                        format_f64(*value),
                    ));
                }
            }
            MetricInner::Gauge(g) => {
                for (labels, value) in &g.series() {
                    out.push_str(&format!(
                        "{}{} {}\n",
                        self.name,
                        format_labels(labels),
                        format_f64(*value),
                    ));
                }
            }
            MetricInner::Histogram(h) => {
                for (labels, (count, sum, buckets)) in &h.all_series() {
                    // Cumulative bucket lines.
                    let mut cumulative: u64 = 0;
                    for &(bound, raw_count) in buckets {
                        cumulative += raw_count;
                        let mut bucket_labels = labels.clone();
                        bucket_labels.push(Label::new("le", format_f64(bound)));
                        out.push_str(&format!(
                            "{}_bucket{} {}\n",
                            self.name,
                            format_labels(&bucket_labels),
                            cumulative,
                        ));
                    }
                    // +Inf bucket.
                    {
                        let mut inf_labels = labels.clone();
                        inf_labels.push(Label::new("le", "+Inf"));
                        out.push_str(&format!(
                            "{}_bucket{} {}\n",
                            self.name,
                            format_labels(&inf_labels),
                            count,
                        ));
                    }
                    out.push_str(&format!(
                        "{}_sum{} {}\n",
                        self.name,
                        format_labels(labels),
                        format_f64(*sum),
                    ));
                    out.push_str(&format!(
                        "{}_count{} {}\n",
                        self.name,
                        format_labels(labels),
                        count,
                    ));
                }
            }
        }

        out
    }
}

// ---------------------------------------------------------------------------
// PrometheusRegistry
// ---------------------------------------------------------------------------

/// A thread-safe registry of Prometheus metric families.
///
/// The registry owns the metric families and provides convenience
/// methods (`increment`, `set_gauge`, `observe`) that look up the
/// correct family by name and delegate to the underlying primitive.
#[derive(Debug, Clone)]
pub struct PrometheusRegistry {
    families: Arc<RwLock<Vec<MetricFamily>>>,
}

impl PrometheusRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            families: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a registry pre-loaded with the standard Polkagent metrics.
    pub fn with_default_metrics() -> Self {
        let reg = Self::new();

        // Counters
        reg.register_counter(
            "polkagent_runs_total",
            "Total number of agent runs",
        );
        reg.register_counter(
            "polkagent_effects_total",
            "Total number of effects executed",
        );
        reg.register_counter(
            "polkagent_model_tokens_total",
            "Total tokens consumed by model calls",
        );
        reg.register_counter(
            "polkagent_store_operations_total",
            "Total store operations",
        );

        // Histograms
        reg.register_histogram(
            "polkagent_run_duration_seconds",
            "Duration of agent runs in seconds",
            DEFAULT_BUCKETS,
        );
        reg.register_histogram(
            "polkagent_effect_latency_seconds",
            "Latency of individual effects in seconds",
            DEFAULT_BUCKETS,
        );
        reg.register_histogram(
            "polkagent_model_latency_seconds",
            "Model inference latency in seconds",
            DEFAULT_BUCKETS,
        );

        // Gauges
        reg.register_gauge("polkagent_active_runs", "Number of currently active runs");
        reg.register_gauge(
            "polkagent_memory_entries_total",
            "Number of memory entries per agent",
        );
        reg.register_gauge(
            "polkagent_approval_pending",
            "Number of effects awaiting operator approval",
        );

        reg
    }

    // -- Registration helpers -----------------------------------------------

    /// Register a counter family.
    pub fn register_counter(&self, name: &str, help: &str) {
        let family = MetricFamily {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Counter,
            inner: MetricInner::Counter(Counter::new()),
        };
        self.families.write().push(family);
    }

    /// Register a gauge family.
    pub fn register_gauge(&self, name: &str, help: &str) {
        let family = MetricFamily {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Gauge,
            inner: MetricInner::Gauge(Gauge::new()),
        };
        self.families.write().push(family);
    }

    /// Register a histogram family with custom bucket boundaries.
    pub fn register_histogram(&self, name: &str, help: &str, buckets: &[f64]) {
        let family = MetricFamily {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Histogram,
            inner: MetricInner::Histogram(Histogram::with_buckets(buckets)),
        };
        self.families.write().push(family);
    }

    // -- Mutation helpers ---------------------------------------------------

    /// Increment a counter by `delta` for the given label set.
    ///
    /// Returns `true` if the counter was found, `false` otherwise.
    pub fn increment(&self, name: &str, labels: &[Label], delta: f64) -> bool {
        let families = self.families.read();
        for family in families.iter() {
            if family.name == name {
                if let MetricInner::Counter(ref c) = family.inner {
                    c.increment(labels, delta);
                    return true;
                }
            }
        }
        false
    }

    /// Set a gauge to an absolute value.
    ///
    /// Returns `true` if the gauge was found, `false` otherwise.
    pub fn set_gauge(&self, name: &str, labels: &[Label], value: f64) -> bool {
        let families = self.families.read();
        for family in families.iter() {
            if family.name == name {
                if let MetricInner::Gauge(ref g) = family.inner {
                    g.set(labels, value);
                    return true;
                }
            }
        }
        false
    }

    /// Increment a gauge by `delta`.
    ///
    /// Returns `true` if the gauge was found, `false` otherwise.
    pub fn inc_gauge(&self, name: &str, labels: &[Label], delta: f64) -> bool {
        let families = self.families.read();
        for family in families.iter() {
            if family.name == name {
                if let MetricInner::Gauge(ref g) = family.inner {
                    g.inc(labels, delta);
                    return true;
                }
            }
        }
        false
    }

    /// Decrement a gauge by `delta`.
    ///
    /// Returns `true` if the gauge was found, `false` otherwise.
    pub fn dec_gauge(&self, name: &str, labels: &[Label], delta: f64) -> bool {
        let families = self.families.read();
        for family in families.iter() {
            if family.name == name {
                if let MetricInner::Gauge(ref g) = family.inner {
                    g.dec(labels, delta);
                    return true;
                }
            }
        }
        false
    }

    /// Record a histogram observation.
    ///
    /// Returns `true` if the histogram was found, `false` otherwise.
    pub fn observe(&self, name: &str, labels: &[Label], value: f64) -> bool {
        let families = self.families.read();
        for family in families.iter() {
            if family.name == name {
                if let MetricInner::Histogram(ref h) = family.inner {
                    h.observe(labels, value);
                    return true;
                }
            }
        }
        false
    }

    // -- Rendering ----------------------------------------------------------

    /// Render all registered families in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let families = self.families.read();
        let mut out = String::new();
        for (i, family) in families.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&family.render());
        }
        out
    }

    /// Return the number of registered families.
    pub fn family_count(&self) -> usize {
        self.families.read().len()
    }
}

impl Default for PrometheusRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sort labels by name for deterministic map keys.
fn sorted_labels(labels: &[Label]) -> Vec<Label> {
    let mut v = labels.to_vec();
    v.sort();
    v
}

/// Format an `f64` for Prometheus output. Integers are rendered without a
/// decimal point (e.g. `42` not `42.0`).
fn format_f64(v: f64) -> String {
    if v.fract() == 0.0 && v.is_finite() {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Label tests --------------------------------------------------------

    #[test]
    fn label_display_simple() {
        let l = Label::new("agent", "alpha");
        assert_eq!(l.to_string(), "agent=\"alpha\"");
    }

    #[test]
    fn label_display_escapes_quotes() {
        let l = Label::new("msg", "say \"hello\"");
        assert_eq!(l.to_string(), "msg=\"say \\\"hello\\\"\"");
    }

    #[test]
    fn label_display_escapes_backslash() {
        let l = Label::new("path", "a\\b");
        assert_eq!(l.to_string(), "path=\"a\\\\b\"");
    }

    #[test]
    fn label_display_escapes_newline() {
        let l = Label::new("val", "line1\nline2");
        assert_eq!(l.to_string(), "val=\"line1\\nline2\"");
    }

    #[test]
    fn format_labels_empty() {
        assert_eq!(format_labels(&[]), "");
    }

    #[test]
    fn format_labels_single() {
        let labels = [Label::new("a", "1")];
        assert_eq!(format_labels(&labels), "{a=\"1\"}");
    }

    #[test]
    fn format_labels_multiple() {
        let labels = [Label::new("a", "1"), Label::new("b", "2")];
        assert_eq!(format_labels(&labels), "{a=\"1\",b=\"2\"}");
    }

    // -- Counter tests ------------------------------------------------------

    #[test]
    fn counter_starts_at_zero() {
        let c = Counter::new();
        assert_eq!(c.get(&[]), 0.0);
    }

    #[test]
    fn counter_increment_no_labels() {
        let c = Counter::new();
        c.increment(&[], 1.0);
        c.increment(&[], 2.0);
        assert_eq!(c.get(&[]), 3.0);
    }

    #[test]
    fn counter_increment_with_labels() {
        let c = Counter::new();
        let ok = [Label::new("status", "ok")];
        let err = [Label::new("status", "err")];
        c.increment(&ok, 5.0);
        c.increment(&err, 2.0);
        c.increment(&ok, 1.0);
        assert_eq!(c.get(&ok), 6.0);
        assert_eq!(c.get(&err), 2.0);
    }

    #[test]
    fn counter_ignores_negative_delta() {
        let c = Counter::new();
        c.increment(&[], 10.0);
        c.increment(&[], -5.0);
        assert_eq!(c.get(&[]), 10.0);
    }

    #[test]
    fn counter_reset_clears_all_series() {
        let c = Counter::new();
        c.increment(&[], 10.0);
        c.increment(&[Label::new("a", "1")], 5.0);
        c.reset();
        assert_eq!(c.get(&[]), 0.0);
        assert_eq!(c.get(&[Label::new("a", "1")]), 0.0);
    }

    #[test]
    fn counter_series_returns_all() {
        let c = Counter::new();
        c.increment(&[Label::new("x", "1")], 1.0);
        c.increment(&[Label::new("x", "2")], 2.0);
        let s = c.series();
        assert_eq!(s.len(), 2);
    }

    // -- Gauge tests --------------------------------------------------------

    #[test]
    fn gauge_starts_at_zero() {
        let g = Gauge::new();
        assert_eq!(g.get(&[]), 0.0);
    }

    #[test]
    fn gauge_set() {
        let g = Gauge::new();
        g.set(&[], 42.0);
        assert_eq!(g.get(&[]), 42.0);
    }

    #[test]
    fn gauge_inc() {
        let g = Gauge::new();
        g.inc(&[], 5.0);
        g.inc(&[], 3.0);
        assert_eq!(g.get(&[]), 8.0);
    }

    #[test]
    fn gauge_dec() {
        let g = Gauge::new();
        g.set(&[], 10.0);
        g.dec(&[], 3.0);
        assert_eq!(g.get(&[]), 7.0);
    }

    #[test]
    fn gauge_can_go_negative() {
        let g = Gauge::new();
        g.dec(&[], 5.0);
        assert_eq!(g.get(&[]), -5.0);
    }

    #[test]
    fn gauge_with_labels() {
        let g = Gauge::new();
        let a = [Label::new("agent", "alpha")];
        let b = [Label::new("agent", "beta")];
        g.set(&a, 10.0);
        g.set(&b, 20.0);
        assert_eq!(g.get(&a), 10.0);
        assert_eq!(g.get(&b), 20.0);
    }

    #[test]
    fn gauge_reset() {
        let g = Gauge::new();
        g.set(&[], 99.0);
        g.reset();
        assert_eq!(g.get(&[]), 0.0);
    }

    // -- Histogram tests ----------------------------------------------------

    #[test]
    fn histogram_empty() {
        let h = Histogram::new();
        assert_eq!(h.get(&[]), (0, 0.0));
    }

    #[test]
    fn histogram_observe_count_and_sum() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);
        h.observe(&[], 2.5);
        h.observe(&[], 7.0);
        let (count, sum) = h.get(&[]);
        assert_eq!(count, 2);
        assert!((sum - 9.5).abs() < f64::EPSILON);
    }

    #[test]
    fn histogram_bucket_cumulative_counts() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);
        h.observe(&[], 0.5); // fits in 1.0, 5.0, 10.0
        h.observe(&[], 3.0); // fits in 5.0, 10.0
        h.observe(&[], 7.0); // fits in 10.0
        h.observe(&[], 15.0); // fits in none

        let series = h.all_series();
        let (_, _, buckets) = series.get(&Vec::<Label>::new()).cloned().unwrap_or_default();
        // Raw (non-cumulative) counts stored per bucket:
        // le=1.0 -> 1, le=5.0 -> 1, le=10.0 -> 1
        assert_eq!(buckets[0], (1.0, 1));
        assert_eq!(buckets[1], (5.0, 1));
        assert_eq!(buckets[2], (10.0, 1));
    }

    #[test]
    fn histogram_with_labels() {
        let h = Histogram::with_buckets(&[1.0, 10.0]);
        let a = [Label::new("agent", "a")];
        let b = [Label::new("agent", "b")];
        h.observe(&a, 5.0);
        h.observe(&b, 2.0);
        h.observe(&b, 3.0);
        assert_eq!(h.get(&a), (1, 5.0));
        assert_eq!(h.get(&b), (2, 5.0));
    }

    #[test]
    fn histogram_default_buckets() {
        let h = Histogram::new();
        assert_eq!(h.upper_bounds(), DEFAULT_BUCKETS);
    }

    #[test]
    fn histogram_custom_buckets_sorted_and_deduped() {
        let h = Histogram::with_buckets(&[10.0, 1.0, 5.0, 1.0]);
        assert_eq!(h.upper_bounds(), &[1.0, 5.0, 10.0]);
    }

    #[test]
    fn histogram_reset() {
        let h = Histogram::with_buckets(&[1.0]);
        h.observe(&[], 0.5);
        h.reset();
        assert_eq!(h.get(&[]), (0, 0.0));
    }

    // -- MetricFamily / render tests ----------------------------------------

    #[test]
    fn render_counter_family() {
        let family = MetricFamily {
            name: "polkagent_runs_total".to_string(),
            help: "Total number of agent runs".to_string(),
            metric_type: MetricType::Counter,
            inner: MetricInner::Counter(Counter::new()),
        };
        if let MetricInner::Counter(ref c) = family.inner {
            c.increment(
                &[Label::new("agent", "alpha"), Label::new("status", "completed")],
                42.0,
            );
            c.increment(
                &[Label::new("agent", "alpha"), Label::new("status", "failed")],
                3.0,
            );
        }
        let text = family.render();
        assert!(text.contains("# HELP polkagent_runs_total Total number of agent runs\n"));
        assert!(text.contains("# TYPE polkagent_runs_total counter\n"));
        assert!(text.contains(
            "polkagent_runs_total{agent=\"alpha\",status=\"completed\"} 42\n"
        ));
        assert!(text.contains(
            "polkagent_runs_total{agent=\"alpha\",status=\"failed\"} 3\n"
        ));
    }

    #[test]
    fn render_gauge_family() {
        let family = MetricFamily {
            name: "polkagent_active_runs".to_string(),
            help: "Number of currently active runs".to_string(),
            metric_type: MetricType::Gauge,
            inner: MetricInner::Gauge(Gauge::new()),
        };
        if let MetricInner::Gauge(ref g) = family.inner {
            g.set(&[], 7.0);
        }
        let text = family.render();
        assert!(text.contains("# TYPE polkagent_active_runs gauge\n"));
        assert!(text.contains("polkagent_active_runs 7\n"));
    }

    #[test]
    fn render_histogram_family() {
        let family = MetricFamily {
            name: "polkagent_run_duration_seconds".to_string(),
            help: "Duration of agent runs in seconds".to_string(),
            metric_type: MetricType::Histogram,
            inner: MetricInner::Histogram(Histogram::with_buckets(&[0.5, 1.0, 5.0])),
        };
        if let MetricInner::Histogram(ref h) = family.inner {
            h.observe(&[], 0.3);
            h.observe(&[], 0.8);
            h.observe(&[], 3.0);
        }
        let text = family.render();
        assert!(text.contains("# TYPE polkagent_run_duration_seconds histogram\n"));
        assert!(text.contains("polkagent_run_duration_seconds_bucket{le=\"0.5\"} 1\n"));
        assert!(text.contains("polkagent_run_duration_seconds_bucket{le=\"1\"} 2\n"));
        assert!(text.contains("polkagent_run_duration_seconds_bucket{le=\"5\"} 3\n"));
        assert!(text.contains("polkagent_run_duration_seconds_bucket{le=\"+Inf\"} 3\n"));
        assert!(text.contains("polkagent_run_duration_seconds_count 3\n"));
    }

    #[test]
    fn render_counter_no_labels() {
        let family = MetricFamily {
            name: "my_counter".to_string(),
            help: "A counter".to_string(),
            metric_type: MetricType::Counter,
            inner: MetricInner::Counter(Counter::new()),
        };
        if let MetricInner::Counter(ref c) = family.inner {
            c.increment(&[], 5.0);
        }
        let text = family.render();
        assert!(text.contains("my_counter 5\n"));
    }

    #[test]
    fn render_histogram_sum_line() {
        let family = MetricFamily {
            name: "h".to_string(),
            help: "help".to_string(),
            metric_type: MetricType::Histogram,
            inner: MetricInner::Histogram(Histogram::with_buckets(&[1.0])),
        };
        if let MetricInner::Histogram(ref h) = family.inner {
            h.observe(&[], 0.5);
            h.observe(&[], 0.3);
        }
        let text = family.render();
        assert!(text.contains("h_sum 0.8\n"));
        assert!(text.contains("h_count 2\n"));
    }

    // -- Registry tests -----------------------------------------------------

    #[test]
    fn registry_register_and_count() {
        let reg = PrometheusRegistry::new();
        reg.register_counter("c1", "help");
        reg.register_gauge("g1", "help");
        reg.register_histogram("h1", "help", &[1.0]);
        assert_eq!(reg.family_count(), 3);
    }

    #[test]
    fn registry_increment_counter() {
        let reg = PrometheusRegistry::new();
        reg.register_counter("c", "help");
        assert!(reg.increment("c", &[], 1.0));
        assert!(reg.increment("c", &[], 2.0));
        let text = reg.render();
        assert!(text.contains("c 3\n"));
    }

    #[test]
    fn registry_increment_missing_returns_false() {
        let reg = PrometheusRegistry::new();
        assert!(!reg.increment("nope", &[], 1.0));
    }

    #[test]
    fn registry_set_gauge() {
        let reg = PrometheusRegistry::new();
        reg.register_gauge("g", "help");
        assert!(reg.set_gauge("g", &[], 42.0));
        let text = reg.render();
        assert!(text.contains("g 42\n"));
    }

    #[test]
    fn registry_inc_dec_gauge() {
        let reg = PrometheusRegistry::new();
        reg.register_gauge("g", "help");
        reg.inc_gauge("g", &[], 10.0);
        reg.dec_gauge("g", &[], 3.0);
        let text = reg.render();
        assert!(text.contains("g 7\n"));
    }

    #[test]
    fn registry_observe_histogram() {
        let reg = PrometheusRegistry::new();
        reg.register_histogram("h", "help", &[1.0, 5.0]);
        assert!(reg.observe("h", &[], 0.5));
        assert!(reg.observe("h", &[], 3.0));
        let text = reg.render();
        assert!(text.contains("h_count 2\n"));
    }

    #[test]
    fn registry_observe_missing_returns_false() {
        let reg = PrometheusRegistry::new();
        assert!(!reg.observe("nope", &[], 1.0));
    }

    #[test]
    fn registry_set_gauge_missing_returns_false() {
        let reg = PrometheusRegistry::new();
        assert!(!reg.set_gauge("nope", &[], 1.0));
    }

    #[test]
    fn registry_default_metrics_count() {
        let reg = PrometheusRegistry::with_default_metrics();
        // 4 counters + 3 histograms + 3 gauges = 10
        assert_eq!(reg.family_count(), 10);
    }

    #[test]
    fn registry_default_metrics_render_nonempty() {
        let reg = PrometheusRegistry::with_default_metrics();
        reg.increment(
            "polkagent_runs_total",
            &[Label::new("agent", "alpha"), Label::new("status", "completed")],
            1.0,
        );
        let text = reg.render();
        assert!(text.contains("polkagent_runs_total"));
        assert!(text.contains("polkagent_active_runs"));
    }

    #[test]
    fn registry_render_empty() {
        let reg = PrometheusRegistry::new();
        assert!(reg.render().is_empty());
    }

    #[test]
    fn registry_clone_shares_state() {
        let reg = PrometheusRegistry::new();
        reg.register_counter("c", "help");
        let reg2 = reg.clone();
        reg.increment("c", &[], 5.0);
        let text = reg2.render();
        assert!(text.contains("c 5\n"));
    }

    // -- Concurrency tests --------------------------------------------------

    #[test]
    fn counter_concurrent_increments() {
        let c = Arc::new(Counter::new());
        let mut handles = Vec::new();
        for _ in 0..10 {
            let c = Arc::clone(&c);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    c.increment(&[], 1.0);
                }
            }));
        }
        for h in handles {
            h.join().expect("thread panicked");
        }
        assert_eq!(c.get(&[]), 1000.0);
    }

    #[test]
    fn gauge_concurrent_set() {
        let g = Arc::new(Gauge::new());
        let mut handles = Vec::new();
        for i in 0..10 {
            let g = Arc::clone(&g);
            handles.push(std::thread::spawn(move || {
                g.set(&[], f64::from(i));
            }));
        }
        for h in handles {
            h.join().expect("thread panicked");
        }
        // Value should be one of 0..10
        let val = g.get(&[]);
        assert!((0.0..10.0).contains(&val));
    }

    #[test]
    fn histogram_concurrent_observe() {
        let h = Arc::new(Histogram::with_buckets(&[1.0, 5.0, 10.0]));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let h = Arc::clone(&h);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    h.observe(&[], 2.5);
                }
            }));
        }
        for handle in handles {
            handle.join().expect("thread panicked");
        }
        let (count, sum) = h.get(&[]);
        assert_eq!(count, 1000);
        assert!((sum - 2500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_concurrent_operations() {
        let reg = Arc::new(PrometheusRegistry::new());
        reg.register_counter("c", "help");
        reg.register_gauge("g", "help");
        reg.register_histogram("h", "help", &[1.0]);

        let mut handles = Vec::new();
        for _ in 0..5 {
            let r = Arc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    r.increment("c", &[], 1.0);
                    r.set_gauge("g", &[], 1.0);
                    r.observe("h", &[], 0.5);
                }
            }));
        }
        for h in handles {
            h.join().expect("thread panicked");
        }

        let text = reg.render();
        assert!(text.contains("c 500\n"));
    }

    // -- format_f64 tests ---------------------------------------------------

    #[test]
    fn format_f64_integer() {
        assert_eq!(format_f64(42.0), "42");
    }

    #[test]
    fn format_f64_decimal() {
        assert_eq!(format_f64(3.14), "3.14");
    }

    // -- MetricType display --------------------------------------------------

    #[test]
    fn metric_type_display() {
        assert_eq!(MetricType::Counter.to_string(), "counter");
        assert_eq!(MetricType::Gauge.to_string(), "gauge");
        assert_eq!(MetricType::Histogram.to_string(), "histogram");
    }

    // -- Label ordering stability -------------------------------------------

    #[test]
    fn labels_sorted_for_key_stability() {
        let c = Counter::new();
        let labels_ab = [Label::new("a", "1"), Label::new("b", "2")];
        let labels_ba = [Label::new("b", "2"), Label::new("a", "1")];
        c.increment(&labels_ab, 1.0);
        c.increment(&labels_ba, 1.0);
        // Both refer to the same series because labels are sorted.
        assert_eq!(c.get(&labels_ab), 2.0);
        assert_eq!(c.get(&labels_ba), 2.0);
        assert_eq!(c.series().len(), 1);
    }
}
