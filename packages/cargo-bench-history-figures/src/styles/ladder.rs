//! The gate ladder: one bar per gate a candidate met, in the order the detector applies
//! them, showing what the gate computed against what it demanded.
//!
//! This is the appendix's answer to "why was this not reported?". A verdict alone teaches
//! nothing — the reader needs to see that seven gates passed and the eighth found the
//! move smaller than the series' own scatter. Because the detectors stop at the first
//! gate that vetoes, a ladder is naturally short when a candidate is rejected early, and
//! the gates below the veto are drawn as not reached rather than omitted, so the reader
//! learns that gates short-circuit.
//!
//! Each bar is normalised against its own threshold rather than against a shared scale,
//! because the quantities are incommensurable: a p-value, a percentage, a nanosecond
//! count and a probability of superiority cannot share an axis. What the figure compares
//! is therefore always the same thing — how far the computed value sits from the line the
//! gate draws — and the raw numbers are printed alongside so nothing is lost to the
//! normalisation.

use std::error::Error;

use plotters::element::{Circle, Rectangle, Text};
use plotters::prelude::ChartBuilder;
use plotters::style::{Color as _, RGBColor, ShapeStyle, TextStyle};

use crate::{canvas, coord, theme};

/// What a gate did with the candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// The candidate cleared the gate and moved on to the next one.
    Passed,

    /// The gate declined the candidate; nothing below it ran.
    Declined,

    /// The gate never ran, because an earlier one had already declined the candidate.
    NotReached,
}

impl Verdict {
    /// The colour a bar in this state is drawn in.
    fn color(self) -> RGBColor {
        match self {
            Self::Passed => theme::IMPROVEMENT,
            Self::Declined => theme::REGRESSION,
            Self::NotReached => theme::MUTED,
        }
    }
}

/// One gate's row in the ladder.
#[derive(Clone, Debug)]
pub struct Rung {
    /// The gate's name, as the appendix refers to it.
    pub gate: String,

    /// What the gate computed from the candidate, rendered for display so the figure
    /// carries the value in the metric's own units rather than a bare number.
    pub value: String,

    /// What the gate required, rendered the same way.
    pub threshold: String,

    /// How far the computed value sits from the threshold, as a multiple of the
    /// threshold. One means the candidate landed exactly on the line; above one it
    /// cleared with room to spare. `None` for a categorical gate that has no
    /// value/threshold pair to ratio — those rows draw a status marker instead of a bar.
    pub ratio: Option<f64>,

    /// What the gate did.
    pub verdict: Verdict,
}

/// A gate ladder for one candidate.
#[derive(Clone, Debug)]
pub struct Ladder {
    caption: String,
    rungs: Vec<Rung>,
}

/// The widest ratio a bar is drawn at.
///
/// A candidate can clear a gate by orders of magnitude — a decisive p-value against its
/// alpha, say — and drawing that to scale would squash every other bar into
/// invisibility. Bars are clamped here and the true figure stays in the printed value,
/// so an outlier costs the reader nothing.
const RATIO_CEILING: f64 = 3.0;

impl Ladder {
    /// An empty ladder captioned `caption`.
    #[must_use]
    pub fn new(caption: impl Into<String>) -> Self {
        Self {
            caption: caption.into(),
            rungs: Vec::new(),
        }
    }

    /// Appends a gate's row.
    #[must_use]
    pub fn rung(mut self, rung: Rung) -> Self {
        self.rungs.push(rung);
        self
    }

    /// Renders the ladder.
    #[must_use]
    pub fn render(&self) -> String {
        // Rows need a fixed height each rather than sharing a fixed figure height, or a
        // three-gate ladder would draw absurdly thick bars and a twelve-gate one would
        // crush its labels together.
        let row_height = 34_u32;
        let height = 70_u32.saturating_add(
            row_height.saturating_mul(u32::try_from(self.rungs.len()).unwrap_or(u32::MAX)),
        );

        canvas::draw(theme::WIDTH, height, |root| self.draw_into(root, height))
    }

