// Renders released-file changes as unified diffs for report artifacts.

use std::fmt::Write as _;

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
        // The same side labels as a text diff, so a consumer reads an added or
        // deleted binary file as such rather than as a modification.
        return FileDiff {
            text: format!(
                "Binary files {} and {} differ\n",
                side_label(old_present, "a", path),
                side_label(new_present, "b", path)
            ),
            insertions: 0,
            deletions: 0,
        };
    };
    unified_diff(path, old, old_present, new, new_present)
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
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let regions = changed_regions(&old_lines, &new_lines);

    let mut hunks = String::new();
    let mut insertions = 0_usize;
    let mut deletions = 0_usize;
    for region in &regions {
        let old_region = old_lines
            .get(region.old_start..region.old_end)
            .unwrap_or(&[]);
        let new_region = new_lines
            .get(region.new_start..region.new_end)
            .unwrap_or(&[]);
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
    FileDiff {
        text: format!("--- {old_label}\n+++ {new_label}\n{hunks}"),
        insertions,
        deletions,
    }
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
fn changed_regions<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<ChangedRegion> {
    let script = edit_script(old, new);
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
#[derive(Clone, Copy)]
enum Edit {
    Keep,
    Delete,
    Insert,
}

/// Longest-common-subsequence budget before the renderer stops refining.
///
/// Myers' algorithm stores one trace row per edit, so an adversarial pair of
/// large unrelated files would cost quadratic memory. Released source files
/// differ by far less than this within a single release cycle, and exceeding it
/// only coarsens the rendering into one whole-file hunk.
const MAX_EDIT_SCRIPT_LENGTH: usize = 4096;

/// Computes a minimal line-level edit script with Myers' algorithm.
///
/// Falls back to replacing the whole file when the script would exceed
/// `MAX_EDIT_SCRIPT_LENGTH`; the result is still a valid unified diff, just a
/// coarser one.
fn edit_script(old: &[&str], new: &[&str]) -> Vec<Edit> {
    let max = old.len().saturating_add(new.len());
    if max > MAX_EDIT_SCRIPT_LENGTH {
        return whole_file_script(old.len(), new.len());
    }
    let Some(trace) = myers_trace(old, new, max) else {
        return whole_file_script(old.len(), new.len());
    };
    backtrack(&trace, max, old.len(), new.len())
}

fn whole_file_script(old_len: usize, new_len: usize) -> Vec<Edit> {
    let mut script = Vec::with_capacity(old_len.saturating_add(new_len));
    script.extend(std::iter::repeat_n(Edit::Delete, old_len));
    script.extend(std::iter::repeat_n(Edit::Insert, new_len));
    script
}

/// Records the furthest-reaching path on each diagonal after every edit.
///
/// Positions are held as signed values because a diagonal index is the signed
/// difference between the two sides' line indexes.
fn myers_trace(old: &[&str], new: &[&str], max: usize) -> Option<Vec<Vec<isize>>> {
    let offset = isize::try_from(max).ok()?;
    let old_len = isize::try_from(old.len()).ok()?;
    let new_len = isize::try_from(new.len()).ok()?;
    let width = max.checked_mul(2)?.checked_add(1)?;
    let mut reach = vec![0_isize; width];
    let mut trace = Vec::new();
    for edits in 0..=max {
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
fn backtrack(trace: &[Vec<isize>], max: usize, old_len: usize, new_len: usize) -> Vec<Edit> {
    let offset = isize::try_from(max).unwrap_or(0);
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
        script.extend(std::iter::repeat_n(Edit::Keep, keeps));
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
        format!("{prefix}/{path}")
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

    #[test]
    fn an_addition_reports_the_absent_side_as_dev_null() {
        let diff = render(None, Some("a\n"));
        assert!(diff.text.starts_with("--- /dev/null\n+++ b/src/lib.rs\n"));
        assert!(diff.text.contains("@@ -0,0 +1,1 @@\n+a\n"));
        assert_eq!((diff.insertions, diff.deletions), (1, 0));
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
        let diff = render(None, Some(""));
        assert_eq!(diff.text, "--- /dev/null\n+++ b/src/lib.rs\n");
        assert_eq!((diff.insertions, diff.deletions), (0, 0));
    }

    #[test]
    fn an_empty_file_deletion_still_renders_headers() {
        let diff = render(Some(""), None);
        assert_eq!(diff.text, "--- a/src/lib.rs\n+++ /dev/null\n");
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

    #[test]
    fn oversized_inputs_fall_back_to_a_whole_file_replacement() {
        let old = "a\n".repeat(MAX_EDIT_SCRIPT_LENGTH);
        let new = "b\n".repeat(MAX_EDIT_SCRIPT_LENGTH);
        let diff = render(Some(&old), Some(&new));
        assert_eq!(
            (diff.insertions, diff.deletions),
            (MAX_EDIT_SCRIPT_LENGTH, MAX_EDIT_SCRIPT_LENGTH)
        );
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
}
