//! Example used in crate-level documentation. See docs for description.

use events_once::RawEventPool;

#[tokio::main]
async fn main() {
    const CUSTOMER_COUNT: usize = 5;

    let pool = Box::pin(RawEventPool::<String>::new());

    for customer_index in 0..CUSTOMER_COUNT {
        // SAFETY: We promise the pool outlives both the returned endpoints.
        let (tx, rx) = unsafe { pool.as_ref().rent() };

        tx.send(format!(
            "Customer {customer_index} has entered the building"
        ));

        let message = rx.await.unwrap();
        println!("{message}");

        // Both endpoints are dropped now and the event is returned to the pool.
        // The next iteration will reuse the resources associated with the first event.
    }
}
