// Renders released-file changes as unified diffs for report artifacts.

use std::fmt::Write as _;
use std::iter::repeat_n;

use crate::quote_path;

/// One file's rendered change together with its line statistics.
///
/// `report` writes the rendered text into a package's `.patch` artifact and
/// accumulates the counts into the report's `stat` object, so both travel
/// together out of the renderer.
pub(crate) struct FileDiff {
    pub(crate) text: String,
    pub(crate) insertions: usize,
    pub(crate) deletions: usize,
}

/// Renders one released file's change, or reports that it is binary.
pub(crate) fn file_diff(path: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> FileDiff {
    let (old_present, new_present) = (old.is_some(), new.is_some());
    // A unified diff can only describe text, and validating here lets both sides
    // be borrowed rather than copied out of a lossy conversion.
    let (Ok(old), Ok(new)) = (
        str::from_utf8(old.unwrap_or_default()),
        str::from_utf8(new.unwrap_or_default()),
    ) else {
        return binary_diff(path, old_present, new_present);
    };
    // A NUL byte is valid UTF-8 but not something a patch reader can consume, so
    // it is the same signal Git uses to call a file binary.
    if old.contains('\0') || new.contains('\0') {
        return binary_diff(path, old_present, new_present);
    }
    unified_diff(path, old, old_present, new, new_present)
}

/// Reports a change the unified format cannot describe.
fn binary_diff(path: &str, old_present: bool, new_present: bool) -> FileDiff {
    FileDiff {
        // The same side labels as a text diff, so a consumer reads an added or
        // deleted binary file as such rather than as a modification.
        text: format!(
            "Binary files {} and {} differ\n",
            side_label(old_present, "a", path),
            side_label(new_present, "b", path)
        ),
        insertions: 0,
        deletions: 0,
    }
}

/// Renders one file's change as a zero-context unified diff.
///
/// Consumers pipe `.patch` artifacts into standard tooling, so the output
/// follows the unified format that `diff -U0` produces: `/dev/null` for the
/// absent side of an addition or deletion, one hunk per changed region, and a
/// hunk header whose line numbers use the preceding line when a side
/// contributes no lines.
fn unified_diff(
    path: &str,
    old: &str,
    old_present: bool,
    new: &str,
    new_present: bool,
) -> FileDiff {
    // `split_inclusive` keeps line terminators, so a change that only adds or
    // removes a trailing newline is still visible as a differing line.
    let old: Vec<&str> = old.split_inclusive('\n').collect();
    let new: Vec<&str> = new.split_inclusive('\n').collect();
    let regions = changed_regions(&old, &new, MAX_EDIT_DISTANCE);

    let mut hunks = String::new();
    let mut insertions = 0_usize;
    let mut deletions = 0_usize;
    for region in &regions {
        let old_region = old.get(region.old_start..region.old_end).unwrap_or(&[]);
        let new_region = new.get(region.new_start..region.new_end).unwrap_or(&[]);
        deletions = deletions.saturating_add(old_region.len());
        insertions = insertions.saturating_add(new_region.len());
        writeln!(
            hunks,
            "@@ -{},{} +{},{} @@",
            hunk_start(region.old_start, old_region.len()),
            old_region.len(),
            hunk_start(region.new_start, new_region.len()),
            new_region.len()
        )
        .expect("writing to String");
        for line in old_region {
            push_diff_line(&mut hunks, '-', line);
        }
        for line in new_region {
            push_diff_line(&mut hunks, '+', line);
        }
    }

    // A file that appears or disappears is a change even when it holds no
    // lines, so the headers alone record it and `diff_path` stays populated.
    if hunks.is_empty() && old_present == new_present {
        return FileDiff {
            text: String::new(),
            insertions: 0,
            deletions: 0,
        };
    }
    let old_label = side_label(old_present, "a", path);
    let new_label = side_label(new_present, "b", path);
    // A patch reader takes the work to do from the hunks, so an added or
    // deleted empty file — which has none — would read as a no-op and never be
    // created or removed. Git's extended headers carry that case instead, and
    // are emitted only for it: every other addition and deletion already
    // carries hunks that describe it.
    let extended = if hunks.is_empty() {
        empty_file_header(path, old_present)
    } else {
        String::new()
    };
    FileDiff {
        text: format!("{extended}--- {old_label}\n+++ {new_label}\n{hunks}"),
        insertions,
        deletions,
    }
}

/// Mode the extended headers record for an added or deleted empty file.
///
/// The renderer does not carry file modes, so this is Git's ordinary-file mode
/// rather than an observation. An empty blob cannot be a symbolic link, whose
/// blob holds its target, and a `.patch` artifact is read for the content
/// change rather than for permissions.
const EMPTY_FILE_MODE: &str = "100644";

/// Git's extended headers for an empty file's creation or deletion.
fn empty_file_header(path: &str, old_present: bool) -> String {
    let change = if old_present { "deleted" } else { "new" };
    // Each side is quoted the way the `---` and `+++` labels quote theirs, with
    // the prefix inside the quotes, so one reader handles every header here.
    let old_name = quote_path(&format!("a/{path}")).into_owned();
    let new_name = quote_path(&format!("b/{path}")).into_owned();
    format!("diff --git {old_name} {new_name}\n{change} file mode {EMPTY_FILE_MODE}\n")
}

/// A contiguous run of deleted and inserted lines, as zero-based line indexes.
struct ChangedRegion {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

/// Groups a minimal edit script into one region per changed area.
///
/// Splitting on unchanged lines is what turns separated edits into separate
/// hunks; treating the whole span between the first and last difference as one
/// region would report unchanged lines as deleted and reinserted.
fn changed_regions<'a>(
    old: &[&'a str],
    new: &[&'a str],
    max_distance: usize,
) -> Vec<ChangedRegion> {
    let script = edit_script(old, new, max_distance);
    let mut regions: Vec<ChangedRegion> = Vec::new();
    let mut old_index = 0_usize;
    let mut new_index = 0_usize;
    let mut open = false;
    for edit in script {
        match edit {
            Edit::Keep => {
                open = false;
                old_index = old_index.saturating_add(1);
                new_index = new_index.saturating_add(1);
            }
            Edit::Delete | Edit::Insert => {
                if !open {
                    regions.push(ChangedRegion {
                        old_start: old_index,
                        old_end: old_index,
                        new_start: new_index,
                        new_end: new_index,
                    });
                    open = true;
                }
                let region = regions.last_mut().expect("a region was just opened");
                if matches!(edit, Edit::Delete) {
                    old_index = old_index.saturating_add(1);
                    region.old_end = old_index;
                } else {
                    new_index = new_index.saturating_add(1);
                    region.new_end = new_index;
                }
            }
        }
    }
    regions
}

/// One step of a line-level edit script.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Edit {
    Keep,
    Delete,
    Insert,
}

/// Edit-distance budget before the renderer stops refining.
///
/// Myers' algorithm records one trace row per edit step, and each row holds one
/// entry per reachable diagonal, so its cost grows with the number of differing
/// lines rather than with file size. Bounding the distance keeps a pair of large
/// unrelated files from costing quadratic memory while still rendering ordinary
/// hunks for a large file that differs in only a few lines. A single released
/// file changes by far less than this within a release cycle.
const MAX_EDIT_DISTANCE: usize = 1024;

/// Computes a minimal line-level edit script with Myers' algorithm.
///
/// Falls back to replacing the whole file when the sides differ by more than
/// `max_distance` edits; the result is still a valid unified diff, just a
/// coarser one. Ref: docs/implementation.md, "Released content".
fn edit_script(old: &[&str], new: &[&str], max_distance: usize) -> Vec<Edit> {
    // Deleting every old line and inserting every new one is always an edit
    // script, so the distance never exceeds that; searching further is wasted.
    let budget = old.len().saturating_add(new.len()).min(max_distance);
    let Some(trace) = myers_trace(old, new, budget) else {
        return whole_file_script(old.len(), new.len());
    };
    backtrack(&trace, budget, old.len(), new.len())
}

fn whole_file_script(old_len: usize, new_len: usize) -> Vec<Edit> {
    let mut script = Vec::with_capacity(old_len.saturating_add(new_len));
    script.extend(repeat_n(Edit::Delete, old_len));
    script.extend(repeat_n(Edit::Insert, new_len));
    script
}

/// Records the furthest-reaching path on each diagonal after every edit.
///
/// Myers' algorithm treats the two sides as a grid whose diagonals are the runs
/// of shared lines. A path through the grid is an edit script: moving right
/// deletes an old line, moving down inserts a new one, and moving along a
/// diagonal keeps a line that both sides share. Diagonal `k` holds the paths
/// where the old index exceeds the new index by `k`, so a single number
/// identifies a diagonal and the old index alone locates a position on it.
///
/// The search proceeds by edit count. After `d` edits only diagonals `-d..=d`
/// are reachable, and on each of those only the path that has advanced furthest
/// can still lead to a minimal script, so one entry per diagonal suffices.
/// Extending to `d + 1` edits reaches diagonal `k` either by inserting from
/// `k + 1` or by deleting from `k - 1`; the furthest-reaching of the two is
/// taken, and the path then runs freely along the diagonal for as long as the
/// two sides agree, which costs no edits. Reaching the far corner means an
/// alignment exists within `d` edits, and the recorded rows are what
/// [`backtrack`] later replays.
///
/// Positions are held as signed values because a diagonal index is the signed
/// difference between the two sides' line indexes. Reports nothing when the two
/// sides are still not aligned after `budget` edits.
fn myers_trace(old: &[&str], new: &[&str], budget: usize) -> Option<Vec<Vec<isize>>> {
    let offset = isize::try_from(budget).ok()?;
    let old_len = isize::try_from(old.len()).ok()?;
    let new_len = isize::try_from(new.len()).ok()?;
    let width = budget.checked_mul(2)?.checked_add(1)?;
    let mut reach = vec![0_isize; width];
    let mut trace = Vec::new();
    for edits in 0..=budget {
        trace.push(reach.clone());
        let edits = isize::try_from(edits).ok()?;
        let mut diagonal = edits.checked_neg()?;
        while diagonal <= edits {
            let index = usize::try_from(diagonal.checked_add(offset)?).ok()?;
            let mut old_index = if takes_insertion(&reach, index, diagonal, edits) {
                *reach.get(index.checked_add(1)?)?
            } else {
                reach.get(index.checked_sub(1)?)?.checked_add(1)?
            };
            let mut new_index = old_index.checked_sub(diagonal)?;
            let shared = common_prefix(old, old_index, new, new_index);
            old_index = old_index.checked_add(shared)?;
            new_index = new_index.checked_add(shared)?;
            *reach.get_mut(index)? = old_index;
            if old_index >= old_len && new_index >= new_len {
                return Some(trace);
            }
            diagonal = diagonal.checked_add(2)?;
        }
    }
    None
}

/// Counts the leading lines both sides share from these positions.
///
/// The count is taken over zipped remainders rather than by advancing a pair of
/// cursors under a compound condition, so it is bounded by the shorter
/// remainder regardless of what the line comparison reports.
fn common_prefix(old: &[&str], old_index: isize, new: &[&str], new_index: isize) -> isize {
    let shared = remaining(old, old_index)
        .iter()
        .zip(remaining(new, new_index))
        .take_while(|(old_line, new_line)| old_line == new_line)
        .count();
    isize::try_from(shared).unwrap_or(0)
}

/// The lines at and after `index`, or nothing when `index` is out of range.
fn remaining<'a, 'b>(lines: &'b [&'a str], index: isize) -> &'b [&'a str] {
    usize::try_from(index)
        .ok()
        .and_then(|index| lines.get(index..))
        .unwrap_or(&[])
}

