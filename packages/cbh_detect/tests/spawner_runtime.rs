//! Exercises the analysis dispatch adapter against a real runtime.

use anyspawn::Spawner as RuntimeSpawner;
use cbh_detect::Spawner;
use futures::executor::block_on;
use tokio::runtime::Builder;

::testing::set_allocator!();

#[test]
#[cfg_attr(miri, ignore = "exercises real runtime task scheduling")]
fn runtime_dispatch_returns_task_values() {
    let runtime = Builder::new_current_thread().build().unwrap();
    let spawner = Spawner::new(RuntimeSpawner::new_tokio(runtime.handle().clone()));
    runtime.block_on(async {
        assert_eq!(spawner.spawn(async { "loaded" }).await, "loaded");
        assert_eq!(spawner.spawn_blocking(|| "detected").await, "detected");
    });
}

#[test]
#[cfg_attr(miri, ignore = "exercises real runtime task scheduling")]
fn runtime_dispatch_propagates_task_panics() {
    let runtime = Builder::new_current_thread().build().unwrap();
    let spawner = Spawner::new(RuntimeSpawner::new_tokio(runtime.handle().clone()));
    testing::assert_panics(|| {
        runtime.block_on(spawner.spawn(async { panic!("task panic") }));
    });
    testing::assert_panics(|| {
        runtime.block_on(spawner.spawn_blocking(|| panic!("task panic")));
    });
}

#[test]
#[cfg_attr(miri, ignore = "exercises real runtime shutdown")]
fn runtime_dispatch_reports_cancelled_tasks() {
    let runtime = Builder::new_current_thread().build().unwrap();
    let spawner = Spawner::new(RuntimeSpawner::new_tokio(runtime.handle().clone()));
    let task = spawner.spawn(std::future::pending::<()>());
    drop(runtime);
    testing::assert_panics(|| block_on(task));
}
