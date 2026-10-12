//! Frame accumulation independent of native I/O and real-time deadlines.

use crate::pal::error::{PalError, PalErrorKind};
use crate::protocol::{Message, decode_body, payload_len_ok};

/// Receives one bounded frame from a reader that may return partial progress.
///
/// The reader owns cancellation and the deadline shared by every call. Its
/// errors pass through unchanged; no frame storage is reused while it is borrowed.
pub(crate) fn read_frame(
    mut read: impl FnMut(&mut [u8]) -> Result<usize, PalError>,
) -> Result<Message, PalError> {
    let mut header = [0_u8; size_of::<u32>()];
    fill(&mut header, &mut read)?;
    let len = u32::from_le_bytes(header);
    if !payload_len_ok(len) {
        return Err(PalError::new(PalErrorKind::Other));
    }
    // Reading the kind separately lets the decoded message own the body allocation.
    let mut kind = [0_u8; 1];
    fill(&mut kind, &mut read)?;
    let body_len = (len as usize)
        .checked_sub(kind.len())
        .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
    let mut body = vec![0_u8; body_len];
    fill(&mut body, &mut read)?;
    decode_body(kind[0], body).map_err(|_error| PalError::new(PalErrorKind::Other))
}

/// Accumulates a frame component while requiring bounded, nonzero progress.
fn fill(
    mut remaining: &mut [u8],
    read: &mut impl FnMut(&mut [u8]) -> Result<usize, PalError>,
) -> Result<(), PalError> {
    while !remaining.is_empty() {
        let count = read(remaining)?;
        if count == 0 {
            return Err(PalError::new(PalErrorKind::Disconnected));
        }
        remaining = remaining
            .get_mut(count..)
            .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
    }
    Ok(())
}