/// Decides whether the furthest path on this diagonal arrives by insertion.
///
/// Diagonal `k` is entered from `k + 1` by an insertion or from `k - 1` by a
/// deletion, and the one that has advanced further along the old side is the
/// one a minimal script takes. At the two extremes only one predecessor exists:
/// the lowest reachable diagonal has no `k - 1` to delete from, and the highest
/// has no `k + 1` to insert from. The same rule decides the step forwards in
/// [`myers_trace`] and the step backwards in [`backtrack`], so both agree on the
/// path without recording it.
fn takes_insertion(reach: &[isize], index: usize, diagonal: isize, edits: isize) -> bool {
    if diagonal == edits.saturating_neg() {
        return true;
    }
    if diagonal == edits {
        return false;
    }
    let before = index
        .checked_sub(1)
        .and_then(|index| reach.get(index))
        .copied()
        .unwrap_or(0);
    let after = index
        .checked_add(1)
        .and_then(|index| reach.get(index))
        .copied()
        .unwrap_or(0);
    before < after
}

/// Walks the recorded trace backwards to recover the edit script.
///
/// [`myers_trace`] records where each edit step reached but not how it got
/// there, so the script is recovered by replaying the same choice in reverse.
/// Starting at the far corner, each step identifies the diagonal the current
/// position sits on, asks which predecessor that step came from, and reads the
/// predecessor's recorded position. The gap between the two positions is a run
/// of lines both sides share, emitted as keeps, and the step itself is the one
/// insertion or deletion that separates the two diagonals. The script is built
/// from the end and reversed once at the finish.
fn backtrack(trace: &[Vec<isize>], budget: usize, old_len: usize, new_len: usize) -> Vec<Edit> {
    let offset = isize::try_from(budget).unwrap_or(0);
    let mut script = Vec::new();
    let mut old_index = isize::try_from(old_len).unwrap_or(0);
    let mut new_index = isize::try_from(new_len).unwrap_or(0);
    for (edits, reach) in trace.iter().enumerate().rev() {
        let edits = isize::try_from(edits).unwrap_or(0);
        let diagonal = old_index.saturating_sub(new_index);
        let index = usize::try_from(diagonal.saturating_add(offset)).unwrap_or(0);
        let insertion = takes_insertion(reach, index, diagonal, edits);
        let previous_diagonal = if insertion {
            diagonal.saturating_add(1)
        } else {
            diagonal.saturating_sub(1)
        };
        let previous_index = usize::try_from(previous_diagonal.saturating_add(offset)).unwrap_or(0);
        let previous_old = reach.get(previous_index).copied().unwrap_or(0);
        let previous_new = previous_old.saturating_sub(previous_diagonal);
        // Counted rather than looped so no mutation of the bound can run away:
        // the run of kept lines is the shorter distance back to the previous
        // furthest-reaching point on either side.
        let keeps = old_index
            .saturating_sub(previous_old)
            .min(new_index.saturating_sub(previous_new))
            .max(0);
        let keeps = usize::try_from(keeps).unwrap_or(0);
        script.extend(repeat_n(Edit::Keep, keeps));
        let stepped = isize::try_from(keeps).unwrap_or(0);
        old_index = old_index.saturating_sub(stepped);
        new_index = new_index.saturating_sub(stepped);
        if edits == 0 {
            break;
        }
        if insertion {
            script.push(Edit::Insert);
            new_index = new_index.saturating_sub(1);
        } else {
            script.push(Edit::Delete);
            old_index = old_index.saturating_sub(1);
        }
    }
    script.reverse();
    script
}

