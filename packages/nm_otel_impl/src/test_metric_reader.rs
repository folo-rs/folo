use std::sync::{Arc, Weak};
use std::time::Duration;

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::reader::MetricReader;
use opentelemetry_sdk::metrics::{
    InstrumentKind, ManualReader, Pipeline, SdkMeterProvider, Temporality,
};

/// Gives unit tests shared access to an explicitly driven OpenTelemetry metric reader.
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

pub(crate) fn create_test_provider() -> (SdkMeterProvider, TestMetricReader) {
    let reader = TestMetricReader::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(reader.clone())
        .build();
    (provider, reader)
}
