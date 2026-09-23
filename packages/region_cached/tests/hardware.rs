//! Public construction and linked-object behavior against the real hardware provider.

#![cfg(not(miri))]

use std::sync::Arc;
use std::thread;

use linked::{InstancePerThread, InstancePerThreadSync};
use region_cached::{RegionCached, RegionCachedCopyExt, RegionCachedExt, region_cached};
use testing::with_watchdog;

#[test]
fn real_smoke_test() {
    region_cached! {
        static FAVORITE_COLOR: String = "blue".to_string();
        static FAVORITE_NUMBER: i32 = 42;
    }

    FAVORITE_COLOR.with_cached(|color| {
        assert_eq!(*color, "blue");
    });

    FAVORITE_COLOR.set_global("red".to_string());

    FAVORITE_COLOR.with_cached(|color| {
        assert_eq!(*color, "red");
    });

    assert_eq!(FAVORITE_NUMBER.get_cached(), 42);
}

#[test]
fn with_non_const_initial_value() {
    region_cached!(static FAVORITE_COLOR: Arc<String> = Arc::new("blue".to_string()));

    FAVORITE_COLOR.with_cached(|color| {
        assert_eq!(**color, "blue");
    });
}

#[test]
fn non_static() {
    with_watchdog(|| {
        let favorite_color_linked = InstancePerThread::new(RegionCached::new("blue".to_string()));

        let favorite_color = favorite_color_linked.acquire();

        favorite_color.with_cached(|color| {
            assert_eq!(*color, "blue");
        });

        thread::spawn(move || {
            let favorite_color = favorite_color_linked.acquire();

            favorite_color.with_cached(|color| {
                assert_eq!(*color, "blue");
            });

            favorite_color.set_global("red".to_string());

            favorite_color.with_cached(|color| {
                assert_eq!(*color, "red");
            });
        })
        .join()
        .unwrap();

        favorite_color.with_cached(|color| {
            assert_eq!(*color, "red");
        });
    });
}

#[test]
fn non_static_sync() {
    with_watchdog(|| {
        let favorite_color_linked =
            InstancePerThreadSync::new(RegionCached::new("blue".to_string()));

        let favorite_color = favorite_color_linked.acquire();

        favorite_color.with_cached(|color| {
            assert_eq!(*color, "blue");
        });

        thread::spawn(move || {
            let favorite_color = favorite_color_linked.acquire();

            favorite_color.with_cached(|color| {
                assert_eq!(*color, "blue");
            });

            favorite_color.set_global("red".to_string());

            favorite_color.with_cached(|color| {
                assert_eq!(*color, "red");
            });
        })
        .join()
        .unwrap();

        favorite_color.with_cached(|color| {
            assert_eq!(*color, "red");
        });
    });
}
