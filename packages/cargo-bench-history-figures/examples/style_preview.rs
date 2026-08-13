//! Renders one figure in every style with placeholder data, for checking the visual
//! language of the catalogue itself rather than the correctness of any chapter's data.
//!
//! Run with `cargo run --package cargo-bench-history-figures --example style_preview`,
//! then open `target/style-preview.html`. The appendix's real figures are previewed with
//! `just book-figures-preview` instead; this example exists so a change to a *style* can
//! be judged without waiting on the data behind a chapter.

use cargo_bench_history_figures::assets::Asset;
use cargo_bench_history_figures::preview;
use cargo_bench_history_figures::styles::ladder::{Ladder, Rung, Verdict};
use cargo_bench_history_figures::styles::occupancy::{Cell, Occupancy};
use cargo_bench_history_figures::styles::operation::Operation;
use cargo_bench_history_figures::styles::plot::{Mark, Observation, Plot};
use cargo_bench_history_figures::theme;

fn main() {
    let stepped: Vec<f64> = vec![
        100.0, 101.5, 99.2, 100.8, 98.9, 101.1, 100.3, 99.6, 100.9, 100.2, 130.4, 131.8, 129.7,
        130.9, 131.2, 130.1, 129.8, 130.6, 131.4, 130.0,
    ];

    let regime = Plot::new("a step, with the split the detector located", 20)
        .value_label("ns")
        .values(&stepped)
        .band(0, 9, "before", theme::HIGHLIGHT)
        .band(10, 19, "after", theme::REGRESSION)
        .split(10, "change point")
        .rule(100.3, "baseline", theme::HIGHLIGHT)
        .rule(130.5, "latest", theme::REGRESSION);

    let ramp: Vec<f64> = (0..20).map(|index| 100.0 * 1.005_f64.powi(index)).collect();
    let trend = Plot::new("a slow drift, with the fitted line", 20)
        .value_label("ns")
        .values(&ramp)
        .rule(100.0, "fitted start", theme::MUTED)
        .rule(110.0, "fitted end", theme::REGRESSION)
        .note(6, 106.0, "+10.4% across the window", theme::REGRESSION);

    let gappy = Plot::new("a gap, and a tip with no recent observation", 20)
        .value_label("instructions")
        .observations((0..8_u16).map(|index| {
            Observation::new(usize::from(index), 1000.0 + f64::from(index))
        }))
        .observations((14..17_u16).map(|index| {
            Observation::new(usize::from(index), 1009.0 + f64::from(index))
        }));

    let ghost_before = Plot::new("series as reconstructed", 20)
        .value_label("bytes")
        .values(&stepped);
    let ghost_after = Plot::new("after the ghost filter", 20)
        .value_label("bytes")
        .observations(
            stepped
                .iter()
                .enumerate()
                .map(|(index, &value)| Observation::new(index, value).marked(Mark::Removed)),
        );
    let operation = Operation::new(
        "a benchmark absent at the analyzed commit is dropped entirely",
        ghost_before,
        ghost_after,
    );

    let ladder = Ladder::new("a move the series' own scatter explains")
        .rung(Rung {
            gate: "minimum regime".to_owned(),
            value: "10 and 10".to_owned(),
            threshold: "5 each".to_owned(),
            ratio: 2.0,
            verdict: Verdict::Passed,
        })
        .rung(Rung {
            gate: "significance".to_owned(),
            value: "p = 0.004".to_owned(),
            threshold: "p < 0.05".to_owned(),
            ratio: 2.6,
            verdict: Verdict::Passed,
        })
        .rung(Rung {
            gate: "relative floor".to_owned(),
            value: "4.1%".to_owned(),
            threshold: "3.0%".to_owned(),
            ratio: 1.37,
            verdict: Verdict::Passed,
        })
        .rung(Rung {
            gate: "absolute floor".to_owned(),
            value: "4.1 ns".to_owned(),
            threshold: "1.0 ns".to_owned(),
            ratio: 2.9,
            verdict: Verdict::Passed,
        })
        .rung(Rung {
            gate: "residual noise".to_owned(),
            value: "4.1 ns".to_owned(),
            threshold: "7.6 ns".to_owned(),
            ratio: 0.54,
            verdict: Verdict::Declined,
        })
        .rung(Rung {
            gate: "regime separation".to_owned(),
            value: "not reached".to_owned(),
            threshold: "0.85".to_owned(),
            ratio: 0.0,
            verdict: Verdict::NotReached,
        });

    let occupancy = Occupancy::new("what the store holds, by partition")
        .row(
            "criterion / x86_64 / a1b2c3d4",
            (0..20).map(|index| match index {
                5 | 6 => Cell::Absent,
                19 => Cell::Dirty,
                _ => Cell::Clean,
            }),
        )
        .row(
            "criterion / x86_64 / 9f8e7d6c",
            (0..20).map(|index| if index < 12 { Cell::Absent } else { Cell::Excluded }),
        )
        .row(
            "callgrind / x86_64 / a1b2c3d4",
            (0..20).map(|index| if index % 3 == 0 { Cell::Focus } else { Cell::Clean }),
        );

    let assets = vec![
        Asset::new("style/regime.svg", regime.render()),
        Asset::new("style/trend.svg", trend.render()),
        Asset::new("style/gaps.svg", gappy.render()),
        Asset::new("style/operation.svg", operation.render()),
        Asset::new("style/ladder.svg", ladder.render()),
        Asset::new("style/occupancy.svg", occupancy.render()),
    ];

    std::fs::write("target/style-preview.html", preview::page(&assets)).unwrap();
    println!("wrote target/style-preview.html");
}
