//! A self-contained HTML page showing every generated figure on both of the book's
//! backgrounds.
//!
//! The figures adapt to the reader's theme by inheriting the surrounding text colour, so
//! "does this figure work" is a question about two renderings, not one. Checking that in
//! the built book means rebuilding it and toggling the theme by hand for each figure;
//! this page puts both renderings side by side and needs no book build, which is what
//! makes it practical to check every figure rather than the one being worked on.
//!
//! The page is a development aid and is never published: it is written to the build
//! directory on request and is not part of the checked-in asset set.

use std::fmt::Write as _;

use crate::assets::Asset;

/// The book's light-theme page background and text colour.
///
/// Taken from mdBook's default light theme so the preview shows what a reader sees
/// rather than an approximation.
const LIGHT: (&str, &str) = ("#ffffff", "#333333");

/// The book's dark-theme page background and text colour, from mdBook's navy theme.
const DARK: (&str, &str) = ("#161923", "#bcbdd0");

/// Builds the preview page for `assets`.
///
/// Non-SVG assets are listed as text blocks: a generated Markdown table or report
/// excerpt is content the appendix embeds too, and seeing it beside the figures is how a
/// reader of this page checks a whole chapter's evidence at once.
#[must_use]
pub fn page(assets: &[Asset]) -> String {
    let mut html = String::new();
    html.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>cargo-bench-history appendix figures</title>\n<style>\n\
         body { font-family: sans-serif; margin: 0; padding: 24px; background: #f4f4f4; }\n\
         h1 { font-size: 20px; }\n\
         .asset { margin: 0 0 32px; }\n\
         .asset h2 { font-size: 14px; font-family: monospace; font-weight: normal; }\n\
         .themes { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }\n\
         .theme { padding: 12px; border-radius: 6px; }\n\
         pre { white-space: pre-wrap; font-size: 12px; margin: 0; }\n\
         </style>\n</head>\n<body>\n<h1>Appendix figures</h1>\n",
    );

    for asset in assets {
        write!(
            html,
            "<div class=\"asset\"><h2>{}</h2><div class=\"themes\">",
            asset.path
        )
        .expect("writing to a String never fails");
        for (background, foreground) in [LIGHT, DARK] {
            write!(
                html,
                "<div class=\"theme\" style=\"background:{background};color:{foreground}\">"
            )
            .expect("writing to a String never fails");
            if asset.path.ends_with(".svg") {
                html.push_str(&asset.content);
            } else {
                write!(html, "<pre>{}</pre>", escape(&asset.content))
                    .expect("writing to a String never fails");
            }
            html.push_str("</div>");
        }
        html.push_str("</div></div>\n");
    }

    html.push_str("</body>\n</html>\n");
    html
}

/// Escapes text for embedding in an HTML element.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_asset_is_embedded_once_per_theme() {
        let assets = vec![Asset::new("figures/a.svg", "<svg id=\"marker\"/>")];

        let html = page(&assets);

        assert_eq!(html.matches("id=\"marker\"").count(), 2);
    }

    #[test]
    fn a_text_asset_is_escaped_rather_than_rendered() {
        let assets = vec![Asset::new("tables/a.md", "1 < 2 & 3 > 2")];

        let html = page(&assets);

        assert!(html.contains("1 &lt; 2 &amp; 3 &gt; 2"));
    }
}
