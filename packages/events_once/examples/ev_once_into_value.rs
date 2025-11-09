//! Example used in crate-level documentation. See docs for description.

use events_once::{Event, IntoValueError};

#[tokio::main]
async fn main() {
    let (sender, receiver) = Event::<String>::boxed();

    // into_value() is designed for synchronous scenarios where you do not want to wait but
    // simply want to either obtain the received value or do nothing. First, we do nothing.
    //
    // If no value has been sent yet, into_value() returns Err(IntoValueError::Pending(self)).
    let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
        panic!("Expected receiver to indicate that it is still waiting for a payload to be sent.");
    };

    sender.send("Hello, world!".to_string());

    let message = receiver.into_value().unwrap();

    println!("Received message: {message}");
}
