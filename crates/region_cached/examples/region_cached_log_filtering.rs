use std::thread;

use region_cached::{RegionCachedExt, region_cached};

region_cached!(static FILTER_KEYS: Vec<String> = load_initial_filters());

/// Returns true if the log line contains any of the filter keys.
fn process_log_line(line: &str) -> bool {
    // `.with_current()` provides an immutable reference to the cached value.
    FILTER_KEYS.with_current(|keys| keys.iter().any(|key| line.contains(key)))
}

fn update_filters(new_filters: Vec<String>) {
    // `.set()` publishes a new value, which will be distributed to all memory regions in an
    // eventually consistent manner.
    FILTER_KEYS.set(new_filters);
}

fn load_initial_filters() -> Vec<String> {
    // For example purposes we only have a trivial data set, which makes little sense to cache.
    // In realistic scenarios, you would want to use region-local caching only if your data
    // set is too large to naturally fit in processor caches (e.g. 100K+ entries). Other
    // considerations also apply - let profiling be your guide in choosing your data structures.
    vec!["error".to_string(), "panic".to_string()]
}

static SAMPLE_LOG_LINES: &[&str] = &[
    "info: everything is fine",
    "error: something went wrong",
    "warning: this is a warning",
    "panic: oh no, we're doomed",
];

fn main() {
    // Start a bunch of threads that will process log lines.
    let mut threads = Vec::new();

    for _ in 0..100 {
        threads.push(thread::spawn(|| {
            for line in SAMPLE_LOG_LINES {
                if process_log_line(line) {
                    println!("Matched filters: {}", line);
                }
            }
        }));
    }

    let new_filters = vec![
        "error".to_string(),
        "panic".to_string(),
        "warning".to_string(),
    ];

    // Update the filters. The update will arrive eventually on all threads in all memory regions.
    // In terminal output, you may see the first threads act on the initial data set and later
    // threads act on the updated data set, simply because the first threads already finish before
    // getting the updated value.
    update_filters(new_filters);

    for thread in threads {
        thread.join().unwrap();
    }

    println!("All threads have finished processing log lines.");
}
