use std::fmt::Debug;
use std::time::Instant;

pub(crate) trait Platform: Debug + Send + Sync + 'static {
    type TimeSource: TimeSource;

    fn new_time_source(&self) -> Self::TimeSource;
}

#[cfg_attr(test, mockall::automock)]
pub(crate) trait TimeSource: Debug + Send {
    fn now(&mut self) -> Instant;
}
