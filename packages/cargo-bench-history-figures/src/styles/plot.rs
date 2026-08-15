//! The core value-against-commit-position plot every data figure is built from.
//!
//! Most of what the appendix has to show is one shape: a metric's values laid out along
//! a stretch of first-parent history, with some of those values called out — removed by
//! a filter, collapsed into one, selected as a window, or fitted by a model. Giving that
//! shape one implementation is what keeps fifty figures looking like one document, and
//! it is why a style module composes a [`Plot`] rather than driving `plotters` directly.
//!
//! The x axis is a **commit position**, not a time and not an index into the observed
//! values. That distinction is itself one of the appendix's lessons: a commit with no
//! observation leaves an empty column, and the detectors never see how wide that gap is.
//! Laying every figure out this way means the pictures cannot accidentally imply
//! otherwise.

use std::error::Error;

use plotters::backend::SVGBackend;
use plotters::coord::Shift;
use plotters::drawing::DrawingArea;
use plotters::element::{Circle, PathElement, Rectangle, Text};
use plotters::prelude::ChartBuilder;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use plotters::style::{Color as _, RGBColor, ShapeStyle, TextStyle};

use crate::{coord, theme};

/// How a single observation should read against the rest of its series.
///
/// The appendix's teaching method is to show an operation acting on data — which points
/// it dropped, which it kept, which it created — so a plotted point almost always
/// carries a role beyond its value. Encoding that role here rather than as a colour at
/// the call site is what makes "removed" look the same in every chapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mark {
    /// An ordinary observation, carrying no special role in this figure.
    Plain,

    /// Data the operation under illustration excluded. Drawn subdued and struck
    /// through, so the reader can see what was dropped rather than having to infer it
    /// from an absence.
    Removed,

    /// Data the operation under illustration introduced or promoted.
    Added,

    /// The observation the surrounding prose is talking about.
    Focus,

    /// An observation the figure wants read as the bad direction.
    Regression,
}

impl Mark {
    /// The colour this role is drawn in.
    fn color(self, base: RGBColor) -> RGBColor {
        match self {
            Self::Plain => base,
            Self::Removed => theme::MUTED,
            Self::Added => theme::IMPROVEMENT,
            Self::Focus => theme::HIGHLIGHT,
            Self::Regression => theme::REGRESSION,
        }
    }

    /// Whether the point is struck through to show it was taken out of play.
    fn struck(self) -> bool {
        self == Self::Removed
    }
}

/// One observation: a value at a commit position, in a role.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    /// The commit's position along the first-parent line the figure covers.
    pub position: usize,

    /// The metric value recorded at that commit.
    pub value: f64,

    /// How the observation should read against the rest of the series.
    pub mark: Mark,

    /// The confidence interval the engine reported with the value, where it reported one.
    ///
    /// Drawn as a bar through the point. Carried on the observation rather than as a
    /// separate overlay because that is where it lives in the stored data, and because
    /// several figures turn on the fact that only some engines supply one.
    pub interval: Option<(f64, f64)>,
}

impl Observation {
    /// An ordinary observation.
    #[must_use]
    pub fn new(position: usize, value: f64) -> Self {
        Self {
            position,
            value,
            mark: Mark::Plain,
            interval: None,
        }
    }

    /// The same observation in a different role.
    #[must_use]
    pub fn marked(mut self, mark: Mark) -> Self {
        self.mark = mark;
        self
    }

    /// The same observation carrying the confidence interval the engine reported.
    #[must_use]
    pub fn interval(mut self, low: f64, high: f64) -> Self {
        self.interval = Some((low, high));
        self
    }
}

/// A horizontal reference line: a regime's level, a gate's threshold, a fitted centre.
#[derive(Clone, Debug)]
pub struct Rule {
    /// Where the line sits on the value axis.
    pub value: f64,

    /// What the line means, drawn at its right-hand end.
    pub label: String,

    /// The line's colour.
    pub color: RGBColor,

    /// Whether the line is dashed. Dashes mark a *derived* quantity — a fitted level or
    /// a computed threshold — so the reader can tell it apart from measured data at a
    /// glance.
    pub dashed: bool,
}

/// A shaded region of the plot: a regime, a selected window, a prediction band, or the
/// dead zone a floor gate ignores.
#[derive(Clone, Debug)]
pub struct Band {
    /// Inclusive commit position the region starts at.
    pub from: usize,

