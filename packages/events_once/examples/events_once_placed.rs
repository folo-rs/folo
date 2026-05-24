//! Example used in crate-level documentation. See docs for description.

use std::time::Duration;

use events_once::{EmbeddedEvent, Event};
use pin_project::pin_project;

#[pin_project]
struct Account {
    id: u64,

    // Event triggered when the account has been prepared
    // and is ready for use by the customer's user agent.
    #[pin]
    ready_to_use: EmbeddedEvent<()>,
}

#[tokio::main]
async fn main() {
    let mut account = Box::pin(Account {
        id: 42,
        ready_to_use: EmbeddedEvent::new(),
    });

    // SAFETY: We promise that `account` lives longer than any of the endpoints returned.
    let (ready_tx, ready_rx) = unsafe { Event::placed(account.as_mut().project().ready_to_use) };

    let prepare_account_task = tokio::spawn(async move {
        // Simulate some asynchronous work to prepare the account.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Signal that the account is ready to use.
        ready_tx.send(());
    });

    let use_account_task = tokio::spawn(async move {
        // Wait until the account is ready to use.
        ready_rx.await.unwrap();

        println!("Account {} is ready to use!", account.id);
    });

    // The safety promise we made requires that we keep the account alive for
    // at least as long as the events endpoints are alive. As we are now dropping
    // the account, we must also ensure that the two tasks using the endpoints
    // have completed first. We do not care about the result here, we just want
    // to ensure that they are done, so they could not possibly be referencing the
    // embedded event once we drop the account.
    drop(prepare_account_task.await);
    drop(use_account_task.await);
}
