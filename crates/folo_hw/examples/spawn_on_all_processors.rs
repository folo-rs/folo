use folo_hw::ProcessorSet;

fn main() {
    let threads = ProcessorSet::all().spawn_threads(|processor| {
        println!("Spawned thread on {processor:?}");
    });

    for thread in threads {
        thread.join().unwrap();
    }
}