    /// Inclusive commit position the region ends at.
    pub to: usize,

    /// The value range the region spans, or `None` to span the whole value axis, which
    /// is what a "this stretch of history" band wants.
    pub values: Option<(f64, f64)>,

    /// What the region means.
    pub label: String,

    /// The region's colour, applied at [`theme::BAND_OPACITY`].
    pub color: RGBColor,
}

/// A free-standing note pinned to a point in the plot.
#[derive(Clone, Debug)]
pub struct Note {
    /// The commit position the note is anchored at.
    pub position: usize,

    /// The value the note is anchored at.
    pub value: f64,

    /// The note's text.
    pub text: String,

    /// The note's colour.
    pub color: RGBColor,
}

/// One value-against-commit-position figure, or one pane of a stacked comparison.
///
/// Built up by chained calls and rendered by [`Plot::render`] (standalone) or by the
/// composition helpers in [`crate::styles`], which draw several plots into one SVG.
#[derive(Clone, Debug)]
pub struct Plot {
    caption: String,
    value_label: String,
    span: usize,
    observations: Vec<Observation>,
    connect: bool,
    rules: Vec<Rule>,
    bands: Vec<Band>,
    notes: Vec<Note>,
    splits: Vec<(usize, String)>,
    base_color: RGBColor,
}

impl Plot {
    /// An empty plot covering `span` commit positions.
    ///
    /// The span is given rather than derived from the observations because the gaps at
    /// the end of a series are exactly what several figures exist to show: a series
    /// whose last observation is ten commits behind the tip must be drawn against the
    /// full history, not cropped to its own data.
    #[must_use]
    pub fn new(caption: impl Into<String>, span: usize) -> Self {
        Self {
            caption: caption.into(),
            value_label: String::new(),
            span,
            observations: Vec::new(),
            connect: true,
            rules: Vec::new(),
            bands: Vec::new(),
            notes: Vec::new(),
            splits: Vec::new(),
            base_color: theme::HIGHLIGHT,
        }
    }

    /// Labels the value axis, normally with the metric's unit.
    #[must_use]
    pub fn value_label(mut self, label: impl Into<String>) -> Self {
        self.value_label = label.into();
        self
    }

    /// Sets the colour plain observations are drawn in.
    #[must_use]
    pub fn base_color(mut self, color: RGBColor) -> Self {
        self.base_color = color;
        self
    }

    /// Draws observations as points only, without the connecting line.
    ///
    /// Wanted where the figure is about the *sample* rather than about movement through
    /// history — a branch-mode base window, for instance, whose points are levels being
    /// compared rather than a trajectory.
    #[must_use]
    pub fn scattered(mut self) -> Self {
        self.connect = false;
        self
    }

    /// Adds the observations to plot.
    #[must_use]
    pub fn observations(mut self, observations: impl IntoIterator<Item = Observation>) -> Self {
        self.observations.extend(observations);
        self
    }

    /// Plots `values` at consecutive commit positions starting from zero, all in the
    /// plain role — the common case for a figure about a dense series.
    #[must_use]
    pub fn values(self, values: &[f64]) -> Self {
        let observations = values
            .iter()
            .enumerate()
            .map(|(index, &value)| Observation::new(index, value));
        self.observations(observations)
    }

    /// Adds a horizontal reference line.
    #[must_use]
    pub fn rule(mut self, value: f64, label: impl Into<String>, color: RGBColor) -> Self {
        self.rules.push(Rule {
            value,
            label: label.into(),
            color,
            dashed: true,
        });
        self
    }

    /// Adds a shaded region spanning the whole value axis.
    #[must_use]
    pub fn band(
        mut self,
        from: usize,
        to: usize,
        label: impl Into<String>,
        color: RGBColor,
    ) -> Self {
        self.bands.push(Band {
            from,
            to,
            values: None,
            label: label.into(),
            color,
        });
        self
    }

    /// Adds a shaded region bounded on the value axis, such as a prediction band.
    #[must_use]
    pub fn value_band(
        mut self,
        from: usize,
        to: usize,
        values: (f64, f64),
        label: impl Into<String>,
        color: RGBColor,
    ) -> Self {
        self.bands.push(Band {
            from,
            to,
            values: Some(values),
            label: label.into(),
            color,
        });
        self
    }

