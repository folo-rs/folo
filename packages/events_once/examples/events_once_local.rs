//! Example used in crate-level documentation. See docs for description.

use events_once::LocalEvent;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (sender, receiver) = LocalEvent::<String>::boxed();

    sender.send("Hello, world!".to_string());

    let message = receiver.await.unwrap();
    println!("{message}");
}
