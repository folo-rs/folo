//! Byte-bounded lookahead for ordered patch-content requests.

use std::collections::{HashMap, HashSet};
use std::num::NonZero;
use std::rc::Rc;

use ohno::AppError;

use crate::git::blob_batch::InvalidBlobBatch;

/// Limits retained lookahead, not accepted file sizes or the renderer's current comparison.
///
/// A modest buffer amortizes Git startup across ordinary source files without retaining a
/// package's entire changed contents. Ref: docs/implementation.md, "Patch content acquisition".
pub const BLOB_BATCH_BYTES: NonZero<usize> =
    NonZero::new(1_048_576).expect("the fixed lookahead budget is nonzero");

/// Caps framing and map overhead even when payloads are empty.
/// This is secondary to the byte budget, not a file-count admission limit.
const MAX_BATCH_OBJECTS: usize = 1024;

/// Ordered object requests with at most one byte-budgeted batch retained.
///
/// Callers may retain the current file's two endpoints while the next batch is acquired.
/// Everything else is released on batch advance; oversized objects occupy a batch alone.
#[derive(Debug)]
pub struct BlobReader<'a> {
    requests: &'a [&'a str],
    sizes: HashMap<&'a str, usize>,
    budget: NonZero<usize>,
    position: usize,
    end: usize,
    retained: HashMap<&'a str, Rc<[u8]>>,
}

