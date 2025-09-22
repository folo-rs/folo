// This test should fail to compile because we're trying to call a non-const method in nz! expression
use std::num::NonZero;
use new_zealand::nz;

fn get_value() -> u64 {
    42
}

fn main() {
    // This should fail to compile because function calls are not const expressions
    let _non_const: NonZero<u64> = nz!(get_value());
}