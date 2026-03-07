//! Example code for the `README.md` file.
//!
//! This contains the same code that appears in the `futurism` package `README.md`.

fn main() {
    use futures::{StreamExt, executor::block_on};
    use futurism::LocalFutureDeque;

    block_on(async {
        let mut deque = LocalFutureDeque::new();

        deque.push_back(async { 1 });
        deque.push_back(async { 2 });
        deque.push_front(async { 0 });

        assert_eq!(deque.next().await, Some(0));
        assert_eq!(deque.next().await, Some(1));
        assert_eq!(deque.next().await, Some(2));
        assert_eq!(deque.next().await, None);
    });
}
