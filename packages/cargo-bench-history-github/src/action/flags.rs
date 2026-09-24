use std::ffi::OsString;

use ohno::AppError;

use crate::action::errors::InvalidInput;
use crate::action::port::Host;

/// Cargo's encoded flags preserve argument boundaries independently of whitespace.
pub(crate) const ENCODED_SEPARATOR: &str = "\u{1f}";

/// Appends validated additional arguments to Cargo's effective ambient compiler flags.
///
/// An absent input leaves inheritance untouched, including unreadable ambient values.
/// See docs/implementation.md, "Compiler-flag composition" for precedence and process isolation.
pub(crate) fn compiler_environment(
    additional: Option<&str>,
    host: &impl Host,
) -> Result<Vec<(OsString, OsString)>, AppError> {
    let Some(additional) = additional else {
        return Ok(Vec::new());
    };
    let mut encoded = match host.environment("CARGO_ENCODED_RUSTFLAGS")? {
        Some(encoded) => encoded,
        None => {
            let ambient = host.environment("RUSTFLAGS")?.unwrap_or_default();
            // Cargo's encoded representation cannot escape this character inside an argument.
            if ambient.contains(ENCODED_SEPARATOR) {
                return Err(InvalidInput::new(
                    "RUSTFLAGS",
                    "encoded argument separator is reserved",
                )
                .into());
            }
            ambient
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(ENCODED_SEPARATOR)
        }
    };
    for argument in additional.split_whitespace() {
        if !encoded.is_empty() {
            encoded.push_str(ENCODED_SEPARATOR);
        }
        encoded.push_str(argument);
    }
    Ok(vec![("CARGO_ENCODED_RUSTFLAGS".into(), encoded.into())])
}