    /// Marks a vertical boundary, such as a located change point or a merge base.
    #[must_use]
    pub fn split(mut self, position: usize, label: impl Into<String>) -> Self {
        self.splits.push((position, label.into()));
        self
    }

    /// Pins a note to a point in the plot.
    #[must_use]
    pub fn note(
        mut self,
        position: usize,
        value: f64,
        text: impl Into<String>,
        color: RGBColor,
    ) -> Self {
        self.notes.push(Note {
            position,
            value,
            text: text.into(),
            color,
        });
        self
    }

    /// The value range the plot must cover, padded so nothing touches the frame.
    ///
    /// Every drawn element contributes, not just the observations: a threshold rule or a
    /// prediction band that fell outside the axis would silently vanish, which in a
    /// figure whose whole point is "the move did not reach the threshold" would remove
    /// the evidence.
    fn value_range(&self) -> (f64, f64) {
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        let mut include = |value: f64| {
            if value.is_finite() {
                low = low.min(value);
                high = high.max(value);
            }
        };

        for observation in &self.observations {
            include(observation.value);
            if let Some((low, high)) = observation.interval {
                include(low);
                include(high);
            }
        }
        for rule in &self.rules {
            include(rule.value);
        }
        for band in &self.bands {
            if let Some((from, to)) = band.values {
                include(from);
                include(to);
            }
        }
        for note in &self.notes {
            include(note.value);
        }

        if !low.is_finite() || !high.is_finite() {
            // Nothing to scale to. Any range renders an empty frame rather than a
            // degenerate one that `plotters` would reject.
            return (0.0, 1.0);
        }

        // A flat series has no natural extent, so give it one proportional to its own
        // level. Without this the axis collapses and every point lands on one row.
        let extent = if (high - low).abs() < f64::EPSILON {
            high.abs().max(1.0) * 0.1
        } else {
            (high - low) * 0.15
        };
        (low - extent, high + extent)
    }

    /// The line segments joining the observations that are still in play.
    ///
    /// Extracted from the drawing code so the two rules it encodes — a removed point is
    /// not connected, and a gap is not bridged — can be asserted directly rather than
    /// inferred from rendered markup.
    fn segments(&self) -> Vec<((f64, f64), (f64, f64))> {
        if !self.connect {
            return Vec::new();
        }

        // Only observations still in play are connected: a line through removed points
        // would suggest the filtered series still contains them.
        let live: Vec<(f64, f64)> = self
            .observations
            .iter()
            .filter(|observation| observation.mark != Mark::Removed)
            .map(|observation| (coord::of(observation.position), observation.value))
            .collect();

        live.windows(2)
            .filter_map(|window| {
                let (Some(&from), Some(&to)) = (window.first(), window.get(1)) else {
                    return None;
                };
                // Consecutive commits only. A commit that carries no observation must read
                // as a hole, and a line drawn straight across one would assert a
                // trajectory through commits nothing was ever measured at — the exact
                // misreading the gap figures exist to prevent.
                if (to.0 - from.0) > 1.5 {
                    return None;
                }
                Some((from, to))
            })
            .collect()
    }

    /// Draws the plot into `area`.
    ///
    /// # Errors
    ///
    /// Propagates any drawing failure reported by the backend.
    pub fn draw(&self, area: &DrawingArea<SVGBackend<'_>, Shift>) -> Result<(), Box<dyn Error>> {
        let (low, high) = self.value_range();
        // The axis runs half a column past each end so the first and last observations
        // sit inside the frame rather than on it.
        let x_range = -0.5_f64..(coord::of(self.span) - 0.5);

        let mut chart = ChartBuilder::on(area)
            .caption(
                self.caption.as_str(),
                (theme::FONT, theme::FONT_TITLE, &theme::INK),
            )
            .margin(12)
            .x_label_area_size(34)
            .y_label_area_size(62)
            .build_cartesian_2d(x_range, low..high)?;

        chart
            .configure_mesh()
            .light_line_style(theme::INK.mix(0.0))
            .bold_line_style(theme::INK.mix(theme::GRID_OPACITY))
            .axis_style(theme::INK)
            .x_desc("commit position")
            .y_desc(self.value_label.as_str())
            .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
            .x_label_formatter(&|position| format!("{position:.0}"))
            .draw()?;

        for band in &self.bands {
            let (bottom, top) = band.values.unwrap_or((low, high));
            let left = coord::of(band.from) - 0.5;
            let right = coord::of(band.to) + 0.5;
            chart.draw_series(std::iter::once(Rectangle::new(
                [(left, bottom), (right, top)],
                band.color.mix(theme::BAND_OPACITY).filled(),
            )))?;
            if !band.label.is_empty() {
                chart.draw_series(std::iter::once(Text::new(
                    band.label.clone(),
                    (left, top),
                    TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&band.color),
                )))?;
            }
        }

