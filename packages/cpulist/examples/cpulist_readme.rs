//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `cpulist` module for parsing and emitting processor lists.

fn main() {
    println!("=== CPUList README Example ===");

    let selected_processors = cpulist::parse("0-9,32-35,40").unwrap();
    assert_eq!(
        selected_processors,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 32, 33, 34, 35, 40]
    );

    println!("Selected processors: {selected_processors:?}");
    println!("As cpulist: {}", cpulist::emit(selected_processors));

    println!("README example completed successfully!");
}
