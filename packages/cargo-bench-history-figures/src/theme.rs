//! The single palette, type scale and geometry every figure in the appendix shares.
//!
//! Consistency across some fifty figures is a property of this module rather than of
//! the person drawing them: a style module never names a colour or a size directly, it
//! names a role here. Changing how the whole appendix looks is therefore an edit to
//! this file.
//!
//! # Colour and the book's two themes
//!
//! mdBook ships a light and a dark theme and the reader switches between them, so a
//! figure cannot bake in "black on white". Structural elements — axes, labels, tick
//! marks, rules — are drawn in [`INK`], a sentinel colour that [`canvas`] rewrites to
//! the CSS keyword `currentColor` on the way out. The browser then paints them in
//! whatever colour the surrounding prose is using, in either theme.
//!
//! Data colours cannot use that trick, because they must stay distinguishable from one
//! another. They are instead chosen to hold enough contrast against both a white and a
//! near-black background, which rules out both very dark and very pale tones.
//!
//! [`canvas`]: crate::canvas

use plotters::style::RGBColor;

/// Structural ink: axes, tick labels, captions, rules, and anything else that is
/// furniture rather than data.
///
/// The value is a sentinel, not a colour anyone sees. [`to_svg`] rewrites it to
/// `currentColor` so the browser paints it in the reader's theme colour. It is a dark
/// near-black so that a figure inspected outside the book (opened directly from the
/// repository, where nothing rewrites it) still reads correctly on white.
///
/// [`to_svg`]: crate::canvas::to_svg
pub const INK: RGBColor = RGBColor(1, 2, 3);

/// The literal the SVG serializer writes for [`INK`], which is what
/// [`to_svg`](crate::canvas::to_svg) searches for.
pub const INK_HEX: &str = "#010203";

/// A regression, or any quantity the appendix wants the reader to read as the bad
/// direction.
///
/// Mid-toned rather than a pure red so it stays legible against the dark theme's
/// near-black background as well as against white.
pub const REGRESSION: RGBColor = RGBColor(214, 69, 65);

/// An improvement, or any quantity the appendix wants the reader to read as the good
/// direction.
pub const IMPROVEMENT: RGBColor = RGBColor(35, 144, 86);

/// The neutral highlight for the element a figure is drawing attention to when that
/// element carries no good/bad meaning — a selected window, an accepted boundary, the
/// point under discussion.
pub const HIGHLIGHT: RGBColor = RGBColor(48, 110, 200);

/// Data the operation under illustration removed, excluded, or never looked at.
///
/// Deliberately low-contrast against both themes: excluded data must stay visible so
/// the reader can see what was dropped, while being unmistakably subordinate to the
/// data that survived.
pub const MUTED: RGBColor = RGBColor(150, 155, 160);

/// A second data colour, for figures that must distinguish two series that carry no
/// good/bad meaning of their own.
pub const ALTERNATE: RGBColor = RGBColor(150, 100, 190);

/// The opacity shaded regions are filled at.
///
/// Bands, regime shading and dead-zones sit *behind* the data and must never compete
/// with it, but a band the reader cannot see teaches nothing. This is the weakest fill
/// that stays visible against both themes.
pub const BAND_OPACITY: f64 = 0.15;

/// The opacity gridlines are drawn at.
pub const GRID_OPACITY: f64 = 0.12;

/// The font stack every figure labels with.
///
/// A generic family rather than a specific face: the SVG carries no embedded font, so
/// naming a face the reader may not have installed would only produce an unpredictable
/// fallback. The book's own body text resolves the same way.
pub const FONT: &str = "sans-serif";

/// Point size for a figure's caption.
pub const FONT_TITLE: i32 = 15;

/// Point size for axis labels and in-figure annotations.
pub const FONT_LABEL: i32 = 12;

/// Point size for tick labels and other secondary text.
pub const FONT_TICK: i32 = 11;

/// Width of a full-width figure, in SVG user units.
///
/// The book's content column is around 700 units wide at its default typography, so a
/// figure this wide fills the column without forcing the reader to scroll sideways,
/// and scales down cleanly on a narrow viewport because the SVG carries a `viewBox`.
pub const WIDTH: u32 = 680;

/// Height of a standard single-pane figure, in SVG user units.
///
/// Sized so a figure and a short paragraph of explanation share one screen, which is
/// what lets the reader check the claim against the picture without scrolling.
pub const HEIGHT: u32 = 300;

/// Height of one pane in a stacked before/after figure.
pub const PANE_HEIGHT: u32 = 240;

/// Stroke width for data lines.
pub const DATA_STROKE: u32 = 2;

/// Radius of a plotted observation.
pub const POINT_RADIUS: u32 = 3;
