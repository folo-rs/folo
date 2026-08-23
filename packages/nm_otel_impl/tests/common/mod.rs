use std::sync::{Arc, Weak};
use std::time::Duration;

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_sdk::metrics::reader::MetricReader;
use opentelemetry_sdk::metrics::{InstrumentKind, ManualReader, Pipeline, Temporality};

/// Gives integration tests shared access to an explicitly driven metric reader.
#[derive(Clone, Debug, Default)]
pub(crate) struct TestMetricReader {
    inner: Arc<ManualReader>,
}

impl TestMetricReader {
    pub(crate) fn collect(&self) -> ResourceMetrics {
        let mut metrics = ResourceMetrics::default();
        MetricReader::collect(self, &mut metrics).unwrap();
        metrics
    }
}

pub(crate) fn find_u64_sum(metrics: &ResourceMetrics, name: &str) -> Option<(bool, u64)> {
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

impl MetricReader for TestMetricReader {
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

    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.inner.temporality(kind)
    }
}
