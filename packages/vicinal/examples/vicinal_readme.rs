//! Example from the README.

use vicinal::Pool;

#[tokio::main]
async fn main() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let task1 = scheduler.spawn(|| 42);
    let _task2 = scheduler.spawn(|| println!("doing some stuff"));

    assert_eq!(task1.await, 42);

    let scheduler2 = scheduler.clone();
    let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));

    assert_eq!(task3.await.await, 55);
}
