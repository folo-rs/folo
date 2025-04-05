//! Parsing a cpulist string and emitting it back to the terminal.

fn main() {
    let selected_processors = cpulist::parse("0-9,32-35,40").unwrap();
    assert_eq!(
        selected_processors,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 32, 33, 34, 35, 40]
    );

    println!("Selected processors: {:?}", selected_processors);
    println!("As cpulist: {}", cpulist::emit(&selected_processors));
}