/// Reports the first line number a hunk side covers.
fn hunk_start(zero_based: usize, len: usize) -> usize {
    if len == 0 {
        // A side that contributes no lines anchors on the preceding line, which
        // is what `diff -U0` emits and what patch readers expect.
        zero_based
    } else {
        zero_based.saturating_add(1)
    }
}

fn side_label(present: bool, prefix: &str, path: &str) -> String {
    if present {
        // Git quotes the prefixed name as a unit, so `a/` sits inside the quotes.
        quote_path(&format!("{prefix}/{path}")).into_owned()
    } else {
        // The unified-diff placeholder for the absent side of an add or delete.
        "/dev/null".to_string()
    }
}

fn push_diff_line(out: &mut String, marker: char, line: &str) {
    out.push(marker);
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn render(old: Option<&str>, new: Option<&str>) -> FileDiff {
        file_diff("src/lib.rs", old.map(str::as_bytes), new.map(str::as_bytes))
    }

    fn count(script: &[Edit], wanted: Edit) -> usize {
        script.iter().filter(|edit| **edit == wanted).count()
    }

    #[test]
    fn an_addition_reports_the_absent_side_as_dev_null() {
        let diff = render(None, Some("a\n"));
        assert!(diff.text.starts_with("--- /dev/null\n+++ b/src/lib.rs\n"));
        assert!(diff.text.contains("@@ -0,0 +1,1 @@\n+a\n"));
        assert_eq!((diff.insertions, diff.deletions), (1, 0));
    }

    #[test]
    fn a_diff_header_quotes_an_unusual_file_name() {
        let diff = file_diff("od\"d.rs", Some(b"a\n"), Some(b"b\n"));
        assert!(
            diff.text.starts_with("--- \"a/od\\\"d.rs\"\n"),
            "{}",
            diff.text
        );
        assert!(
            diff.text.contains("+++ \"b/od\\\"d.rs\"\n"),
            "{}",
            diff.text
        );
    }

    #[test]
    fn a_binary_diff_quotes_an_unusual_file_name() {
        let diff = file_diff("od\"d.bin", Some(b"\0"), Some(b"\0\0"));
        assert!(
            diff.text
                .contains("Binary files \"a/od\\\"d.bin\" and \"b/od\\\"d.bin\" differ"),
            "{}",
            diff.text
        );
    }

    #[test]
    fn a_deletion_reports_the_absent_side_as_dev_null() {
        let diff = render(Some("a\n"), None);
        assert!(diff.text.starts_with("--- a/src/lib.rs\n+++ /dev/null\n"));
        assert!(diff.text.contains("@@ -1,1 +0,0 @@\n-a\n"));
        assert_eq!((diff.insertions, diff.deletions), (0, 1));
    }

    #[test]
    fn separated_edits_render_as_separate_hunks() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\n";
        let diff = render(Some(old), Some(new));
        assert!(
            diff.text.contains("@@ -2,1 +2,1 @@\n-b\n+B\n"),
            "{}",
            diff.text
        );
        assert!(
            diff.text.contains("@@ -5,1 +5,1 @@\n-e\n+E\n"),
            "{}",
            diff.text
        );
        // The unchanged lines between the edits are not reported as changes.
        assert_eq!((diff.insertions, diff.deletions), (2, 2));
    }

    #[test]
    fn an_empty_file_addition_still_renders_headers() {
        // A reader takes its work from the hunks, and an empty file has none,
        // so the creation is carried by Git's extended headers instead.
        let diff = render(None, Some(""));
        assert_eq!(
            diff.text,
            "diff --git a/src/lib.rs b/src/lib.rs\nnew file mode 100644\n--- /dev/null\n+++ \
             b/src/lib.rs\n"
        );
        assert_eq!((diff.insertions, diff.deletions), (0, 0));
    }

    #[test]
    fn an_empty_file_deletion_still_renders_headers() {
        let diff = render(Some(""), None);
        assert_eq!(
            diff.text,
            "diff --git a/src/lib.rs b/src/lib.rs\ndeleted file mode 100644\n--- a/src/lib.rs\n+++ \
             /dev/null\n"
        );
    }

    #[test]
    fn a_non_empty_addition_carries_no_extended_headers() {
        // Its hunk already tells a reader to create the file.
        let diff = render(None, Some("a\n"));
        assert_eq!(
            diff.text,
            "--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1,1 @@\n+a\n"
        );
    }

    #[test]
    fn identical_text_renders_nothing() {
        let diff = render(Some("a\n"), Some("a\n"));
        assert!(diff.text.is_empty());
        assert_eq!((diff.insertions, diff.deletions), (0, 0));
    }

    #[test]
    fn a_missing_trailing_newline_is_a_visible_change() {
        let diff = render(Some("a\n"), Some("a"));
        assert!(diff.text.contains("\\ No newline at end of file"));
        assert_eq!((diff.insertions, diff.deletions), (1, 1));
    }

    #[test]
    fn binary_content_is_reported_without_a_hunk() {
        let diff = file_diff("data.bin", Some(&[0xff, 0xfe]), Some(&[0xfe, 0xff]));
        assert_eq!(diff.text, "Binary files a/data.bin and b/data.bin differ\n");
        assert_eq!((diff.insertions, diff.deletions), (0, 0));
    }

    #[test]
    fn an_insertion_between_unchanged_lines_deletes_nothing() {
        let diff = render(Some("a\nb\n"), Some("a\nx\nb\n"));
        assert!(diff.text.contains("@@ -1,0 +2,1 @@\n+x\n"), "{}", diff.text);
        assert_eq!((diff.insertions, diff.deletions), (1, 0));
    }

    #[test]
    fn common_prefix_counts_shared_leading_lines() {
        let old = ["a", "b", "c"];
        let new = ["a", "b", "d"];
        assert_eq!(common_prefix(&old, 0, &new, 0), 2);
        assert_eq!(common_prefix(&old, 1, &new, 0), 0);
        // Positions outside either side share nothing.
        assert_eq!(common_prefix(&old, 9, &new, 0), 0);
        assert_eq!(common_prefix(&old, -1, &new, 0), 0);
    }

    /// Two entirely unrelated files exceed the edit-distance budget, so the
    /// renderer stops refining and replaces one side with the other.
    #[test]
    fn exceeding_the_edit_distance_budget_falls_back_to_a_whole_file_replacement() {
        // Disjoint sides of four lines each need eight edits, which the budget
        // used here forbids.
        let old = ["a\n", "a\n", "a\n", "a\n"];
        let new = ["b\n", "b\n", "b\n", "b\n"];
        let script = edit_script(&old, &new, 4);
        assert_eq!(count(&script, Edit::Delete), old.len());
        assert_eq!(count(&script, Edit::Insert), new.len());
        assert_eq!(count(&script, Edit::Keep), 0);
    }

    /// A file far larger than the budget but differing in a single line has a
    /// tiny edit distance, so it is still rendered as one small hunk. The budget
    /// bounds the distance, not the input size.
    #[test]
    fn a_file_much_larger_than_the_budget_with_one_changed_line_stays_a_small_hunk() {
        let old = ["a\n"; 40];
        let mut new = old.to_vec();
        new.push("tail\n");
        let script = edit_script(&old, &new, 4);
        assert_eq!(count(&script, Edit::Keep), old.len());
        assert_eq!(count(&script, Edit::Insert), 1);
        assert_eq!(count(&script, Edit::Delete), 0);
    }

    /// The production budget renders an ordinary single-line change as one hunk.
    #[test]
    fn the_production_budget_renders_a_single_line_change_as_one_hunk() {
        let diff = render(Some("one\ntwo\n"), Some("one\ntoo\n"));
        assert_eq!((diff.insertions, diff.deletions), (1, 1));
    }

    #[test]
    fn nul_bytes_are_reported_as_binary_even_though_they_are_valid_utf8() {
        let diff = file_diff("data.dat", Some(b"one\n"), Some(b"one\0two\n"));
        assert_eq!(diff.text, "Binary files a/data.dat and b/data.dat differ\n");
        assert_eq!((diff.insertions, diff.deletions), (0, 0));
    }

    #[test]
    fn binary_changes_use_the_dev_null_placeholder_for_an_absent_side() {
        let added = file_diff("logo.png", None, Some(&[0xFF, 0xFE, 0x00]));
        assert_eq!(added.text, "Binary files /dev/null and b/logo.png differ\n");

        let deleted = file_diff("logo.png", Some(&[0xFF, 0xFE, 0x00]), None);
        assert_eq!(
            deleted.text,
            "Binary files a/logo.png and /dev/null differ\n"
        );

        let modified = file_diff("logo.png", Some(&[0xFF, 0x00]), Some(&[0xFF, 0x01]));
        assert_eq!(
            modified.text,
            "Binary files a/logo.png and b/logo.png differ\n"
        );
    }
    /// The Myers search gives up rather than overflow on a pathological input,
    /// and the fallback still has to describe a valid transformation: delete
    /// every old line, then insert every new one.
    #[test]
    fn the_whole_file_fallback_replaces_one_side_with_the_other() {
        let script = whole_file_script(2, 3);

        let deletes = script
            .iter()
            .take_while(|edit| matches!(**edit, Edit::Delete))
            .count();
        let inserts = script
            .iter()
            .filter(|edit| matches!(**edit, Edit::Insert))
            .count();
        assert_eq!(deletes, 2);
        assert_eq!(inserts, 3);
        // Deletions come first, so the trailing insertions are all that remain.
        assert_eq!(script.len(), deletes + inserts);
        assert!(whole_file_script(0, 0).is_empty());
    }
}