    /// Draws the ladder into `root`.
    fn draw_into(
        &self,
        root: &plotters::drawing::DrawingArea<
            plotters::backend::SVGBackend<'_>,
            plotters::coord::Shift,
        >,
        _height: u32,
    ) -> Result<(), Box<dyn Error>> {
        let rows = self.rungs.len();
        let mut chart = ChartBuilder::on(root)
            .caption(
                self.caption.as_str(),
                (theme::FONT, theme::FONT_TITLE, &theme::INK),
            )
            .margin(12)
            .x_label_area_size(30)
            .y_label_area_size(10)
            .build_cartesian_2d(0.0_f64..RATIO_CEILING, 0.0_f64..coord::of(rows))?;

        chart
            .configure_mesh()
            .disable_y_mesh()
            .light_line_style(theme::INK.mix(0.0))
            .bold_line_style(theme::INK.mix(theme::GRID_OPACITY))
            .axis_style(theme::INK)
            .x_desc("computed value as a multiple of the gate's threshold")
            .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
            .y_labels(0)
            .draw()?;

        // The gate's demand is the same place on every row, which is what lets the
        // reader scan the column and see at a glance which bar fell short.
        chart.draw_series(std::iter::once(Rectangle::new(
            [(1.0, 0.0), (1.0, coord::of(rows))],
            ShapeStyle::from(theme::INK).stroke_width(2),
        )))?;

        for (index, rung) in self.rungs.iter().enumerate() {
            // Rows read top-down in application order, but the axis counts upward.
            let top = coord::of(rows.saturating_sub(index));
            let bottom = top - 1.0;
            let bar_top = top - 0.25;
            let bar_bottom = bottom + 0.35;
            let color = rung.verdict.color();

            match rung.ratio {
                Some(ratio) => {
                    let width = ratio.clamp(0.0, RATIO_CEILING);
                    chart.draw_series(std::iter::once(Rectangle::new(
                        [(0.0, bar_bottom), (width, bar_top)],
                        color.mix(0.55).filled(),
                    )))?;
                }
                None => {
                    // Off the ratio scale: a categorical gate has no value/threshold pair,
                    // so a bar would invent a magnitude it does not have.
                    let mid = f64::midpoint(bar_top, bar_bottom);
                    chart.draw_series(std::iter::once(Circle::new(
                        (0.15, mid),
                        theme::POINT_RADIUS,
                        color.filled(),
                    )))?;
                }
            }

            chart.draw_series(std::iter::once(Text::new(
                rung.gate.clone(),
                (0.25, bar_top),
                TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::INK),
            )))?;

            chart.draw_series(std::iter::once(Text::new(
                format!("{} vs {}", rung.value, rung.threshold),
                (0.25, bar_bottom + 0.05),
                TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&color),
            )))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Ladder {
        Ladder::new("a step that the series' own scatter explains")
            .rung(Rung {
                gate: "significance".to_owned(),
                value: "p = 0.004".to_owned(),
                threshold: "p < 0.05".to_owned(),
                ratio: Some(2.4),
                verdict: Verdict::Passed,
            })
            .rung(Rung {
                gate: "residual noise".to_owned(),
                value: "2.1 ns".to_owned(),
                threshold: "3.9 ns".to_owned(),
                ratio: Some(0.54),
                verdict: Verdict::Declined,
            })
            .rung(Rung {
                gate: "split located".to_owned(),
                value: "held".to_owned(),
                threshold: "must hold".to_owned(),
                ratio: None,
                verdict: Verdict::Passed,
            })
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn every_gate_is_labelled_with_its_value_and_threshold() {
        let svg = sample().render();

        assert!(svg.contains("residual noise"));
        assert!(svg.contains("2.1 ns vs 3.9 ns"));
        assert!(svg.contains("split located"));
        assert!(svg.contains("held vs must hold"));
    }

    /// A categorical row must not acquire a bar on the ratio scale; a numeric row must.
    #[test]
    fn a_categorical_row_and_a_ratio_row_are_distinct_shapes() {
        assert!(sample().rungs.iter().any(|rung| rung.ratio.is_some()));
        assert!(sample().rungs.iter().any(|rung| rung.ratio.is_none()));
    }

    /// A gate's threshold is often written with a comparison sign, which the SVG
    /// serializer escapes. The label still has to survive intact, since it is the figure's
    /// only record of what the gate actually demanded.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn a_decisive_pass_is_clamped_so_the_other_bars_stay_readable() {
        let ladder = Ladder::new("decisive").rung(Rung {
            gate: "significance".to_owned(),
            value: "p = 1e-9".to_owned(),
            threshold: "p < 0.05".to_owned(),
            ratio: Some(5_000_000.0),
            verdict: Verdict::Passed,
        });

        // Rendering must not fail or run off the axis; the true figure survives in the
        // printed value.
        let svg = ladder.render();

        assert!(svg.contains("p = 1e-9 vs p &lt; 0.05"));
    }

    #[test]
    fn the_three_verdicts_are_visually_distinct() {
        assert_ne!(Verdict::Passed.color(), Verdict::Declined.color());
        assert_ne!(Verdict::Passed.color(), Verdict::NotReached.color());
        assert_ne!(Verdict::Declined.color(), Verdict::NotReached.color());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn rendering_is_reproducible() {
        assert_eq!(sample().render(), sample().render());
    }
}
