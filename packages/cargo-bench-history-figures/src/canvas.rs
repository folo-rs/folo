//! SVG emission: the boundary between a style module's drawing code and the bytes the
//! book includes.
//!
//! Every figure in the book goes through [`draw`]. It exists to guarantee the three
//! properties the book's freshness check depends on, which no individual style
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
//! * **Accessibility.** Each SVG has a name and description independent of color.
//!   The book's scrollable figure container preserves authored label sizes on narrow screens.

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
pub fn draw<F>(width: u32, height: u32, title: &str, description: &str, body: F) -> String
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
    to_svg(&buffer, title, description)
}

/// Rewrites a raw `plotters` SVG into the form the book embeds.
///
/// Kept separate from [`draw`] so the rewriting rules can be tested directly against a
/// known input rather than only through a rendered figure.
///
/// # Panics
///
/// Panics when the input does not have the `plotters` SVG root shape this crate emits.
/// This is the generator's in-memory boundary for the responsive-SVG contract, so a
/// shape the rewrite cannot handle is a figure defect rather than pass-through content.
#[must_use]
pub fn to_svg(raw: &str, title: &str, description: &str) -> String {
    let themed = raw.replace(theme::INK_HEX, "currentColor");

    // The viewBox keeps normal column scaling; the book's figure container supplies
    // a minimum width and local scrolling so narrow screens do not shrink the labels.
    let rest = themed
        .strip_prefix("<svg width=")
        .expect("plotters SVG output must begin with a width-bearing root element");
    let view_box_start = rest
        .find("viewBox=")
        .expect("plotters SVG output must carry a viewBox on the root element");
    let tail = rest
        .get(view_box_start..)
        .expect("viewBox starts at an ASCII token boundary");
    let (attributes, content) = tail
        .split_once('>')
        .expect("the plotters root element must have a closing angle bracket");
    let title = escape(title);
    let description = escape(description);
    // aria-label avoids document-global IDs: the same chart can occur more than once
    // in mdBook's print page or the side-by-side theme preview.
    format!(
        "<svg role=\"img\" aria-label=\"{title}\" style=\"width:100%;height:auto\" \
         {attributes}><title>{title}</title><desc>{description}</desc>{content}"
    )
}

/// Escapes text used in SVG metadata and quoted attributes.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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

        let rewritten = to_svg(&raw, "Example", "Description");

        assert!(rewritten.contains("fill=\"currentColor\""));
        assert!(!rewritten.contains(theme::INK_HEX));
    }

    #[test]
    fn the_root_element_scales_to_the_content_column() {
        let raw = "<svg width=\"680\" height=\"300\" viewBox=\"0 0 680 300\"><g/></svg>";

        let rewritten = to_svg(raw, "Example", "Description");

        assert!(rewritten.contains("style=\"width:100%;height:auto\" viewBox="));
        assert!(rewritten.contains("viewBox=\"0 0 680 300\""));
    }

    #[test]
    fn data_colours_are_left_alone_so_they_stay_distinguishable() {
        let raw = "<svg width=\"10\" height=\"10\" viewBox=\"0 0 10 10\"><path stroke=\"#D64541\"/></svg>";

        let rewritten = to_svg(raw, "Example", "Description");

        assert!(rewritten.contains("stroke=\"#D64541\""));
    }

    #[test]
    #[should_panic]
    fn an_svg_without_the_plotters_width_prefix_panics() {
        let raw = "<svg viewBox=\"0 0 10 10\"><g/></svg>";

        _ = to_svg(raw, "Example", "Description");
    }

    #[test]
    #[should_panic]
    fn an_svg_without_a_view_box_panics() {
        let raw = "<svg width=\"10\" height=\"10\"><g/></svg>";

        _ = to_svg(raw, "Example", "Description");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn rendering_the_same_figure_twice_yields_identical_bytes() {
        let render = || {
            draw(120, 80, "Example", "Blank canvas", |root| {
                root.fill(&plotters::style::WHITE)?;
                Ok(())
            })
        };

        assert_eq!(render(), render());
    }

    #[test]
    fn accessible_names_escape_markup_without_global_ids() {
        let svg = to_svg(
            "<svg width=\"10\" height=\"10\" viewBox=\"0 0 10 10\"><g/></svg>",
            "A \"quoted\" <comparison>",
            "Earlier & later values",
        );
        assert!(svg.contains("role=\"img\""));
        assert!(svg.contains("aria-label=\"A &quot;quoted&quot; &lt;comparison&gt;\""));
        assert!(svg.contains("<title>A &quot;quoted&quot; &lt;comparison&gt;</title>"));
        assert!(svg.contains("<desc>Earlier &amp; later values</desc>"));
        assert!(!svg.contains(" id="));
    }
}
