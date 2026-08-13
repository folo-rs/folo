//! The before/after figure that shows a pipeline stage acting on data.
//!
//! This is the appendix's workhorse. Every stage of the pipeline adds, removes,
//! reorders, collapses or reshapes observations, and a reader should never have to infer
//! which of those happened from a changed picture. An operation figure therefore draws
//! the input, draws the output, and marks in the input exactly what the stage was about
//! to do to each affected observation — so the two panes read as one sentence rather
//! than as a puzzle.
//!
//! The pane captions are supplied by the caller rather than fixed to "before" and
//! "after" because the useful caption is the *reason*: "every stored run" above,
//! "admitted for this analysis" below.

use crate::canvas;
use crate::styles::plot::Plot;
use crate::theme;

use plotters::style::TextStyle;

/// A two-pane figure: one operation, its input, and its output.
#[derive(Clone, Debug)]
pub struct Operation {
    /// What the operation does, shown above both panes.
    title: String,

    /// The state going in.
    before: Plot,

    /// The state coming out.
    after: Plot,
}

impl Operation {
    /// A figure showing `before` becoming `after` under the operation named by `title`.
    #[must_use]
    pub fn new(title: impl Into<String>, before: Plot, after: Plot) -> Self {
        Self {
            title: title.into(),
            before,
            after,
        }
    }

    /// Renders the figure.
    #[must_use]
    pub fn render(&self) -> String {
        // Two panes plus a strip for the title. The title is drawn as its own band
        // rather than as a caption on the upper pane so both panes keep the same plot
        // area and their value axes stay visually comparable.
        let title_strip = 28_u32;
        let height = theme::PANE_HEIGHT
            .saturating_mul(2)
            .saturating_add(title_strip);

        canvas::draw(theme::WIDTH, height, |root| {
            let (header, panes) = root.split_vertically(title_strip);
            header.titled(
                self.title.as_str(),
                TextStyle::from((theme::FONT, theme::FONT_TITLE)).color(&theme::INK),
            )?;

            let (upper, lower) = panes.split_vertically(theme::PANE_HEIGHT);
            self.before.draw(&upper)?;
            self.after.draw(&lower)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::styles::plot::{Mark, Observation};

    fn sample() -> Operation {
        let before = Plot::new("every stored run", 6)
            .values(&[100.0, 101.0, 99.0, 130.0, 131.0, 129.0]);
        let after = Plot::new("admitted for this analysis", 6).observations([
            Observation::new(0, 100.0),
            Observation::new(1, 101.0),
            Observation::new(2, 99.0).marked(Mark::Removed),
            Observation::new(3, 130.0),
            Observation::new(4, 131.0),
            Observation::new(5, 129.0),
        ]);
        Operation::new("dirty runs are not admitted on the base side", before, after)
    }

    #[test]
    fn both_panes_are_rendered_into_one_figure() {
        let svg = sample().render();

        assert!(svg.contains("every stored run"));
        assert!(svg.contains("admitted for this analysis"));
        assert!(svg.contains("dirty runs are not admitted on the base side"));
    }

    #[test]
    fn rendering_is_reproducible() {
        assert_eq!(sample().render(), sample().render());
    }
}
