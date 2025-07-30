use std::time::{Duration, Instant};

use crate::pal::TimeSource;
use crate::pal::windows::{Bindings, BindingsFacade};

#[derive(Debug)]
pub(crate) struct TimeSourceImpl {
    rust_epoch: Instant,
    platform_epoch: u64,

    bindings: BindingsFacade,
}

impl TimeSourceImpl {
    pub(crate) fn new(bindings: BindingsFacade) -> Self {
        Self {
            rust_epoch: bindings.now(),
            platform_epoch: bindings.get_tick_count_64(),

            bindings,
        }
    }
}

impl TimeSource for TimeSourceImpl {
    fn now(&self) -> Instant {
        let elapsed_millis = self
            .bindings
            .get_tick_count_64()
            .saturating_sub(self.platform_epoch);

        self.rust_epoch
            .checked_add(Duration::from_millis(elapsed_millis))
            .expect("platform timestamp beyond the end of the universe - impossible")
    }
}

#[cfg(test)]
mod tests {
    use mockall::Sequence;

    use super::*;
    use crate::pal::windows::bindings::MockBindings;

    #[test]
    fn smoke_test() {
        let mut bindings = MockBindings::new();

        let rust_epoch = Instant::now();

        bindings.expect_now().once().return_const(rust_epoch);

        let mut seq = Sequence::new();
        bindings
            .expect_get_tick_count_64()
            .once()
            .in_sequence(&mut seq)
            .return_const(9_000_u64);

        // A - one second elapsed.
        bindings
            .expect_get_tick_count_64()
            .once()
            .in_sequence(&mut seq)
            .return_const(10_000_u64);

        // B - still one second elapsed.
        bindings
            .expect_get_tick_count_64()
            .once()
            .in_sequence(&mut seq)
            .return_const(10_000_u64);

        // C - one second and one millisecond elapsed
        bindings
            .expect_get_tick_count_64()
            .once()
            .in_sequence(&mut seq)
            .return_const(10_001_u64);

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
