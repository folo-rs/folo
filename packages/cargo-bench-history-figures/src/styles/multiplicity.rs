//! Figures for the multiplicity chapter: the step-up staircase and the census bar.
//!
//! Both exist to make a group-level argument visible. A reader can follow "test enough
//! things and something will look surprising" as a sentence, but not act on it; seeing
//! twelve candidates sorted against a rising bar, with the cut falling where it does,
//! turns it into a rule they can apply to their own report.

use std::error::Error;

use plotters::element::{Circle, PathElement, Rectangle, Text};
use plotters::prelude::ChartBuilder;
use plotters::style::{Color as _, RGBColor, ShapeStyle, TextStyle};

use crate::{canvas, coord, theme};

/// One candidate's place in the step-up procedure.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// How the appendix names the series.
    pub label: String,

    /// The candidate's chance level.
    pub chance_level: f64,

    /// The bar this candidate's rank had to clear.
    pub threshold: f64,

    /// Whether the procedure kept it.
    pub kept: bool,
}

/// The step-up staircase: candidates sorted by chance level against the rising bar.
#[derive(Clone, Debug)]
pub struct Staircase {
    caption: String,
    candidates: Vec<Candidate>,
}

impl Staircase {
    /// An empty staircase captioned `caption`.
    #[must_use]
    pub fn new(caption: impl Into<String>) -> Self {
        Self {
            caption: caption.into(),
            candidates: Vec::new(),
        }
    }

    /// Adds a candidate. Order does not matter; the figure sorts by chance level, which
    /// is the order the procedure itself works in.
    #[must_use]
    pub fn candidate(mut self, candidate: Candidate) -> Self {
        self.candidates.push(candidate);
        self
    }

