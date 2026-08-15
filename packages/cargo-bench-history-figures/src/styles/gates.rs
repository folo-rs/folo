//! Figures for the noise-gate chapter: the residual strip, the agreement grid, and the
//! interval plot.
//!
//! Each gate asks a different question of the same candidate, and each of these figures
//! makes one of those questions visible. They are separate styles rather than variants of
//! the series plot because none of them is about movement through history: a residual
//! strip is about spread around a model, an agreement grid is about pairings, and an
//! interval plot is about overlap.

use std::error::Error;

use plotters::element::{Circle, PathElement, Rectangle, Text};
use plotters::prelude::ChartBuilder;
use plotters::style::{Color as _, ShapeStyle, TextStyle};

use crate::{canvas, coord, theme};

/// How far each observation sits from the model fitted to the series, against the band
/// the residual gate treats as ordinary.
///
/// This is the figure that answers "how big does a move have to be before it stands out
/// from what this series does anyway?" — which is the question the residual gate exists to
/// ask, and the one readers most often expect a percentage to answer.
#[derive(Clone, Debug)]
pub struct Residuals {
    caption: String,
    distances: Vec<f64>,
    band: f64,
    move_size: Option<f64>,
}

impl Residuals {
    /// A residual strip captioned `caption`, with each observation's distance from the
    /// fitted model and the band the gate considers ordinary.
    #[must_use]
    pub fn new(caption: impl Into<String>, distances: Vec<f64>, band: f64) -> Self {
        Self {
            caption: caption.into(),
            distances,
            band,
            move_size: None,
        }
    }

    /// Marks the size of the move being judged, so the reader can see it against the band
    /// rather than being told the comparison's outcome.
    #[must_use]
    pub fn move_size(mut self, size: f64) -> Self {
        self.move_size = Some(size);
        self
    }

    /// Renders the strip.
    #[must_use]
    pub fn render(&self) -> String {
        canvas::draw(theme::WIDTH, 240, |root| {
            let extent = self
                .distances
                .iter()
                .map(|distance| distance.abs())
                .chain(std::iter::once(self.band))
                .chain(self.move_size.map(f64::abs))
                .fold(0.0_f64, f64::max)
                .max(f64::MIN_POSITIVE);
            let count = self.distances.len();

            let mut chart = ChartBuilder::on(root)
                .caption(
                    self.caption.as_str(),
                    (theme::FONT, theme::FONT_TITLE, &theme::INK),
                )
                .margin(12)
                .x_label_area_size(34)
                .y_label_area_size(62)
                .build_cartesian_2d(
                    -0.5_f64..coord::of(count) - 0.5,
                    -extent * 1.2..extent * 1.2,
                )?;

            chart
                .configure_mesh()
                .light_line_style(theme::INK.mix(0.0))
                .bold_line_style(theme::INK.mix(theme::GRID_OPACITY))
                .axis_style(theme::INK)
                .x_desc("commit position")
                .y_desc("distance from the fitted model")
                .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
                .draw()?;

            chart.draw_series(std::iter::once(Rectangle::new(
                [(-0.5, -self.band), (coord::of(count) - 0.5, self.band)],
                theme::MUTED.mix(theme::BAND_OPACITY).filled(),
            )))?;
            chart.draw_series(std::iter::once(Text::new(
                "what this series does anyway".to_owned(),
                (-0.3, self.band),
                TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::MUTED),
            )))?;

            for (index, &residual) in self.distances.iter().enumerate() {
                let at = coord::of(index);
                // Drawn as a stem from zero rather than as a bare point, because the
                // quantity is a distance and a point alone reads as a value.
                chart.draw_series(std::iter::once(PathElement::new(
                    vec![(at, 0.0), (at, residual)],
                    ShapeStyle::from(theme::HIGHLIGHT).stroke_width(1),
                )))?;
                chart.draw_series(std::iter::once(Circle::new(
                    (at, residual),
                    theme::POINT_RADIUS - 1,
                    theme::HIGHLIGHT.filled(),
                )))?;
            }

            if let Some(size) = self.move_size {
                chart.draw_series(std::iter::once(PathElement::new(
                    vec![(-0.5, size), (coord::of(count) - 0.5, size)],
                    ShapeStyle::from(theme::REGRESSION).stroke_width(2),
                )))?;
                chart.draw_series(std::iter::once(Text::new(
                    "the move being judged".to_owned(),
                    (-0.3, size),
                    TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::REGRESSION),
                )))?;
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }
}

/// Every before-and-after pairing, and whether each agrees the level moved.
///
/// The agreement gate is the one readers find least intuitive, because a significance test
/// has already passed by the time it runs. Drawing the pairings shows why the two are not
/// the same question: a series that oscillates between two levels produces a grid visibly
/// speckled with disagreement, whatever its chance level says.
#[derive(Clone, Debug)]
pub struct Agreement {
    caption: String,
    before: Vec<f64>,
    after: Vec<f64>,
}

impl Agreement {
    /// An agreement grid for the two samples.
    #[must_use]
    pub fn new(caption: impl Into<String>, before: Vec<f64>, after: Vec<f64>) -> Self {
        Self {
            caption: caption.into(),
            before,
            after,
        }
    }

