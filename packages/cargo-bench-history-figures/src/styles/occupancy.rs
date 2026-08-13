//! The occupancy grid: which commits carry an observation, for each discriminant set.
//!
//! A stored history is not a table — it is a sparse scattering of runs across commits and
//! machines, and almost every question a reader has about collection is really a question
//! about where the holes are. Rendering the store as a grid makes that shape immediate:
//! a CI pool that rotates machine keys shows up as two half-filled rows, a backfill shows
//! up as a row filling in, and a benchmark that stopped being measured shows up as a row
//! that stops.
//!
//! The same grid also carries the selection stage's story, where cells are struck out as
//! filters exclude them.

use std::error::Error;

use plotters::element::{Rectangle, Text};
use plotters::prelude::ChartBuilder;
use plotters::style::{Color as _, RGBColor, ShapeStyle, TextStyle};

use crate::{canvas, coord, theme};

/// What one commit holds for one discriminant set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cell {
    /// No run was stored.
    Absent,

    /// A run from a clean working tree.
    Clean,

    /// A run from a dirty working tree.
    Dirty,

    /// A run that exists but this analysis excluded.
    Excluded,

    /// A run the surrounding prose is drawing attention to.
    Focus,
}

impl Cell {
    /// The colour the cell is filled with.
    fn color(self) -> Option<RGBColor> {
        match self {
            Self::Absent => None,
            Self::Clean => Some(theme::HIGHLIGHT),
            Self::Dirty => Some(theme::ALTERNATE),
            Self::Excluded => Some(theme::MUTED),
            Self::Focus => Some(theme::IMPROVEMENT),
        }
    }
}

/// One row of the grid: a discriminant set and what it holds at each commit.
#[derive(Clone, Debug)]
pub struct Row {
    /// How the appendix names this partition, e.g. the engine and machine key.
    pub label: String,

    /// One entry per commit position, oldest first.
    pub cells: Vec<Cell>,
}

/// A store rendered as commits across, partitions down.
#[derive(Clone, Debug)]
pub struct Occupancy {
    caption: String,
    rows: Vec<Row>,
}

impl Occupancy {
    /// An empty grid captioned `caption`.
    #[must_use]
    pub fn new(caption: impl Into<String>) -> Self {
        Self {
            caption: caption.into(),
            rows: Vec::new(),
        }
    }

    /// Appends a partition's row.
    #[must_use]
    pub fn row(mut self, label: impl Into<String>, cells: impl IntoIterator<Item = Cell>) -> Self {
        self.rows.push(Row {
            label: label.into(),
            cells: cells.into_iter().collect(),
        });
        self
    }

    /// The number of commit columns, taken from the longest row so a partition that
    /// stopped being measured still shows the commits it is missing.
    fn span(&self) -> usize {
        self.rows.iter().map(|row| row.cells.len()).max().unwrap_or(0)
    }

    /// Renders the grid.
    #[must_use]
    pub fn render(&self) -> String {
        let row_height = 36_u32;
        let height = 76_u32.saturating_add(
            row_height.saturating_mul(u32::try_from(self.rows.len()).unwrap_or(u32::MAX)),
        );

        canvas::draw(theme::WIDTH, height, |root| {
            let span = self.span();
            let rows = self.rows.len();
            // Partition names are long, and `plotters` puts value-axis labels at tick
            // positions rather than between them, which cannot label a band. The names
            // are therefore drawn inside the plot, in a gutter made by extending the
            // commit axis to the left of zero. Negative positions are suppressed from the
            // tick labels so the gutter reads as margin rather than as commit -8.
            let gutter = (coord::of(span) * 0.45).max(4.0);
            let mut chart = ChartBuilder::on(root)
                .caption(
                    self.caption.as_str(),
                    (theme::FONT, theme::FONT_TITLE, &theme::INK),
                )
                .margin(12)
                .x_label_area_size(30)
                .y_label_area_size(10)
                .build_cartesian_2d(-gutter..coord::of(span), 0.0_f64..coord::of(rows))?;

            chart
                .configure_mesh()
                .disable_mesh()
                .axis_style(theme::INK)
                .x_desc("commit position")
                .label_style((theme::FONT, theme::FONT_TICK, &theme::INK))
                .y_labels(0)
                .x_label_formatter(&|position| {
                    if *position < 0.0 {
                        String::new()
                    } else {
                        format!("{position:.0}")
                    }
                })
                .draw()?;

            for (index, row) in self.rows.iter().enumerate() {
                // Rows read top-down in the order given, but the axis counts upward.
                let top = coord::of(rows.saturating_sub(index));
                let bottom = top - 1.0;

                chart.draw_series(std::iter::once(Text::new(
                    row.label.clone(),
                    (-gutter + 0.2, top - 0.35),
                    TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::INK),
                )))?;

                for (column, cell) in row.cells.iter().enumerate() {
                    let left = coord::of(column) + 0.12;
                    let right = coord::of(column) + 0.88;
                    let cell_bottom = bottom + 0.2;
                    let cell_top = top - 0.2;

                    let Some(color) = cell.color() else {
                        // An absent run is drawn as an outline rather than left blank, so
                        // the reader can count the commits that hold nothing.
                        chart.draw_series(std::iter::once(Rectangle::new(
                            [(left, cell_bottom), (right, cell_top)],
                            ShapeStyle::from(theme::INK.mix(0.25)).stroke_width(1),
                        )))?;
                        continue;
                    };

                    chart.draw_series(std::iter::once(Rectangle::new(
                        [(left, cell_bottom), (right, cell_top)],
                        color.mix(0.75).filled(),
                    )))?;

                    if *cell == Cell::Excluded {
                        chart.draw_series(std::iter::once(Rectangle::new(
                            [(left, cell_bottom), (right, cell_top)],
                            ShapeStyle::from(theme::REGRESSION).stroke_width(2),
                        )))?;
                    }
                }
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Occupancy {
        Occupancy::new("what the store holds")
            .row(
                "criterion / machine a1b2",
                [Cell::Clean, Cell::Clean, Cell::Absent, Cell::Clean, Cell::Dirty],
            )
            .row(
                "criterion / machine c3d4",
                [Cell::Absent, Cell::Absent, Cell::Clean, Cell::Clean, Cell::Excluded],
            )
    }

    #[test]
    fn every_partition_is_labelled() {
        let svg = sample().render();

        assert!(svg.contains("criterion / machine a1b2"));
        assert!(svg.contains("criterion / machine c3d4"));
    }

    #[test]
    fn the_span_covers_the_longest_row() {
        let grid = Occupancy::new("ragged")
            .row("short", [Cell::Clean])
            .row("long", [Cell::Clean, Cell::Clean, Cell::Clean]);

        assert_eq!(grid.span(), 3, "a partition that stopped must still show the commits it lacks");
    }

    #[test]
    fn an_absent_run_carries_no_fill() {
        assert!(Cell::Absent.color().is_none());
        assert!(Cell::Clean.color().is_some());
    }

    #[test]
    fn rendering_is_reproducible() {
        assert_eq!(sample().render(), sample().render());
    }
}