    /// Renders the staircase.
    #[must_use]
    pub fn render(&self) -> String {
        let mut sorted = self.candidates.clone();
        sorted.sort_by(|left, right| {
            left.chance_level
                .partial_cmp(&right.chance_level)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        canvas::draw(theme::WIDTH, theme::HEIGHT, |root| {
            let count = sorted.len();
            let ceiling = sorted
                .iter()
                .flat_map(|candidate| [candidate.chance_level, candidate.threshold])
                .fold(0.0_f64, f64::max)
                .max(f64::MIN_POSITIVE);

            let mut chart = ChartBuilder::on(root)
                .caption(
                    self.caption.as_str(),
                    (theme::FONT, theme::FONT_TITLE, &theme::INK),
                )
                .margin(12)
                .x_label_area_size(34)
                .y_label_area_size(62)
                .build_cartesian_2d(0.5_f64..coord::of(count) + 0.5, 0.0_f64..ceiling * 1.15)?;

            chart
                .configure_mesh()
                .light_line_style(theme::INK.mix(0.0))
                .bold_line_style(theme::INK.mix(theme::GRID_OPACITY))
                .axis_style(theme::INK)
                .x_desc("rank, by chance level")
                .y_desc("chance level")
                .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
                .draw()?;

            // The bar rises with rank: the strongest candidate must clear the strictest
            // bar, and each one after it a slightly looser one. Drawing it as a line
            // rather than as per-point marks is what makes the "staircase" shape — and
            // the point where the candidates cross it — legible at a glance.
            let bar: Vec<(f64, f64)> = sorted
                .iter()
                .enumerate()
                .map(|(index, candidate)| (coord::of(index) + 1.0, candidate.threshold))
                .collect();
            chart.draw_series(std::iter::once(PathElement::new(
                bar,
                ShapeStyle::from(theme::INK).stroke_width(2),
            )))?;

            for (index, candidate) in sorted.iter().enumerate() {
                let at = (coord::of(index) + 1.0, candidate.chance_level);
                let color = if candidate.kept {
                    theme::REGRESSION
                } else {
                    theme::MUTED
                };
                chart.draw_series(std::iter::once(Circle::new(
                    at,
                    theme::POINT_RADIUS + 1,
                    color.filled(),
                )))?;

                if !candidate.kept {
                    // A dropped candidate is struck through rather than merely dimmed:
                    // the figure's whole subject is which ones the procedure discards.
                    let tick = ceiling * 0.02;
                    chart.draw_series(std::iter::once(PathElement::new(
                        vec![(at.0 - 0.22, at.1 - tick), (at.0 + 0.22, at.1 + tick)],
                        ShapeStyle::from(color).stroke_width(2),
                    )))?;
                }
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }
}

/// One slice of the census: how many series ended in a given state.
#[derive(Clone, Debug)]
pub struct Slice {
    /// What the state is called in a report.
    pub label: String,

    /// How many series are in it.
    pub count: usize,

    /// The slice's colour.
    pub color: RGBColor,
}

/// The census bar: every series in the analysis, sorted into judged and the reasons the
/// rest were not.
///
/// Drawn as one bar rather than as a table because the question it answers is
/// proportional — "how much of my suite did this report actually cover?" — and a
/// proportion is the one thing a table of counts communicates badly.
#[derive(Clone, Debug)]
pub struct Census {
    caption: String,
    slices: Vec<Slice>,
}

impl Census {
    /// An empty census bar captioned `caption`.
    #[must_use]
    pub fn new(caption: impl Into<String>) -> Self {
        Self {
            caption: caption.into(),
            slices: Vec::new(),
        }
    }

    /// Appends a slice, left to right.
    #[must_use]
    pub fn slice(mut self, label: impl Into<String>, count: usize, color: RGBColor) -> Self {
        self.slices.push(Slice {
            label: label.into(),
            count,
            color,
        });
        self
    }

    /// Renders the bar.
    #[must_use]
    pub fn render(&self) -> String {
        let total: usize = self.slices.iter().map(|slice| slice.count).sum();
        // Legend entries stack below the bar, so the figure grows with the number of
        // states rather than crushing them together.
        let height = 120_u32.saturating_add(
            18_u32.saturating_mul(u32::try_from(self.slices.len()).unwrap_or(u32::MAX)),
        );

        canvas::draw(theme::WIDTH, height, |root| {
            let mut chart = ChartBuilder::on(root)
                .caption(
                    self.caption.as_str(),
                    (theme::FONT, theme::FONT_TITLE, &theme::INK),
                )
                .margin(12)
                .x_label_area_size(34)
                .y_label_area_size(10)
                .build_cartesian_2d(0.0_f64..coord::of(total.max(1)), 0.0_f64..1.0_f64)?;

            chart
                .configure_mesh()
                .disable_y_mesh()
                .light_line_style(theme::INK.mix(0.0))
                .bold_line_style(theme::INK.mix(theme::GRID_OPACITY))
                .axis_style(theme::INK)
                .x_desc("series")
                .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
                .y_labels(0)
                .draw()?;

            let mut cursor = 0.0_f64;
            for slice in &self.slices {
                let width = coord::of(slice.count);
                chart.draw_series(std::iter::once(Rectangle::new(
                    [(cursor, 0.62), (cursor + width, 0.95)],
                    slice.color.mix(0.75).filled(),
                )))?;
                cursor += width;
            }

            // The legend carries the counts, because a slice can be too narrow to label
            // in place and a legend that omits the number would leave the reader
            // measuring pixels.
            for (index, slice) in self.slices.iter().enumerate() {
                let row = 0.5 - (coord::of(index) * 0.11);
                chart.draw_series(std::iter::once(Rectangle::new(
                    [(0.0, row), (coord::of(total.max(1)) * 0.02, row + 0.06)],
                    slice.color.mix(0.75).filled(),
                )))?;
                chart.draw_series(std::iter::once(Text::new(
                    format!("{} — {}", slice.label, slice.count),
                    (coord::of(total.max(1)) * 0.03, row + 0.06),
                    TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::INK),
                )))?;
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staircase() -> Staircase {
        Staircase::new("twelve candidates against the rising bar")
            .candidate(Candidate {
                label: "a".to_owned(),
                chance_level: 0.001,
                threshold: 0.008,
                kept: true,
            })
            .candidate(Candidate {
                label: "b".to_owned(),
                chance_level: 0.012,
                threshold: 0.016,
                kept: true,
            })
            .candidate(Candidate {
                label: "c".to_owned(),
                chance_level: 0.040,
                threshold: 0.025,
                kept: false,
            })
    }

    #[test]
    fn the_staircase_renders_every_candidate() {
        let svg = staircase().render();

        assert!(svg.contains("rank, by chance level"));
        assert!(svg.contains("chance level"));
    }

    #[test]
    fn the_staircase_is_reproducible_regardless_of_input_order() {
        let forward = staircase().render();
        let reversed = Staircase::new("twelve candidates against the rising bar")
            .candidate(Candidate {
                label: "c".to_owned(),
                chance_level: 0.040,
                threshold: 0.025,
                kept: false,
            })
            .candidate(Candidate {
                label: "b".to_owned(),
                chance_level: 0.012,
                threshold: 0.016,
                kept: true,
            })
            .candidate(Candidate {
                label: "a".to_owned(),
                chance_level: 0.001,
                threshold: 0.008,
                kept: true,
            })
            .render();

        assert_eq!(
            forward, reversed,
            "the procedure sorts by chance level, so the figure must not depend on input order"
        );
    }

    #[test]
    fn the_census_legend_carries_every_count() {
        let svg = Census::new("what this report judged")
            .slice("judged", 8, theme::HIGHLIGHT)
            .slice("too few points", 2, theme::MUTED)
            .slice("ghost", 1, theme::ALTERNATE)
            .render();

        assert!(svg.contains("judged — 8"));
        assert!(svg.contains("too few points — 2"));
        assert!(svg.contains("ghost — 1"));
    }

    #[test]
    fn an_empty_census_still_renders() {
        let svg = Census::new("nothing at all").render();

        assert!(svg.contains("nothing at all"));
    }
}
