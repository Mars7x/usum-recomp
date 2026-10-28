//! Static recompilation of ARM code to Rust.
//!
//! [`discover`] finds functions and labels by recursive descent; [`emit`]
//! writes them as Rust source against `usum_guest`. The output is plain
//! text: compiling it is the caller's job (a build script in tests, the
//! local build in the product).

pub mod discover;
pub mod emit;

pub use discover::{analyze, analyze_with, Analysis, Function, Image, Origin};

/// Translates the code reachable from `entries` into a Rust module body.
pub fn translate(image: &Image, entries: &[u32]) -> (Analysis, String) {
    let analysis = analyze(image, entries);
    let source = emit::module(&analysis);
    (analysis, source)
}
