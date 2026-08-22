//! The appendix's glossary: every term it defines, in one place.
//!
//! The appendix is written for engineers rather than statisticians, so a term is defined
//! before it is used. Both the glossary page and each chapter's own "Terms used here" table
//! are generated from this one list, which is what stops the same term being explained two
//! different ways in two chapters — the failure mode a hand-maintained glossary always
//! eventually reaches, and one that had already begun here before the tables were generated.
//!
//! Adding a term therefore has a cost, which is deliberate: the cheapest way to avoid it is to
//! write a sentence the reader already understands.

/// One defined term.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Term {
    /// The phrase as it appears in the prose. Written the way it reads in a sentence
    /// rather than as a dictionary headword.
    pub phrase: &'static str,

    /// The plain-language definition. One sentence, no notation, and no term that is
    /// itself only defined here.
    pub definition: &'static str,

    /// The name a statistics text would use, where knowing it helps the reader search for
    /// more. Empty when the plain phrase *is* the standard name, or when no standard name
    /// would help.
    pub formal_name: &'static str,

    /// The appendix page that introduces the term, as a link target relative to the
    /// appendix directory.
    pub chapter: &'static str,
}

/// Every term the appendix defines.
///
/// Kept in the order a reader meets them rather than alphabetically; the generated page
/// sorts them for lookup. Reading down this list is a fair summary of what the appendix
/// asks a non-statistician to absorb, and it is meant to stay short enough to do that.
pub const TERMS: &[Term] = &[
    Term {
        phrase: "series",
        definition: "One metric of one benchmark, tracked across commits.",
        formal_name: "",
        chapter: "reconstruction.md",
    },
    Term {
        phrase: "detector",
        definition: "The procedure that examines one series for one shape of change: a \
                     change point, a drift, or a branch comparison.",
        formal_name: "",
        chapter: "detection.md",
    },
    Term {
        phrase: "level",
        definition: "The value a series sits at over a stretch of commits, ignoring \
                     run-to-run scatter.",
        formal_name: "",
        chapter: "detection.md",
    },
    Term {
        phrase: "scatter",
        definition: "Between-commit variation in a series when nothing has changed.",
        formal_name: "",
        chapter: "detection.md",
    },
    Term {
        phrase: "median",
        definition: "The middle value of a sample, which a few extreme measurements \
                     cannot drag around.",
        formal_name: "",
        chapter: "detection.md",
    },
    Term {
        phrase: "regime",
        definition: "A stretch of commits over which a series holds one level.",
        formal_name: "",
        chapter: "detection.md",
    },
    Term {
        phrase: "change point",
        definition: "The commit where a series stops holding one level and starts \
                     holding another.",
        formal_name: "",
        chapter: "detection.md",
    },
    Term {
        phrase: "drift",
        definition: "A series that moves steadily in one direction rather than stepping \
                     between levels.",
        formal_name: "monotonic trend",
        chapter: "detection.md",
    },
    Term {
        phrase: "rank comparison",
        definition: "A test for whether two regimes differ that weighs each measurement by \
                     its rank among all of them rather than by its size, so a few extreme \
                     values cannot dominate. Tested two-sided.",
        formal_name: "Mann-Whitney U test",
        chapter: "detection.md",
    },
    Term {
        phrase: "split search",
        definition: "A scan for the single most likely place a series changed level.",
        formal_name: "Pettitt test",
        chapter: "detection.md",
    },
    Term {
        phrase: "one-way-trend check",
        definition: "A test for whether a series mostly moves in one direction, which \
                     counts rises against falls rather than fitting a line.",
        formal_name: "Mann-Kendall test",
        chapter: "detection.md",
    },
    Term {
        phrase: "outlier-resistant slope",
        definition: "A trend line fitted from the middle of all the pairwise slopes, so a \
                     few odd measurements cannot tilt it.",
        formal_name: "Theil-Sen estimator",
        chapter: "detection.md",
    },
    Term {
        phrase: "chance level",
        definition: "How often pure chance alone would produce a pattern at least this \
                     strong.",
        formal_name: "p-value",
        chapter: "detection.md",
    },
    Term {
        phrase: "prediction interval",
        definition: "The range a single further measurement is expected to land in, given \
                     what the previous ones did.",
        formal_name: "Student's t prediction interval",
        chapter: "detection.md",
    },
    Term {
        phrase: "agreement share",
        definition: "The fraction of before-and-after pairs that agree the level moved in \
                     the same direction.",
        formal_name: "probability of superiority",
        chapter: "gates.md",
    },
    Term {
        phrase: "typical residual",
        definition: "How far a representative point sits from the model fitted to the series: \
                     a level for a change point or branch comparison, a line for a drift.",
        formal_name: "median absolute residual",
        chapter: "gates.md",
    },
    Term {
        phrase: "confidence interval",
        definition: "A range the benchmark engine reports alongside a measurement to say \
                     how precisely it pinned it down.",
        formal_name: "",
        chapter: "gates.md",
    },
    Term {
        phrase: "quantum",
        definition: "The smallest step a metric can actually take, such as one whole \
                     instruction.",
        formal_name: "",
        chapter: "gates.md",
    },
    Term {
        phrase: "false-discovery family",
        definition: "Every series that carried enough data to be tested, which is the \
                     group a finding has to stand out from.",
        formal_name: "",
        chapter: "coverage.md",
    },
    Term {
        phrase: "group-wide correction",
        definition: "A stricter bar applied when many things are tested at once, so that \
                     only a small share of what is reported is expected to be wrong.",
        formal_name: "Benjamini-Hochberg false discovery rate control",
        chapter: "coverage.md",
    },
    Term {
        phrase: "ghost",
        definition: "A benchmark that history remembers but the analyzed commit no longer \
                     measures.",
        formal_name: "",
        chapter: "reconstruction.md",
    },
    Term {
        phrase: "blessing",
        definition: "A recorded decision to treat a change as accepted, so history stops \
                     reporting it.",
        formal_name: "",
        chapter: "reconstruction.md",
    },
    Term {
        phrase: "discriminant set",
        definition: "The engine, target triple, and machine key a run was measured with, which \
                     together decide what it may be compared against.",
        formal_name: "",
        chapter: "shape.md",
    },
    Term {
        phrase: "harvest",
        definition: "Reading whatever output the benchmark engines left behind after a \
                     run.",
        formal_name: "",
        chapter: "collection.md",
    },
    Term {
        phrase: "machine key",
        definition: "A fingerprint of the host hardware, used to keep incomparable \
                     results apart.",
        formal_name: "",
        chapter: "collection.md",
    },
    Term {
        phrase: "dirty run",
        definition: "A measurement taken with uncommitted changes in the working tree.",
        formal_name: "",
        chapter: "collection.md",
    },
    Term {
        phrase: "partition",
        definition: "One discriminant set's slice of the store.",
        formal_name: "",
        chapter: "collection.md",
    },
    Term {
        phrase: "merge base",
        definition: "The newest commit a branch and its base still share.",
        formal_name: "",
        chapter: "selection.md",
    },
    Term {
        phrase: "finding",
        definition: "A move that survived detection, every gate, and the group-wide \
                     correction.",
        formal_name: "",
        chapter: "reporting.md",
    },
    Term {
        phrase: "census",
        definition: "The report's account of how many series it judged, and why it did \
                     not judge the rest.",
        formal_name: "",
        chapter: "reporting.md",
    },
    Term {
        phrase: "comparison-base lag",
        definition: "A branch comparison made against base data from several commits \
                     back.",
        formal_name: "",
        chapter: "reporting.md",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_phrase_is_defined_twice() {
        let mut phrases: Vec<&str> = TERMS.iter().map(|term| term.phrase).collect();
        phrases.sort_unstable();
        let count = phrases.len();
        phrases.dedup();

        assert_eq!(count, phrases.len(), "a phrase is defined more than once");
    }

    #[test]
    fn every_definition_is_a_sentence() {
        for term in TERMS {
            assert!(
                term.definition.ends_with('.'),
                "the definition of '{}' is not a sentence",
                term.phrase
            );
            assert!(
                !term.definition.is_empty(),
                "'{}' has no definition",
                term.phrase
            );
        }
    }

    #[test]
    fn every_term_names_a_chapter_that_introduces_it() {
        for term in TERMS {
            assert!(
                term.chapter.ends_with(".md"),
                "'{}' does not name a chapter",
                term.phrase
            );
        }
    }
}
