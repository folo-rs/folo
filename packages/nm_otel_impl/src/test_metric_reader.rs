use std::sync::{Arc, Weak};
use std::time::Duration;

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_sdk::metrics::reader::MetricReader;
use opentelemetry_sdk::metrics::{
    InstrumentKind, ManualReader, Pipeline, SdkMeterProvider, Temporality,
};

/// Gives tests shared access to an explicitly driven OpenTelemetry metric reader.
#[derive(Clone, Debug, Default)]
pub struct TestMetricReader {
    inner: Arc<ManualReader>,
}

impl TestMetricReader {
    /// Collects the metrics recorded since the preceding collection.
    #[must_use]
    pub fn collect(&self) -> ResourceMetrics {
        let mut metrics = ResourceMetrics::default();
        MetricReader::collect(self, &mut metrics).unwrap();
        metrics
    }
}

impl MetricReader for TestMetricReader {
    // SDK pipeline wiring is observed by the integration tests' exported-metric assertions.
    // A live Pipeline has no public constructor apart from the SDK provider.
    #[cfg_attr(test, mutants::skip)]
    fn register_pipeline(&self, pipeline: Weak<Pipeline>) {
        self.inner.register_pipeline(pipeline);
    }

    fn collect(&self, rm: &mut ResourceMetrics) -> OTelSdkResult {
        self.inner.collect(rm)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    // The wrapped default reader and `Temporality::default()` both select cumulative
    // temporality, so the generated default-return mutation is behaviorally equivalent.
    #[cfg_attr(test, mutants::skip)]
    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.inner.temporality(kind)
    }
}

/// Creates a meter provider and its explicitly driven test reader.
// Provider construction runs SDK resource detection. Integration tests verify the returned
// reader collects this provider's metrics; keep real SDK services outside library unit tests.
#[cfg_attr(test, mutants::skip)]
#[must_use]
pub fn create_test_provider() -> (SdkMeterProvider, TestMetricReader) {
    let reader = TestMetricReader::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(reader.clone())
        .build();
    (provider, reader)
}

/// Finds the monotonic flag and value of a `u64` sum metric.
// Integration-test assertion adapter, not exporter logic. Populated SDK snapshots have no
// public in-memory constructors; obtaining them requires the real SDK collection pipeline.
// Keep that coverage in integration tests rather than expanding library-only mutation targets.
#[cfg_attr(test, mutants::skip)]
pub fn find_u64_sum(metrics: &ResourceMetrics, name: &str) -> Option<(bool, u64)> {
    metrics
        .scope_metrics()
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == name)
        .map(|metric| {
            let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                panic!("expected Sum<u64> metric data");
            };
            let mut data_points = sum.data_points();
            let value = data_points.next().unwrap().value();
            assert!(data_points.next().is_none());
            (sum.is_monotonic(), value)
        })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn collect_rejects_unregistered_reader() {
        // An unregistered reader fails entirely in memory, without SDK resource detection
        // or a collection clock. Failed collection must not masquerade as an empty snapshot.
        _ = TestMetricReader::default().collect();
    }
}
