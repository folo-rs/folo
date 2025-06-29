use std::cell::RefCell;
use std::rc::Rc;

use super::LocalEventPool;

#[test]
fn local_event_pool_by_ref_basic() {
    futures::executor::block_on(async {
        let mut pool = LocalEventPool::new();
        let (sender, receiver) = pool.by_ref();

        sender.send(42);
        let value = receiver.await;
        assert_eq!(value, 42);
    });
}

#[test]
fn local_event_pool_by_rc_basic() {
    futures::executor::block_on(async {
        let pool = Rc::new(RefCell::new(LocalEventPool::new()));
        let (sender, receiver) = pool.borrow_mut().by_rc(&pool);

        sender.send(42);
        let value = receiver.await;
        assert_eq!(value, 42);
    });
}

#[test]
fn local_event_pool_by_ptr_basic() {
    use std::pin::Pin;

    futures::executor::block_on(async {
        let mut pool = LocalEventPool::new();
        let pinned_pool = Pin::new(&mut pool);
        // SAFETY: We ensure the pool outlives the sender and receiver
        let (sender, receiver) = unsafe { pinned_pool.by_ptr() };

        sender.send(42);
        let value = receiver.await;
        assert_eq!(value, 42);
    });
}

#[test]
fn local_event_pool_multiple_events() {
    futures::executor::block_on(async {
        let mut pool = LocalEventPool::new();

        // Test multiple events sequentially
        {
            let (sender1, receiver1) = pool.by_ref();
            sender1.send(1);
            let value1 = receiver1.await;
            assert_eq!(value1, 1);
        }

        {
            let (sender2, receiver2) = pool.by_ref();
            sender2.send(2);
            let value2 = receiver2.await;
            assert_eq!(value2, 2);
        }
    });
}

#[test]
fn local_event_pool_cleanup() {
    futures::executor::block_on(async {
        let mut pool = LocalEventPool::new();

        {
            let (sender, receiver) = pool.by_ref();
            sender.send(42);
            let value = receiver.await;
            assert_eq!(value, 42);
            // sender and receiver dropped here
        }

        // Pool should be able to shrink after cleanup
        pool.shrink_to_fit();
    });
}

#[test]
fn local_event_pool_rc_multiple_events() {
    futures::executor::block_on(async {
        let pool = Rc::new(RefCell::new(LocalEventPool::new()));

        // Create first event
        let (sender1, receiver1) = pool.borrow_mut().by_rc(&pool);

        // Create second event
        let (sender2, receiver2) = pool.borrow_mut().by_rc(&pool);

        // Send values
        sender1.send(1);
        sender2.send(2);

        // Receive values
        let value1 = receiver1.await;
        let value2 = receiver2.await;

        assert_eq!(value1, 1);
        assert_eq!(value2, 2);
    });
}
