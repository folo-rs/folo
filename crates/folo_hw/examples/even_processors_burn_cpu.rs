use folo_hw::pinned_threads::{self, Instantiation, Pinning};

fn main() {
    let handles =
        pinned_threads::spawn(Instantiation::PerProcessor, Pinning::PerProcessor, |ctx| {
            if ctx.processor().index_in_group() % 2 == 0 {
                loop {
                    std::hint::spin_loop();
                }
            }
        });

    for handle in handles {
        handle.join().unwrap();
    }
}
