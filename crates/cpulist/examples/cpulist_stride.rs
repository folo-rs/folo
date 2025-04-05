//! The stride operator can be used to divide ranges into any number of individual series.

fn main() {
    let evens = cpulist::parse("0-16:2").unwrap();
    let odds = cpulist::parse("1-16:2").unwrap();

    let all = cpulist::emit(odds.iter().chain(evens.iter()));

    println!("Evens: {:?}", evens);
    println!("Odds: {:?}", odds);

    println!("All as cpulist: {}", all);
}
