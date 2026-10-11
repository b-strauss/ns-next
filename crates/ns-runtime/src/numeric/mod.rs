//! Numbers with no heap and no `Value`: what the numeric leaves compute
//! and write, kept apart so that it is tested as plain Rust and so that
//! `Heap::describe` can write a float as `Float>>printString` does.
//! `arbitrary_integer` beside it is the integers' arithmetic.

pub(crate) mod float_text;
