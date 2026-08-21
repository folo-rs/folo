//! Example for README.md demonstration of basic `events_once` usage.

use events_once::Event;
use tokio::task::JoinError;

#[tokio::main]
async fn main() -> Result<(), JoinError> {
    let (sender, receiver) = Event::<String>::boxed();

    sender.send("Hello, world!".to_string());

    // Events are thread-safe by default and their endpoints
    // may be freely moved to other threads or tasks.
    tokio::spawn(async move {
        let message = receiver
            .await
            .expect("Failed to receive message from sender");
        println!("{message}");
    })
    .await?;

    Ok(())
}
