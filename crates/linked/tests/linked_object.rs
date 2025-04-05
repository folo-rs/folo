//! Linked object definition under various edge cases.

#[test]
fn empty_struct() {
    #[linked::object]
    struct Empty {}

    impl Empty {
        fn new() -> Self {
            linked::new!(Self {})
        }
    }

    drop(Empty::new());
}

#[test]
fn very_empty_struct() {
    #[linked::object]
    struct Empty {}

    impl Empty {
        fn new() -> Self {
            linked::new!(Self)
        }
    }

    drop(Empty::new());
}
