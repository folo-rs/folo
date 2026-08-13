//! SVG emission: the boundary between a style module's drawing code and the bytes the
//! book includes.
//!
//! Every figure in the appendix goes through [`draw`]. It exists to guarantee the three
//! properties the appendix's freshness check depends on, which no individual style
//! module should have to think about:
//!
//! * **Determinism.** Two renders of the same data produce byte-identical SVG, on any
//!   machine. This is what lets a test regenerate every figure and compare against the
//!   checked-in copy. It is why the crate takes `plotters` without its default
//!   features: the default text backend resolves fonts from the host system, which
//!   would make the bytes depend on what is installed.
//! * **Theme adaptation.** Structural elements are drawn in [`theme::INK`] and rewritten
//!   here to the CSS keyword `currentColor`, so the browser paints them in the reader's
//!   text colour under either of mdBook's themes. The background is left unpainted for
//!   the same reason — an opaque fill would punch a light rectangle into a dark page.
//! * **Responsiveness.** The emitted root element keeps its `viewBox` but scales to the
//!   width of the book's content column, so a figure never forces sideways scrolling.

use std::error::Error;

use plotters::backend::SVGBackend;
use plotters::coord::Shift;
use plotters::drawing::DrawingArea;
use plotters::prelude::IntoDrawingArea as _;

use crate::theme;

/// Renders one figure and returns the SVG the book embeds.
///
/// `body` receives the root drawing area and draws the figure into it. It reports
/// failure as any boxed error, which lets it use the `?` operator over `plotters`'
/// several error types; a failure means the figure's own drawing code is wrong, so it
/// panics rather than propagating.
///
/// # Panics
///
/// Panics when `body` reports an error. A style module draws into an in-memory buffer
/// with dimensions it chose itself, so the only way to fail is a bug in the drawing
/// code, which must fail the generator loudly rather than emit a malformed figure.
#[must_use]
pub fn draw<F>(width: u32, height: u32, body: F) -> String
where
    F: FnOnce(&DrawingArea<SVGBackend<'_>, Shift>) -> Result<(), Box<dyn Error>>,
{
    let mut buffer = String::new();
    {
        let root = SVGBackend::with_string(&mut buffer, (width, height)).into_drawing_area();
        body(&root).expect("a figure draws into an in-memory buffer of its own chosen size, so the only reachable failure is a defect in the figure's drawing code");
        root.present()
            .expect("presenting an in-memory SVG buffer performs no I/O and cannot fail");
    }
    to_svg(&buffer)
}

/// Rewrites a raw `plotters` SVG into the form the book embeds.
///
/// Kept separate from [`draw`] so the rewriting rules can be tested directly against a
/// known input rather than only through a rendered figure.
#[must_use]
pub fn to_svg(raw: &str) -> String {
    let themed = raw.replace(theme::INK_HEX, "currentColor");

    // Fixed `width`/`height` attributes would hold the figure at its authored size on a
    // narrow viewport and force sideways scrolling. Dropping them in favour of the
    // `viewBox` that `plotters` already emits lets the figure take the width of the
    // content column and derive its height from the aspect ratio.
    let Some(rest) = themed.strip_prefix("<svg width=") else {
        return themed;
    };
    let Some(view_box_start) = rest.find("viewBox=") else {
        return themed;
    };
    let Some(tail) = rest.get(view_box_start..) else {
        return themed;
    };
    format!("<svg style=\"width:100%;height:auto\" {tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_becomes_current_color_so_both_book_themes_read_it() {
        let raw = format!(
            "<svg width=\"10\" height=\"10\" viewBox=\"0 0 10 10\"><text fill=\"{}\"/></svg>",
            theme::INK_HEX
        );

        let rewritten = to_svg(&raw);

        assert!(rewritten.contains("fill=\"currentColor\""));
        assert!(!rewritten.contains(theme::INK_HEX));
    }

    #[test]
    fn the_root_element_scales_to_the_content_column() {
        let raw = "<svg width=\"680\" height=\"300\" viewBox=\"0 0 680 300\"><g/></svg>";

        let rewritten = to_svg(raw);

        assert!(rewritten.starts_with("<svg style=\"width:100%;height:auto\" viewBox="));
        assert!(rewritten.contains("viewBox=\"0 0 680 300\""));
    }

    #[test]
    fn data_colours_are_left_alone_so_they_stay_distinguishable() {
        let raw = "<svg width=\"10\" height=\"10\" viewBox=\"0 0 10 10\"><path stroke=\"#D64541\"/></svg>";

        let rewritten = to_svg(raw);

        assert!(rewritten.contains("stroke=\"#D64541\""));
    }

    #[test]
    fn an_unrecognized_root_element_passes_through_unchanged() {
        let raw = "<not-an-svg/>";

        assert_eq!(to_svg(raw), raw);
    }

    #[test]
    fn rendering_the_same_figure_twice_yields_identical_bytes() {
        let render = || {
            draw(120, 80, |root| {
                root.fill(&plotters::style::WHITE)?;
                Ok(())
            })
        };

        assert_eq!(render(), render());
    }
}
