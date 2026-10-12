//! The pretty printer: a module lowered to a [`Document`], then laid out.

mod document;

pub(crate) use document::{
    concat, group, hard_line, indent, layout, line, soft_line, text, Document,
};
