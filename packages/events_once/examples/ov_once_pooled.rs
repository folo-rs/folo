//! Example used in crate-level documentation. See docs for description.

use events_once::EventPool;

#[tokio::main]
async fn main() {
    const CUSTOMER_COUNT: usize = 5;

    let pool = EventPool::<String>::new();

    for customer_index in 0..CUSTOMER_COUNT {
        let (tx, rx) = pool.rent();

        tx.send(format!(
            "Customer {customer_index} has entered the building"
        ));

        let message = rx.await.unwrap();
        println!("{message}");

        // Both endpoints are dropped now and the event is returned to the pool.
        // The next iteration will reuse the resources associated with the first event.
    }
}
