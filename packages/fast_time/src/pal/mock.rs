#![cfg_attr(coverage_nightly, coverage(off))]

use mockall::mock;

use crate::pal::{MockTimeSource, Platform};

mock! {
    #[derive(Debug)]
    pub Platform {
    }

    impl Platform for Platform {
        type TimeSource = MockTimeSource;

        fn new_time_source(&self) -> MockTimeSource;
    }
}
