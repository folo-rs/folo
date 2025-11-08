//! Example used in crate-level documentation. See docs for description.

use events_once::{Disconnected, Event};

#[tokio::main]
async fn main() {
    let (sender, receiver) = Event::<String>::boxed();

    // into_value() is designed for synchronous scenarios where you do not want to wait but
    // simply want to either obtain the received value or do nothing. First, we do nothing.
    let receiver = match receiver.into_value() {
        Ok(result) => {
            // The result is either Ok(payload) or Err(Disconnected).
            match result {
                Ok(message) => {
                    // Just for demonstration. In reality, we know this line of code
                    // will never be reached because no message is sent yet.
                    println!("Received message: {message}");
                    return;
                }
                Err(Disconnected) => {
                    panic!("The sender was disconnected before sending a message.")
                }
            }
        }
        Err(receiver) => {
            // If no value is available, the original receiver is returned.
            receiver
        }
    };

    sender.send("Hello, world!".to_string());

    match receiver.into_value() {
        Ok(result) => {
            // The result is either Ok(payload) or Err(Disconnected).
            match result {
                Ok(message) => {
                    println!("Received message: {message}");
                    return;
                }
                Err(Disconnected) => {
                    panic!("The sender was disconnected before sending a message.")
                }
            }
        }
        Err(_) => {
            panic!("No value was received even after send(). This should never happen.");
        }
    };
}
