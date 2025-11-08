//! Example used in crate-level documentation. See docs for description.

use std::fmt::Debug;

use events_once::EventLake;

#[tokio::main]
async fn main() {
    let lake = EventLake::new();

    deliver_payload("Hello from the lake!", &lake).await;
    deliver_payload(42, &lake).await;
}

async fn deliver_payload<T>(payload: T, lake: &EventLake)
where
    T: Send + Debug + 'static,
{
    let (tx, rx) = lake.rent::<T>();

    tx.send(payload);
    let payload = rx.await.unwrap();
    println!("Received payload: {payload:?}");
}
