#![expect(
    missing_docs,
    clippy::undocumented_unsafe_blocks,
    clippy::cast_possible_truncation,
    reason = "benchmarks"
)]

use std::cell::UnsafeCell;
use std::hint::black_box;
use std::mem::MaybeUninit;
use std::pin::pin;
use std::task::Waker;
use std::time::Instant;
use std::{iter, task};

use criterion::{Criterion, criterion_group, criterion_main};
use events_once_core::Event;

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("events_once_core_sync");

    g.bench_function("send_receive", |b| {
        b.iter_custom(|iterations| {
            let mut events = iter::repeat_with(|| {
                Box::pin(UnsafeCell::new(MaybeUninit::<Event<i32>>::uninit()))
            })
            .take(iterations as usize)
            .collect::<Vec<_>>();

            let endpoints = events
                .iter_mut()
                .map(|event| unsafe { Event::placed(event.as_mut()) })
                .collect::<Vec<_>>();

            let start = Instant::now();

            for (sender, receiver) in endpoints {
                let mut receiver = pin!(receiver);

                sender.send(42);

                let mut cx = task::Context::from_waker(Waker::noop());
                _ = black_box(receiver.as_mut().poll(&mut cx));
            }

            start.elapsed()
        });
    });

    g.bench_function("poll_connected", |b| {
        b.iter_custom(|iterations| {
            let mut events = iter::repeat_with(|| {
                Box::pin(UnsafeCell::new(MaybeUninit::<Event<i32>>::uninit()))
            })
            .take(iterations as usize)
            .collect::<Vec<_>>();

            let endpoints = events
                .iter_mut()
                .map(|event| unsafe { Event::placed(event.as_mut()) })
                .collect::<Vec<_>>();

            let (_senders, receivers) = endpoints.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();

            let mut receivers = receivers.into_iter().map(Box::pin).collect::<Vec<_>>();

            // In the measured part, we poll each receiver once.
            // This will set the awaiter and return Pending.
            let start = Instant::now();

            let mut cx = task::Context::from_waker(Waker::noop());

            for receiver in &mut receivers {
                _ = receiver.as_mut().poll(&mut cx);
            }

            start.elapsed()
        });
    });

    g.bench_function("poll_disconnected", |b| {
        b.iter_custom(|iterations| {
            let mut events = iter::repeat_with(|| {
                Box::pin(UnsafeCell::new(MaybeUninit::<Event<i32>>::uninit()))
            })
            .take(iterations as usize)
            .collect::<Vec<_>>();

            let endpoints = events
                .iter_mut()
                .map(|event| unsafe { Event::placed(event.as_mut()) })
                .collect::<Vec<_>>();

            let (senders, receivers) = endpoints.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();

            // The senders are all disconnected, so the poll will actually clean up the event.
            drop(senders);

            let mut receivers = receivers.into_iter().map(Box::pin).collect::<Vec<_>>();

            // In the measured part, we poll each receiver once.
            // This will detect disconnection and clean up the events.
            let start = Instant::now();

            let mut cx = task::Context::from_waker(Waker::noop());

            for receiver in &mut receivers {
                _ = receiver.as_mut().poll(&mut cx);
            }

            start.elapsed()
        });
    });

    g.bench_function("set_connected", |b| {
        b.iter_custom(|iterations| {
            let mut events = iter::repeat_with(|| {
                Box::pin(UnsafeCell::new(MaybeUninit::<Event<i32>>::uninit()))
            })
            .take(iterations as usize)
            .collect::<Vec<_>>();

            let endpoints = events
                .iter_mut()
                .map(|event| unsafe { Event::placed(event.as_mut()) })
                .collect::<Vec<_>>();

            let (senders, _receivers) = endpoints.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();

            // In the measured part, we simply set the value for each event.
            let start = Instant::now();

            for sender in senders {
                sender.send(42);
            }

            start.elapsed()
        });
    });

    g.bench_function("set_disconnected", |b| {
        b.iter_custom(|iterations| {
            let mut events = iter::repeat_with(|| {
                Box::pin(UnsafeCell::new(MaybeUninit::<Event<i32>>::uninit()))
            })
            .take(iterations as usize)
            .collect::<Vec<_>>();

            let endpoints = events
                .iter_mut()
                .map(|event| unsafe { Event::placed(event.as_mut()) })
                .collect::<Vec<_>>();

            let (senders, receivers) = endpoints.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();

            // The receivers are all disconnected, so the "set" operations will clean up the event.
            drop(receivers);

            // In the measured part, we simply set the value for each event (and clean up).
            let start = Instant::now();

            for sender in senders {
                sender.send(42);
            }

            start.elapsed()
        });
    });

    g.finish();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
