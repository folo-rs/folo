//! The generated glossary table.
//!
//! The glossary page carries its own introduction; the table of terms is generated from
//! [`crate::glossary::TERMS`] so the page, the hover definitions in the prose, and the
//! test that holds them together all read from one list.

use std::fmt::Write as _;

use crate::assets::Asset;
use crate::glossary::TERMS;

/// The glossary table.
#[must_use]
pub fn assets() -> Vec<Asset> {
    vec![Asset::new("glossary-table.md", table())]
}

/// Renders the terms as a Markdown table, sorted for lookup.
fn table() -> String {
    let mut terms: Vec<_> = TERMS.iter().collect();
    terms.sort_by_key(|term| term.phrase);

    let mut markdown = String::from("| Term | What it means | Also called | Introduced in |\n");
    markdown.push_str("|---|---|---|---|\n");

    for term in terms {
        writeln!(
            markdown,
            "| {} | {} | {} | [{}]({}) |",
            term.phrase,
            term.definition,
            term.formal_name,
            chapter_title(term.chapter),
            term.chapter
        )
        .expect("writing to a String never fails");
    }

    markdown
}

/// The human-readable title of an appendix chapter file.
///
/// The mapping is spelled out rather than derived from the filename so a chapter can be
/// renamed on disk without silently changing how the glossary refers to it.
fn chapter_title(chapter: &str) -> &'static str {
    match chapter {
        "shape.md" => "Shape of the data",
        "collection.md" => "Collection",
        "selection.md" => "Selection",
        "reconstruction.md" => "Reconstruction",
        "detection.md" => "Detection",
        "gates.md" => "Noise gates",
        "coverage.md" => "Multiplicity and coverage",
        "reporting.md" => "Reporting",
        "insights.md" => "Insights",
        "limits.md" => "Limits",
        _ => "Data pipeline",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_term_appears_in_the_table() {
        let markdown = table();

        for term in TERMS {
            assert!(
                markdown.contains(term.phrase),
                "'{}' is missing from the glossary table",
                term.phrase
            );
        }
    }

    #[test]
    fn terms_are_sorted_for_lookup() {
        let markdown = table();
        let phrases: Vec<&str> = markdown
            .lines()
            .skip(2)
            .filter_map(|line| line.split('|').nth(1))
            .map(str::trim)
            .collect();

        let mut sorted = phrases.clone();
        sorted.sort_unstable();

        assert_eq!(phrases, sorted);
    }

    #[test]
    fn every_chapter_reference_resolves_to_a_title() {
        for term in TERMS {
            assert_ne!(
                chapter_title(term.chapter),
                "Data pipeline",
                "'{}' points at an unrecognized chapter '{}'",
                term.phrase,
                term.chapter
            );
        }
    }
}
