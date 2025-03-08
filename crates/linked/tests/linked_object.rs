#[test]
fn empty_struct() {
    #[linked::object]
    struct Empty {}

    impl Empty {
        pub fn new() -> Self {
            linked::new!(Self {})
        }
    }

    _ = Empty::new();
}

#[test]
fn very_empty_struct() {
    #[linked::object]
    struct Empty {}

    impl Empty {
        pub fn new() -> Self {
            linked::new!(Self)
        }
    }

    _ = Empty::new();
}
