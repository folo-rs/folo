//! Public construction and linked-object behavior against the real hardware provider.

#![cfg(not(miri))]

use std::sync::Arc;
use std::thread;

use linked::{InstancePerThread, InstancePerThreadSync};
use region_local::{RegionLocal, RegionLocalExt, region_local};
use testing::with_watchdog;

#[test]
fn real_smoke_test() {
    region_local! {
        static FAVORITE_COLOR: String = "blue".to_string();
    }

    FAVORITE_COLOR.with_local(|color| {
        assert_eq!(*color, "blue");
    });

    FAVORITE_COLOR.set_local("red".to_string());

    FAVORITE_COLOR.with_local(|color| {
        assert_eq!(*color, "red");
    });
}

#[test]
fn with_non_const_initial_value() {
    region_local!(static FAVORITE_COLOR: Arc<String> = Arc::new("blue".to_string()));

    FAVORITE_COLOR.with_local(|color| {
        assert_eq!(**color, "blue");
    });
}

#[test]
fn non_static() {
    with_watchdog(|| {
        let favorite_color_linked = InstancePerThread::new(RegionLocal::new(|| "blue".to_string()));

        let favorite_color = favorite_color_linked.acquire();

        favorite_color.with_local(|color| {
            assert_eq!(*color, "blue");
        });

        thread::spawn(move || {
            let favorite_color = favorite_color_linked.acquire();

            favorite_color.with_local(|color| {
                assert_eq!(*color, "blue");
            });

            favorite_color.set_local("red".to_string());

            favorite_color.with_local(|color| {
                assert_eq!(*color, "red");
            });
        })
        .join()
        .unwrap();

        // The other thread may use a different memory region, so its write need not
        // change the value observed by this thread.
    });
}

#[test]
fn non_static_sync() {
    with_watchdog(|| {
        let favorite_color_linked =
            InstancePerThreadSync::new(RegionLocal::new(|| "blue".to_string()));

        let favorite_color = favorite_color_linked.acquire();

        favorite_color.with_local(|color| {
            assert_eq!(*color, "blue");
        });

        thread::spawn(move || {
            let favorite_color = favorite_color_linked.acquire();

            favorite_color.with_local(|color| {
                assert_eq!(*color, "blue");
            });

            favorite_color.set_local("red".to_string());

            favorite_color.with_local(|color| {
                assert_eq!(*color, "red");
            });
        })
        .join()
        .unwrap();

        // The other thread may use a different memory region, so its write need not
        // change the value observed by this thread.
    });
}
