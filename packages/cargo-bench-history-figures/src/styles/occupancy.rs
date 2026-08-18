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

use plotters::backend::SVGBackend;
use plotters::coord::Shift;
use plotters::drawing::DrawingArea;
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
    /// Cells in the stable order their legend uses.
    const LEGEND_ORDER: [Self; 5] = [
        Self::Clean,
        Self::Dirty,
        Self::Excluded,
        Self::Focus,
        Self::Absent,
    ];

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

    /// How the legend names this cell role.
    fn label(self) -> &'static str {
        match self {
            Self::Absent => "no run",
            Self::Clean => "clean run",
            Self::Dirty => "dirty run",
            Self::Excluded => "excluded run",
            Self::Focus => "highlighted run",
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
        self.rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0)
    }

    /// Renders the grid.
    #[must_use]
    pub fn render(&self) -> String {
        let row_height = 36_u32;
        let grid_height = 76_u32.saturating_add(
            row_height.saturating_mul(u32::try_from(self.rows.len()).unwrap_or(u32::MAX)),
        );
        let legend = self.legend_cells();
        let height = grid_height.saturating_add(if legend.is_empty() {
            0
        } else {
            OCCUPANCY_LEGEND_HEIGHT
        });

        canvas::draw(theme::WIDTH, height, |root| {
            let (grid_area, legend_area) = if legend.is_empty() {
                (root.clone(), None)
            } else {
                let (grid_area, legend_area) = root.split_vertically(grid_height);
                (grid_area, Some(legend_area))
            };
            let span = self.span();
            let rows = self.rows.len();
            // Partition names are long, and `plotters` puts value-axis labels at tick
            // positions rather than between them, which cannot label a band. The names
            // are therefore drawn inside the plot, in a gutter made by extending the
            // commit axis to the left of zero. Negative positions are suppressed from the
            // tick labels so the gutter reads as margin rather than as commit -8.
            let gutter = (coord::of(span) * 0.45).max(4.0);
            let mut chart = ChartBuilder::on(&grid_area)
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

            if let Some(legend_area) = legend_area {
                draw_occupancy_legend(&legend_area, &legend)?;
            }

            Ok::<(), Box<dyn Error>>(())
        })
    }

    /// The cell roles present in this grid, ordered for a stable legend.
    fn legend_cells(&self) -> Vec<Cell> {
        Cell::LEGEND_ORDER
            .into_iter()
            .filter(|cell| {
                self.rows
                    .iter()
                    .any(|row| row.cells.iter().any(|present| present == cell))
            })
            .collect()
    }
}

/// Height reserved below an occupancy grid for the role legend.
///
/// Keeping the legend in its own strip avoids shrinking the commit columns or competing
/// with the row labels drawn inside the grid.
const OCCUPANCY_LEGEND_HEIGHT: u32 = 34;

/// Pixel inset from the legend strip's left edge.
const OCCUPANCY_LEGEND_LEFT: i32 = 16;

/// Pixel inset from the legend strip's top edge.
const OCCUPANCY_LEGEND_TOP: i32 = 8;

/// Width and height of one legend swatch.
const OCCUPANCY_LEGEND_SWATCH: i32 = 12;

/// Gap between a legend swatch and its label.
const OCCUPANCY_LEGEND_TEXT_GAP: i32 = 6;

/// Draws the legend for the cell roles present in an occupancy grid.
fn draw_occupancy_legend(
    area: &DrawingArea<SVGBackend<'_>, Shift>,
    cells: &[Cell],
) -> Result<(), Box<dyn Error>> {
    if cells.is_empty() {
        return Ok(());
    }

    let (width, _) = area.dim_in_pixel();
    let cell_count = i32::try_from(cells.len()).expect("the legend cell count fits in i32");
    let column_width = i32::try_from(width)
        .unwrap_or(i32::MAX)
        .checked_div(cell_count)
        .expect("the legend has at least one cell");
    for (index, cell) in cells.iter().copied().enumerate() {
        let index = i32::try_from(index).expect("the legend cell count fits in i32");
        let left = OCCUPANCY_LEGEND_LEFT.saturating_add(index.saturating_mul(column_width));
        let swatch_right = left.saturating_add(OCCUPANCY_LEGEND_SWATCH);
        let swatch_bottom = OCCUPANCY_LEGEND_TOP.saturating_add(OCCUPANCY_LEGEND_SWATCH);
        let swatch = [(left, OCCUPANCY_LEGEND_TOP), (swatch_right, swatch_bottom)];
        if let Some(color) = cell.color() {
            area.draw(&Rectangle::new(swatch, color.mix(0.75).filled()))?;
        } else {
            area.draw(&Rectangle::new(
                swatch,
                ShapeStyle::from(theme::INK.mix(0.25)).stroke_width(1),
            ))?;
        }
        if cell == Cell::Excluded {
            area.draw(&Rectangle::new(
                swatch,
                ShapeStyle::from(theme::REGRESSION).stroke_width(2),
            ))?;
        }
        area.draw(&Text::new(
            cell.label().to_owned(),
            (
                swatch_right.saturating_add(OCCUPANCY_LEGEND_TEXT_GAP),
                OCCUPANCY_LEGEND_TOP.saturating_add(OCCUPANCY_LEGEND_SWATCH),
            ),
            TextStyle::from((theme::FONT, theme::FONT_TICK)).color(&theme::INK),
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Occupancy {
        Occupancy::new("what the store holds")
            .row(
                "criterion / machine a1b2",
                [
                    Cell::Clean,
                    Cell::Clean,
                    Cell::Absent,
                    Cell::Clean,
                    Cell::Dirty,
                ],
            )
            .row(
                "criterion / machine c3d4",
                [
                    Cell::Absent,
                    Cell::Absent,
                    Cell::Clean,
                    Cell::Clean,
                    Cell::Excluded,
                ],
            )
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn every_partition_is_labelled() {
        let svg = sample().render();

        assert!(svg.contains("criterion / machine a1b2"));
        assert!(svg.contains("criterion / machine c3d4"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn the_legend_names_the_cell_roles_present_in_the_grid() {
        let svg = sample().render();

        assert!(svg.contains("clean run"));
        assert!(svg.contains("dirty run"));
        assert!(svg.contains("excluded run"));
        assert!(svg.contains("no run"));
        assert!(!svg.contains("highlighted run"));
    }

    #[test]
    fn the_span_covers_the_longest_row() {
        let grid = Occupancy::new("ragged")
            .row("short", [Cell::Clean])
            .row("long", [Cell::Clean, Cell::Clean, Cell::Clean]);

        assert_eq!(
            grid.span(),
            3,
            "a partition that stopped must still show the commits it lacks"
        );
    }

    #[test]
    fn an_absent_run_carries_no_fill() {
        assert!(Cell::Absent.color().is_none());
        assert!(Cell::Clean.color().is_some());
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
