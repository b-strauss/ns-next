//! Hint A: a bare slot whose first value a statement assigns can take that
//! value in its declaration.

use super::walk::{Scope, Section};
use super::Hint;

pub(super) fn hints(
    _section: &Section<'_>,
    _scope: &Scope<'_>,
    _text: &str,
    _hints: &mut Vec<Hint>,
) {
}
