//! Figures for the noise-gate chapter: the residual strip, the agreement grid, and the
//! interval plot.
//!
//! Each gate asks a different question of the same candidate, and each of these figures
//! makes one of those questions visible. They are separate styles rather than variants of
//! the series plot because none of them is about movement through history: a residual
//! strip is about spread around a model, an agreement grid is about pairings, and an
//! interval plot is about overlap.

use std::cmp::Ordering;
use std::error::Error;

use cbh_stats::MannWhitneyU;
use plotters::element::{Circle, PathElement, Rectangle, Text};
use plotters::prelude::ChartBuilder;
use plotters::style::{Color as _, RGBColor, ShapeStyle, TextStyle};

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
    values: Vec<f64>,
    band: f64,
    move_size: Option<f64>,
}

impl Residuals {
    /// A residual strip captioned `caption`, with each observation's signed residual
    /// from the fitted model and the band the gate considers ordinary.
    #[must_use]
    pub fn new(caption: impl Into<String>, residuals: Vec<f64>, band: f64) -> Self {
        Self {
            caption: caption.into(),
            values: residuals,
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
                .values
                .iter()
                .map(|residual| residual.abs())
                .chain(std::iter::once(self.band))
                .chain(self.move_size.map(f64::abs))
                .fold(0.0_f64, f64::max)
                .max(f64::MIN_POSITIVE);
            let count = self.values.len();

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
                .y_desc("signed residual from the fitted model")
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

            for (index, &residual) in self.values.iter().enumerate() {
                let at = coord::of(index);
                // Drawn as a stem from zero rather than as a bare point, because the
                // quantity is a signed residual and a point alone reads as a value.
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
    /// Read from [`MannWhitneyU::superiority`], so the caption and the detector's
    /// regime-separation statistic cannot drift apart. A tie counts as half.
    #[must_use]
    pub fn share(&self) -> f64 {
        MannWhitneyU::new(&self.before, &self.after).map_or(0.0, |test| test.superiority())
    }

    /// How each pairing classifies under exact comparison, in draw order (later row,
    /// earlier column).
    fn classifications(&self) -> impl Iterator<Item = PairClass> + '_ {
        self.after.iter().flat_map(|later| {
            self.before
                .iter()
                .map(move |earlier| PairClass::of(*earlier, *later))
        })
    }

    /// Renders the grid.
    #[must_use]
    pub fn render(&self) -> String {
        let rows = self.after.len();
        let columns = self.before.len();
        let height =
            110_u32.saturating_add(22_u32.saturating_mul(u32::try_from(rows).unwrap_or(u32::MAX)));

        canvas::draw(theme::WIDTH, height, |root| {
            let columns_f = coord::of(columns.max(1));
            let rows_f = coord::of(rows.max(1));
            let mut chart = ChartBuilder::on(root)
                .caption(
                    self.caption.as_str(),
                    (theme::FONT, theme::FONT_TITLE, &theme::INK),
                )
                .margin(12)
                .x_label_area_size(34)
                .y_label_area_size(70)
                .build_cartesian_2d(0.0_f64..columns_f, -0.55_f64..rows_f)?;

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

            debug_assert!(
                {
                    let pairs = columns.saturating_mul(rows);
                    pairs == 0 || {
                        let credits: f64 = self.classifications().map(PairClass::credit).sum();
                        let share = credits / f64::from(u32::try_from(pairs).unwrap_or(u32::MAX));
                        (share - self.share()).abs() < f64::EPSILON
                    }
                },
                "rendered pair credits must equal MannWhitneyU::superiority"
            );

            for (index, class) in self.classifications().enumerate() {
                let column = index
                    .checked_rem(columns.max(1))
                    .expect("columns is non-zero after max(1)");
                let row = index
                    .checked_div(columns.max(1))
                    .expect("columns is non-zero after max(1)");
                let left = coord::of(column) + 0.08;
                let right = coord::of(column) + 0.92;
                let bottom = coord::of(row) + 0.08;
                let top = coord::of(row) + 0.92;
                match class {
                    PairClass::Greater => {
                        chart.draw_series(std::iter::once(Rectangle::new(
                            [(left, bottom), (right, top)],
                            theme::REGRESSION.mix(0.7).filled(),
                        )))?;
                    }
                    PairClass::Less => {
                        chart.draw_series(std::iter::once(Rectangle::new(
                            [(left, bottom), (right, top)],
                            theme::MUTED.mix(0.35).filled(),
                        )))?;
                    }
                    PairClass::Equal => {
                        // Half-credit: only the left half is filled, so a tie cannot
                        // be read as either a full agreement or a full disagreement.
                        let mid = f64::midpoint(left, right);
                        chart.draw_series(std::iter::once(Rectangle::new(
                            [(left, bottom), (mid, top)],
                            theme::HIGHLIGHT.mix(0.7).filled(),
                        )))?;
                        chart.draw_series(std::iter::once(Rectangle::new(
                            [(mid, bottom), (right, top)],
                            ShapeStyle::from(theme::HIGHLIGHT).stroke_width(1),
                        )))?;
                    }
                }
            }

            let classes = [PairClass::Greater, PairClass::Equal, PairClass::Less];
            let width = columns_f / coord::of(classes.len());
            for (index, class) in classes.into_iter().enumerate() {
                let left = coord::of(index) * width + 0.08;
                let swatch_right = left + 0.22;
                chart.draw_series(std::iter::once(Rectangle::new(
                    [(left, -0.45), (swatch_right, -0.15)],
                    class.color().mix(0.7).filled(),
                )))?;
                chart.draw_series(std::iter::once(Text::new(
                    class.label().to_owned(),
                    (swatch_right + 0.05, -0.45),
                    TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::INK),
                )))?;
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }
}

/// How one before/after pairing classifies under exact comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairClass {
    Greater,
    Equal,
    Less,
}

impl PairClass {
    fn of(earlier: f64, later: f64) -> Self {
        match later.partial_cmp(&earlier) {
            Some(Ordering::Greater) => Self::Greater,
            Some(Ordering::Equal) => Self::Equal,
            Some(Ordering::Less) | None => Self::Less,
        }
    }

    fn credit(self) -> f64 {
        match self {
            Self::Greater => 1.0,
            Self::Equal => 0.5,
            Self::Less => 0.0,
        }
    }

    fn color(self) -> RGBColor {
        match self {
            Self::Greater => theme::REGRESSION,
            Self::Equal => theme::HIGHLIGHT,
            Self::Less => theme::MUTED,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Greater => "later greater",
            Self::Equal => "tie (half)",
            Self::Less => "later less",
        }
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
        assert!(svg.contains("signed residual from the fitted model"));
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
    fn a_pair_classifies_by_exact_ordering() {
        assert_eq!(PairClass::of(1.0, 2.0), PairClass::Greater);
        assert_eq!(PairClass::of(2.0, 2.0), PairClass::Equal);
        assert_eq!(PairClass::of(3.0, 2.0), PairClass::Less);
    }

    /// The caption aggregate is the production statistic, so the cells that produce it
    /// must credit the same way: greater = 1, equal = ½, less = 0.
    #[test]
    fn rendered_classifications_sum_to_the_production_aggregate() {
        let grid = Agreement::new("ties", vec![100.0, 110.0], vec![100.0, 120.0]);
        let credits: f64 = grid.classifications().map(PairClass::credit).sum();
        let pairs = grid.classifications().count();
        let share = credits / f64::from(u32::try_from(pairs).expect("a figure holds few pairs"));

        assert!((share - grid.share()).abs() < f64::EPSILON);
        assert!(
            grid.classifications()
                .any(|class| class == PairClass::Equal)
        );
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
