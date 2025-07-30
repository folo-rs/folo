use std::time::{Duration, Instant};

use crate::pal::TimeSource;
use crate::pal::linux::{Bindings, BindingsFacade};

#[derive(Debug)]
pub(crate) struct TimeSourceImpl {
    rust_epoch: Instant,
    platform_epoch: u128,

    bindings: BindingsFacade,
}

impl TimeSourceImpl {
    pub(crate) fn new(bindings: BindingsFacade) -> Self {
        Self {
            rust_epoch: bindings.now(),
            platform_epoch: bindings.clock_gettime_nanos(),

            bindings,
        }
    }
}

impl TimeSource for TimeSourceImpl {
    fn now(&self) -> Instant {
        let elapsed_nanos = self
            .bindings
            .clock_gettime_nanos()
            .saturating_sub(self.platform_epoch);

        self.rust_epoch
            .checked_add(Duration::from_nanos(u64::try_from(elapsed_nanos).expect(
                "unrealistically long duration, never going to happen with real clocks",
            )))
            .expect("platform timestamp beyond the end of the universe - impossible")
    }
}

#[cfg(test)]
mod tests {
    use mockall::Sequence;

    use super::*;
    use crate::pal::linux::bindings::MockBindings;

    #[test]
    fn smoke_test() {
        let mut bindings = MockBindings::new();

        let rust_epoch = Instant::now();

        bindings.expect_now().once().return_const(rust_epoch);

        let mut seq = Sequence::new();
        bindings
            .expect_clock_gettime_nanos()
            .once()
            .in_sequence(&mut seq)
            .return_const(9_000_000_000_u64);

        // A - one second elapsed.
        bindings
            .expect_clock_gettime_nanos()
            .once()
            .in_sequence(&mut seq)
            .return_const(10_000_000_000_u64);

        // B - still one second elapsed.
        bindings
            .expect_clock_gettime_nanos()
            .once()
            .in_sequence(&mut seq)
            .return_const(10_000_000_000_u64);

        // C - one second and one millisecond elapsed
        bindings
            .expect_clock_gettime_nanos()
            .once()
            .in_sequence(&mut seq)
            .return_const(10_001_000_000_u64);

        let time_source = TimeSourceImpl::new(bindings.into());

        let a = time_source.now();
        let b = time_source.now();
        let c = time_source.now();

        assert_eq!(
            a.saturating_duration_since(rust_epoch),
            Duration::from_secs(1)
        );
        assert_eq!(
            b.saturating_duration_since(rust_epoch),
            Duration::from_secs(1)
        );
        assert_eq!(
            c.saturating_duration_since(rust_epoch),
            Duration::from_millis(1001)
        );
    }
}
