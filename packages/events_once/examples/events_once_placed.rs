//! Example used in crate-level documentation. See docs for description.

use events_once::{EmbeddedEvent, Event};
use pin_project::pin_project;
use tokio::task::JoinError;

#[pin_project]
struct Account {
    id: u64,

    // Event triggered when the account has been prepared
    // and is ready for use by the customer's user agent.
    #[pin]
    ready_to_use: EmbeddedEvent<()>,
}

#[tokio::main]
async fn main() -> Result<(), JoinError> {
    let mut account = Box::pin(Account {
        id: 42,
        ready_to_use: EmbeddedEvent::new(),
    });

    let ready_to_use = account.as_mut().project().ready_to_use;

    // SAFETY: `Box::pin` gives `account` stable, exclusive heap storage that outlives this
    // scope. `ready_to_use` is freshly initialized above and passed to `Event::placed` only
    // once here. The task that owns `account` awaits `ready_rx` to completion before dropping
    // `account`, and `ready_tx` is consumed by `send` in the other task before that receive can
    // complete, so both endpoints finish before `account` is dropped.
    let (ready_tx, ready_rx) = unsafe { Event::placed(ready_to_use) };

    let prepare_account_task = tokio::spawn(async move {
        // Signal that the account is ready to use.
        ready_tx.send(());
    });

    let use_account_task = tokio::spawn(async move {
        // Wait until the account is ready to use.
        ready_rx.await.unwrap();

        println!("Account {} is ready to use!", account.id);
    });

    // The safety promise we made requires that we keep the account alive for at least as long
    // as the event endpoints are alive. Joining both tasks together ensures they have both
    // completed before `account` is dropped, and propagating each result turns a task panic or
    // cancellation into a failure of this example instead of silently discarding it.
    let (prepare_result, use_result) = tokio::join!(prepare_account_task, use_account_task);
    prepare_result?;
    use_result?;

    Ok(())
}
