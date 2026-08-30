/// Non-ASCII text the helper prints so a test can prove the relay preserves
/// encoding from the app all the way to the user's console.
///
/// Box-drawing characters are what a TUI uses to frame its layout, and each one
/// is a multi-byte UTF-8 sequence. A console that decodes the relay's bytes
/// under an OEM code page turns every one of them into several unrelated
/// glyphs, so this text is absent from the output unless the whole chain agrees
/// on UTF-8 (`dure/docs/implementation.md`, "Console encoding").
pub const SAMPLE_NON_ASCII_TEXT: &str = "┌──┐";
