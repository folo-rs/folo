//! Example used in crate-level documentation. See docs for description.

use events_once::Event;

#[tokio::main]
async fn main() {
    let (sender, receiver) = Event::<String>::boxed();

    sender.send("Hello, world!".to_string());

    // Events are thread-safe by default and their endpoints
    // may be freely moved to other threads or tasks.
    tokio::spawn(async move {
        let message = receiver.await.unwrap();
        println!("{message}");
    })
    .await
    .unwrap();
}
