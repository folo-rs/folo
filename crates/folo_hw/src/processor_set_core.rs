use std::thread;

use nonempty::{nonempty, NonEmpty};

use crate::{pal::PlatformCommon, ProcessorCore, ProcessorSetBuilderCore};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

#[derive(Clone, Debug)]
pub(crate) struct ProcessorSetCore<PAL>
where
    PAL: PlatformCommon,
{
    processors: NonEmpty<ProcessorCore<PAL>>,
    pub(crate) pal: &'static PAL,
}

impl<PAL> ProcessorSetCore<PAL>
where
    PAL: PlatformCommon,
{
    pub fn to_builder(&self) -> ProcessorSetBuilderCore<PAL> {
        ProcessorSetBuilderCore::<PAL>::new(self.pal).filter(|p| self.processors.contains(p))
    }

    pub(crate) fn new(processors: NonEmpty<ProcessorCore<PAL>>, pal: &'static PAL) -> Self {
        Self { processors, pal }
    }

    pub fn len(&self) -> usize {
        self.processors.len()
    }

    pub fn processors(&self) -> impl Iterator<Item = &ProcessorCore<PAL>> + '_ {
        self.processors.iter()
    }

    pub fn pin_current_thread_to(&self) {
        self.pal.pin_current_thread_to(&self.processors);
    }

    pub fn spawn_threads<E, R>(&self, entrypoint: E) -> Box<[thread::JoinHandle<R>]>
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

    pub fn spawn_thread<E, R>(&self, entrypoint: E) -> thread::JoinHandle<R>
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

impl<PAL> From<ProcessorCore<PAL>> for ProcessorSetCore<PAL>
where
    PAL: PlatformCommon,
{
    fn from(value: ProcessorCore<PAL>) -> Self {
        ProcessorSetCore::new(nonempty![value], value.pal)
    }
}