        for (position, label) in &self.splits {
            let x = coord::of(*position) - 0.5;
            chart.draw_series(std::iter::once(PathElement::new(
                vec![(x, low), (x, high)],
                ShapeStyle::from(theme::REGRESSION).stroke_width(2),
            )))?;
            if !label.is_empty() {
                chart.draw_series(std::iter::once(Text::new(
                    label.clone(),
                    (x, high),
                    TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::REGRESSION),
                )))?;
            }
        }

        for rule in &self.rules {
            let segments: Vec<(f64, f64)> = if rule.dashed {
                // Drawn as a run of short strokes: `plotters`' dashed-line helper needs
                // a feature this crate does not take, and an explicit run keeps the
                // output identical across backend versions.
                Vec::new()
            } else {
                vec![(-0.5, rule.value), (coord::of(self.span) - 0.5, rule.value)]
            };

            if segments.is_empty() {
                let mut position = 0_usize;
                while position < self.span {
                    let start = coord::of(position) - 0.5;
                    let end = (coord::of(position) - 0.5 + 0.6).min(coord::of(self.span) - 0.5);
                    chart.draw_series(std::iter::once(PathElement::new(
                        vec![(start, rule.value), (end, rule.value)],
                        ShapeStyle::from(rule.color).stroke_width(1),
                    )))?;
                    position = position.saturating_add(1);
                }
            } else {
                chart.draw_series(std::iter::once(PathElement::new(
                    segments,
                    ShapeStyle::from(rule.color).stroke_width(1),
                )))?;
            }

            if !rule.label.is_empty() {
                // Right-aligned at the right edge and nudged up off the line. Rule labels
                // sat at the left originally, where they collided with both the value-axis
                // labels and the leading observations; the right edge is the one place a
                // horizontal rule reliably has room, whatever the data does.
                let offset = (high - low) * 0.02;
                chart.draw_series(std::iter::once(Text::new(
                    rule.label.clone(),
                    (coord::of(self.span) - 0.6, rule.value + offset),
                    TextStyle::from((theme::FONT, theme::FONT_TICK))
                        .color(&rule.color)
                        .pos(Pos::new(HPos::Right, VPos::Bottom)),
                )))?;
            }
        }

        for (from, to) in self.segments() {
            chart.draw_series(std::iter::once(PathElement::new(
                vec![from, to],
                ShapeStyle::from(self.base_color).stroke_width(theme::DATA_STROKE),
            )))?;
        }

        for observation in &self.observations {
            let color = observation.mark.color(self.base_color);
            let at = (coord::of(observation.position), observation.value);

            if let Some((low, high)) = observation.interval {
                // Drawn with end caps so a narrow interval is still visibly an interval
                // rather than a stray tick, which matters because several figures turn on
                // comparing a wide one against a narrow one.
                chart.draw_series(std::iter::once(PathElement::new(
                    vec![(at.0, low), (at.0, high)],
                    ShapeStyle::from(color).stroke_width(1),
                )))?;
                for end in [low, high] {
                    chart.draw_series(std::iter::once(PathElement::new(
                        vec![(at.0 - 0.18, end), (at.0 + 0.18, end)],
                        ShapeStyle::from(color).stroke_width(1),
                    )))?;
                }
            }

            chart.draw_series(std::iter::once(Circle::new(
                at,
                theme::POINT_RADIUS,
                color.filled(),
            )))?;

            if observation.mark.struck() {
                let span = (high - low) * 0.03;
                chart.draw_series(std::iter::once(PathElement::new(
                    vec![(at.0 - 0.35, at.1 - span), (at.0 + 0.35, at.1 + span)],
                    ShapeStyle::from(color).stroke_width(2),
                )))?;
                chart.draw_series(std::iter::once(PathElement::new(
                    vec![(at.0 - 0.35, at.1 + span), (at.0 + 0.35, at.1 - span)],
                    ShapeStyle::from(color).stroke_width(2),
                )))?;
            }
        }

        for note in &self.notes {
            chart.draw_series(std::iter::once(Text::new(
                note.text.clone(),
                (coord::of(note.position), note.value),
                TextStyle::from((theme::FONT, theme::FONT_LABEL)).color(&note.color),
            )))?;
        }

        Ok(())
    }

    /// Renders the plot as a standalone figure.
    #[must_use]
    pub fn render(&self) -> String {
        crate::canvas::draw(theme::WIDTH, theme::HEIGHT, |root| self.draw(root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_series_still_gets_a_usable_value_axis() {
        let plot = Plot::new("flat", 5).values(&[100.0, 100.0, 100.0, 100.0, 100.0]);

        let (low, high) = plot.value_range();

        assert!(low < 100.0, "the axis must extend below the level");
        assert!(high > 100.0, "the axis must extend above the level");
    }

    #[test]
    fn a_threshold_rule_outside_the_data_stays_on_the_axis() {
        let plot = Plot::new("gated", 3).values(&[100.0, 100.0, 101.0]).rule(
            140.0,
            "threshold",
            theme::REGRESSION,
        );

        let (_, high) = plot.value_range();

        assert!(
            high > 140.0,
            "a threshold the data never reaches is the evidence such a figure exists to show"
        );
    }

    #[test]
    fn an_empty_plot_renders_rather_than_collapsing() {
        let plot = Plot::new("nothing", 4);

        let (low, high) = plot.value_range();

        assert!(low < high);
    }

    #[test]
    fn removed_points_are_drawn_subdued() {
        assert_eq!(Mark::Removed.color(theme::HIGHLIGHT), theme::MUTED);
        assert!(Mark::Removed.struck());
        assert!(!Mark::Plain.struck());
    }

    /// A commit that carries no observation must read as a hole. A connecting line drawn
    /// across one would assert a trajectory through commits nothing was measured at,
    /// which is precisely what the gap figures exist to disprove — so the break is a
    /// correctness property of the figure, not a cosmetic choice.
    #[test]
    fn a_gap_breaks_the_connecting_line() {
        let dense = Plot::new("dense", 6).values(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let gapped = Plot::new("gapped", 6).observations([
            Observation::new(0, 1.0),
            Observation::new(1, 2.0),
            Observation::new(4, 5.0),
            Observation::new(5, 6.0),
        ]);

        assert_eq!(
            dense.segments().len(),
            5,
            "five joins between six adjacent observations"
        );
        assert_eq!(
            gapped.segments().len(),
            2,
            "the two adjacent pairs are joined and the gap between them is not"
        );
    }

    /// Removing a point must also remove the line through it, or the figure would still
    /// show the filtered series passing through data the stage discarded.
    #[test]
    fn a_removed_point_is_not_connected_to_its_neighbours() {
        let plot = Plot::new("filtered", 3).observations([
            Observation::new(0, 1.0),
            Observation::new(1, 2.0).marked(Mark::Removed),
            Observation::new(2, 3.0),
        ]);

        assert!(
            plot.segments().is_empty(),
            "the surviving points are not adjacent once the middle one is dropped"
        );
    }

    #[test]
    fn a_scattered_plot_joins_nothing() {
        let plot = Plot::new("sample", 3).values(&[1.0, 2.0, 3.0]).scattered();

        assert!(plot.segments().is_empty());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn rendering_is_reproducible() {
        let plot = Plot::new("series", 6).values(&[1.0, 2.0, 3.0, 2.0, 1.0, 2.0]);

        assert_eq!(plot.render(), plot.render());
    }

    /// A figure comparing a wide interval against a narrow one is how the interval vetoes
    /// are taught, so an interval reaching past the observations must widen the axis
    /// rather than being clipped away.
    #[test]
    fn a_confidence_interval_is_kept_on_the_axis() {
        let plot = Plot::new("dispersed", 2).observations([
            Observation::new(0, 100.0).interval(80.0, 120.0),
            Observation::new(1, 101.0).interval(99.0, 103.0),
        ]);

        let (low, high) = plot.value_range();

        assert!(low < 80.0);
        assert!(high > 120.0);
    }
}