    /// The share of pairings that agree the level rose.
    ///
    /// A tie counts as half, matching how the rank comparison itself treats one.
    #[must_use]
    pub fn share(&self) -> f64 {
        let pairs = self.before.len().saturating_mul(self.after.len());
        if pairs == 0 {
            return 0.0;
        }
        let agreeing: f64 = self
            .before
            .iter()
            .flat_map(|earlier| {
                self.after.iter().map(move |later| {
                    if later > earlier {
                        1.0
                    } else if (later - earlier).abs() < f64::EPSILON {
                        0.5
                    } else {
                        0.0
                    }
                })
            })
            .sum();
        agreeing / coord::of(pairs)
    }

    /// Renders the grid.
    #[must_use]
    pub fn render(&self) -> String {
        let rows = self.after.len();
        let columns = self.before.len();
        let height =
            90_u32.saturating_add(22_u32.saturating_mul(u32::try_from(rows).unwrap_or(u32::MAX)));

        canvas::draw(theme::WIDTH, height, |root| {
            let mut chart = ChartBuilder::on(root)
                .caption(
                    self.caption.as_str(),
                    (theme::FONT, theme::FONT_TITLE, &theme::INK),
                )
                .margin(12)
                .x_label_area_size(34)
                .y_label_area_size(70)
                .build_cartesian_2d(
                    0.0_f64..coord::of(columns.max(1)),
                    0.0_f64..coord::of(rows.max(1)),
                )?;

            chart
                .configure_mesh()
                .disable_mesh()
                .axis_style(theme::INK)
                .x_desc("each observation from before the change")
                .y_desc("and after")
                .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
                .x_labels(0)
                .y_labels(0)
                .draw()?;

            for (row, later) in self.after.iter().enumerate() {
                for (column, earlier) in self.before.iter().enumerate() {
                    let agrees = later > earlier;
                    let color = if agrees {
                        theme::REGRESSION
                    } else {
                        theme::MUTED
                    };
                    chart.draw_series(std::iter::once(Rectangle::new(
                        [
                            (coord::of(column) + 0.08, coord::of(row) + 0.08),
                            (coord::of(column) + 0.92, coord::of(row) + 0.92),
                        ],
                        color.mix(if agrees { 0.7 } else { 0.35 }).filled(),
                    )))?;
                }
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn the_residual_band_is_labelled_with_what_it_represents() {
        let svg = Residuals::new("scatter about the step model", vec![1.0, -2.0, 0.5], 3.0)
            .move_size(4.0)
            .render();

        assert!(svg.contains("what this series does anyway"));
        assert!(svg.contains("the move being judged"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn a_move_beyond_the_band_stays_on_the_axis() {
        let strip = Residuals::new("dominant move", vec![0.1, -0.1], 0.2).move_size(50.0);

        // The move is what the figure exists to compare against the band, so it must not
        // be scaled off the plot.
        assert!(strip.render().contains("the move being judged"));
    }

    #[test]
    fn cleanly_separated_samples_agree_completely() {
        let grid = Agreement::new("separated", vec![100.0, 101.0], vec![130.0, 131.0]);

        assert!((grid.share() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn an_oscillating_series_leaves_disagreeing_pairs() {
        let grid = Agreement::new("bimodal", vec![100.0, 130.0], vec![100.0, 130.0]);

        let share = grid.share();

        assert!(
            share < 0.85,
            "a series that revisits both levels must not reach the agreement floor, got {share}"
        );
    }

    #[test]
    fn an_empty_sample_agrees_with_nothing() {
        let grid = Agreement::new("empty", Vec::new(), vec![1.0]);

        assert!((grid.share() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn rendering_is_reproducible() {
        let grid = Agreement::new("separated", vec![100.0, 101.0], vec![130.0, 131.0]);

        assert_eq!(grid.render(), grid.render());
    }
}
