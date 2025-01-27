use std::thread;

use nonempty::{nonempty, NonEmpty};

use crate::{pal::Platform, ProcessorCore, ProcessorSetBuilderCore};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

#[derive(Debug)]
pub(crate) struct ProcessorSetCore<PAL: Platform> {
    processors: NonEmpty<ProcessorCore<PAL>>,

    pub(crate) pal: &'static PAL,
}

impl<PAL: Platform> ProcessorSetCore<PAL> {
    pub fn to_builder(&self) -> ProcessorSetBuilderCore<PAL> {
        ProcessorSetBuilderCore::<PAL>::new(self.pal).filter(|p| self.processors.contains(p))
    }

    pub(crate) fn new(processors: NonEmpty<ProcessorCore<PAL>>, pal: &'static PAL) -> Self {
        Self { processors, pal }
    }

    pub(crate) fn len(&self) -> usize {
        self.processors.len()
    }

    pub(crate) fn processors(&self) -> impl Iterator<Item = &ProcessorCore<PAL>> + '_ {
        self.processors.iter()
    }

    pub(crate) fn pin_current_thread_to(&self) {
        self.pal.pin_current_thread_to(&self.processors);
    }

    pub(crate) fn spawn_threads<E, R>(&self, entrypoint: E) -> Box<[thread::JoinHandle<R>]>
    where
        E: Fn(ProcessorCore<PAL>) -> R + Send + Clone + 'static,
        R: Send + 'static,
    {
        self.processors()
            .map(|p| {
                let processor = *p;
                let entrypoint = entrypoint.clone();

                thread::spawn(move || {
                    let set = Self::from(processor);
                    set.pin_current_thread_to();
                    entrypoint(processor)
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    }

    pub(crate) fn spawn_thread<E, R>(&self, entrypoint: E) -> thread::JoinHandle<R>
    where
        E: Fn(Self) -> R + Send + 'static,
        R: Send + 'static,
    {
        let set = self.clone();
        thread::spawn(move || {
            set.pin_current_thread_to();
            entrypoint(set)
        })
    }
}

impl<PAL: Platform> Clone for ProcessorSetCore<PAL> {
    fn clone(&self) -> Self {
        Self {
            processors: self.processors.clone(),
            pal: self.pal,
        }
    }
}

impl<PAL: Platform> From<ProcessorCore<PAL>> for ProcessorSetCore<PAL> {
    fn from(value: ProcessorCore<PAL>) -> Self {
        ProcessorSetCore::new(nonempty![value], value.pal)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, LazyLock,
    };

    use crate::{
        pal::{FakeProcessor, MockPlatform},
        EfficiencyClass,
    };

    use super::*;

    #[test]
    fn smoke_test() {
        static PAL: LazyLock<MockPlatform> = LazyLock::new(|| {
            let mut mock = MockPlatform::new();

            // Pin current thread to entire set.
            mock.expect_pin_current_thread_to_core()
                .withf(|p| p.len() == 2)
                .return_const(());

            // Pin spawned single thread to entire set.
            mock.expect_pin_current_thread_to_core()
                .withf(|p| p.len() == 2)
                .return_const(());

            // Pin spawned two threads, each to one processor.
            mock.expect_pin_current_thread_to_core()
                .withf(|p| p.len() == 1)
                .return_const(());

            mock.expect_pin_current_thread_to_core()
                .withf(|p| p.len() == 1)
                .return_const(());

            mock
        });

        let pal_processors = nonempty![
            FakeProcessor {
                index: 0,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Efficiency,
            },
            FakeProcessor {
                index: 1,
                memory_region: 0,
                efficiency_class: EfficiencyClass::Performance,
            }
        ];

        let processor_set =
            ProcessorSetCore::new(pal_processors.map(|p| ProcessorCore::new(p, &*PAL)), &*PAL);

        // Getters appear to get the expected values.
        assert_eq!(processor_set.len(), 2);

        // Iterator iterates through the expected stuff.
        let mut processor_iter = processor_set.processors();

        let p1 = processor_iter.next().unwrap();
        assert_eq!(p1.id(), 0);
        assert_eq!(p1.memory_region_id(), 0);
        assert_eq!(p1.efficiency_class(), EfficiencyClass::Efficiency);

        let p2 = processor_iter.next().unwrap();
        assert_eq!(p2.id(), 1);
        assert_eq!(p2.memory_region_id(), 0);
        assert_eq!(p2.efficiency_class(), EfficiencyClass::Performance);

        assert!(processor_iter.next().is_none());

        // Pin calls into PAL to execute the pinning.
        processor_set.pin_current_thread_to();

        // spawn_thread() spawns and pins a single thread.
        let threads_spawned = Arc::new(AtomicUsize::new(0));

        processor_set
            .spawn_thread({
                let threads_spawned = Arc::clone(&threads_spawned);
                move |processor_set| {
                    // Verify that we appear to have been given the expected processor set.
                    assert_eq!(processor_set.len(), 2);

                    threads_spawned.fetch_add(1, Ordering::Relaxed);
                }
            })
            .join()
            .unwrap();

        assert_eq!(threads_spawned.load(Ordering::Relaxed), 1);

        // spawn_threads() spawns multiple threads and pins each.
        let threads_spawned = Arc::new(AtomicUsize::new(0));

        processor_set
            .spawn_threads({
                let threads_spawned = Arc::clone(&threads_spawned);
                move |_| {
                    threads_spawned.fetch_add(1, Ordering::Relaxed);
                }
            })
            .into_vec()
            .into_iter()
            .for_each(|h| h.join().unwrap());

        assert_eq!(threads_spawned.load(Ordering::Relaxed), 2);

        // A clone appears to contain the same stuff.
        let cloned_processor_set = processor_set.clone();

        assert_eq!(cloned_processor_set.len(), 2);
    }
}
