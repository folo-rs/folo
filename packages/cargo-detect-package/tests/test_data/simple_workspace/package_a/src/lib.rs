pub fn hello_from_a() {
    println!("Hello from package A!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello() {
        hello_from_a();
    }
}