impl<'a> BlobReader<'a> {
    pub fn new(
        requests: &'a [&'a str],
        budget: NonZero<usize>,
        sizes: impl FnOnce(&[&str]) -> Result<Vec<usize>, AppError>,
    ) -> Result<Self, AppError> {
        let mut unique = Vec::new();
        let mut known = HashMap::new();
        for id in requests {
            if !known.contains_key(id) {
                unique.push(*id);
                // A single distinct object needs no size query: it occupies its own batch.
                known.insert(*id, usize::MAX);
            }
        }
        if unique.len() > 1 {
            let sizes = sizes(&unique)?;
            if sizes.len() != unique.len() {
                return Err(InvalidBlobBatch::new().into());
            }
            known = unique.into_iter().zip(sizes).collect();
        }
        Ok(Self {
            requests,
            sizes: known,
            budget,
            position: 0,
            end: 0,
            retained: HashMap::new(),
        })
    }

    /// Consumes the next planned identity, acquiring only the next bounded batch if needed.
    pub fn read(
        &mut self,
        id: &str,
        acquire: impl FnOnce(&[&str]) -> Result<Vec<Vec<u8>>, AppError>,
    ) -> Result<Rc<[u8]>, AppError> {
        if self.requests.get(self.position) != Some(&id) {
            return Err(InvalidBlobBatch::new().into());
        }
        if self.position == self.end {
            self.retained.clear();
            let (ids, end) = self.next_batch();
            let blobs = acquire(&ids)?;
            if blobs.len() != ids.len() {
                return Err(InvalidBlobBatch::new().into());
            }
            for (id, blob) in ids.into_iter().zip(blobs) {
                if self.sizes.len() > 1 && self.sizes.get(id) != Some(&blob.len()) {
                    return Err(InvalidBlobBatch::new().into());
                }
                self.retained.insert(id, Rc::from(blob));
            }
            self.end = end;
        }
        let blob = self
            .retained
            .get(id)
            .cloned()
            .ok_or_else(InvalidBlobBatch::new)?;
        self.position = self
            .position
            .checked_add(1)
            .expect("position is within requests");
        Ok(blob)
    }

    fn next_batch(&self) -> (Vec<&'a str>, usize) {
        let mut ids = Vec::new();
        let mut included = HashSet::new();
        let mut bytes = 0_usize;
        let mut end = self.position;
        for id in self.requests.iter().skip(self.position) {
            if included.insert(*id) {
                let size = *self
                    .sizes
                    .get(id)
                    .expect("every request has an acquired size");
                // Saturating arithmetic avoids overflow for the permitted single oversized blob.
                if !ids.is_empty()
                    && (ids.len() == MAX_BATCH_OBJECTS
                        || bytes > self.budget.get()
                        || size > self.budget.get().saturating_sub(bytes))
                {
                    break;
                }
                ids.push(*id);
                bytes = bytes.saturating_add(size);
            }
            end = end.checked_add(1).expect("end is within requests");
        }
        (ids, end)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io;
    use std::rc::Weak;

    use super::*;

    #[test]
    fn batches_deduplicate_and_release_before_acquiring_the_next_content() {
        let requests = ["a", "b", "a", "c", "c", "b"];
        let mut reader = BlobReader::new(&requests, NonZero::new(4).unwrap(), |ids| {
            assert_eq!(ids, ["a", "b", "c"]);
            Ok(vec![2, 2, 7])
        })
        .unwrap();
        let mut batches = Vec::new();
        let mut previous = Vec::new();
        for id in requests {
            let blob = reader
                .read(id, |ids| {
                    assert!(
                        previous
                            .iter()
                            .all(|weak: &Weak<[u8]>| weak.upgrade().is_none())
                    );
                    batches.push(ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>());
                    Ok(ids
                        .iter()
                        .map(|id| vec![0; if *id == "c" { 7 } else { 2 }])
                        .collect())
                })
                .unwrap();
            previous = vec![Rc::downgrade(&blob)];
            assert_eq!(blob.len(), if id == "c" { 7 } else { 2 });
        }
        assert_eq!(batches, [vec!["a", "b"], vec!["c"], vec!["b"]]);
    }

    #[test]
    fn empty_single_and_duplicate_requests_do_not_query_sizes() {
        BlobReader::new(&[], BLOB_BATCH_BYTES, |_| panic!("empty")).unwrap();
        let mut reader = BlobReader::new(&["a", "a"], BLOB_BATCH_BYTES, |_| panic!("one")).unwrap();
        let first = reader
            .read("a", |ids| {
                assert_eq!(ids, ["a"]);
                Ok(vec![vec![]])
            })
            .unwrap();
        let second = reader.read("a", |_| panic!("retained")).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        reader.read("a", |_| panic!("exhausted")).unwrap_err();
    }

    #[test]
    fn size_and_content_failures_never_produce_a_successful_batch() {
        BlobReader::new(&["a", "b"], BLOB_BATCH_BYTES, |_| {
            Err(io::Error::other("size").into())
        })
        .err()
        .unwrap();
        BlobReader::new(&["a", "b"], BLOB_BATCH_BYTES, |_| Ok(vec![0]))
            .err()
            .unwrap();
        let mut reader =
            BlobReader::new(&["a", "b"], BLOB_BATCH_BYTES, |_| Ok(vec![0, 1])).unwrap();
        reader.read("b", |_| panic!("out of order")).unwrap_err();
        reader
            .read("a", |_| Err(io::Error::other("read").into()))
            .unwrap_err();
        reader.read("a", |_| Ok(vec![])).unwrap_err();
        reader.read("a", |_| Ok(vec![vec![], vec![]])).unwrap_err();
        assert!(
            reader
                .read("a", |_| Ok(vec![vec![], vec![1]]))
                .unwrap()
                .is_empty()
        );
        assert_eq!(*reader.read("b", |_| panic!("retained")).unwrap(), [1]);
    }

    #[test]
    fn oversized_objects_do_not_share_a_batch_even_with_empty_objects() {
        let mut reader =
            BlobReader::new(&["a", "b"], NonZero::new(1).unwrap(), |_| Ok(vec![2, 0])).unwrap();
        assert_eq!(
            *reader
                .read("a", |ids| {
                    assert_eq!(ids, ["a"]);
                    Ok(vec![vec![1, 2]])
                })
                .unwrap(),
            [1, 2]
        );
        assert!(
            reader
                .read("b", |ids| {
                    assert_eq!(ids, ["b"]);
                    Ok(vec![vec![]])
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn empty_objects_fit_after_an_exact_budget_payload() {
        let mut reader =
            BlobReader::new(&["a", "b"], NonZero::new(1).unwrap(), |_| Ok(vec![1, 0])).unwrap();
        assert_eq!(
            *reader
                .read("a", |ids| {
                    assert_eq!(ids, ["a", "b"]);
                    Ok(vec![vec![1], vec![]])
                })
                .unwrap(),
            [1]
        );
        assert!(reader.read("b", |_| panic!("retained")).unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "native-scale metadata limit; small batch tests retain scheduler coverage"
    )]
    fn empty_payloads_still_bound_retained_metadata() {
        let ids: Vec<_> = (0..=MAX_BATCH_OBJECTS)
            .map(|index| index.to_string())
            .collect();
        let ids: Vec<_> = ids.iter().map(String::as_str).collect();
        let reader = BlobReader::new(&ids, BLOB_BATCH_BYTES, |ids| Ok(vec![0; ids.len()])).unwrap();
        let (batch, end) = reader.next_batch();
        assert_eq!(batch, ids.get(..MAX_BATCH_OBJECTS).unwrap());
        assert_eq!(end, MAX_BATCH_OBJECTS);
    }
}
