// Adapted from num-bigint (https://github.com/rust-num/num-bigint), commit
// 41f0b035d12971c5c7f7501917e31f0f6439c26c, src/biguint/addition.rs and
// src/biguint/subtraction.rs.
// Copyright (c) 2014 The Rust Project Developers. MIT licence; see
// THIRD-PARTY-NOTICES.md at the repository root. Changed for ns-next: names
// written out, `BigUint` replaced by digit slices, no `unsafe`.

//! Addition, subtraction and comparison of magnitudes: little-endian slices
//! of 64-bit digits. Only the portable carry and borrow bodies are taken;
//! upstream's x86 ones call `core::arch` intrinsics, which `ns-runtime`'s
//! `forbid(unsafe_code)` refuses.

use std::cmp::Ordering;

use super::{significant_length, trim_zero_top_digits};

/// `left + right + carry` into `output`, answering the carry out (0 or 1).
#[inline]
fn add_with_carry(carry: u8, left: u64, right: u64, output: &mut u64) -> u8 {
    let (sum, first_overflow) = left.overflowing_add(right);
    let (total, second_overflow) = sum.overflowing_add(u64::from(carry));
    *output = total;
    u8::from(first_overflow || second_overflow)
}

/// `left - right - borrow` into `output`, answering the borrow out (0 or 1).
#[inline]
fn subtract_with_borrow(borrow: u8, left: u64, right: u64, output: &mut u64) -> u8 {
    let (difference, first_overflow) = left.overflowing_sub(right);
    let (total, second_overflow) = difference.overflowing_sub(u64::from(borrow));
    *output = total;
    u8::from(first_overflow || second_overflow)
}

/// `accumulator += addend`, answering the carry out of the accumulator's top
/// digit. The accumulator must be at least as long as the addend.
#[inline]
pub(super) fn add_in_place_with_carry(accumulator: &mut [u64], addend: &[u64]) -> u64 {
    debug_assert!(accumulator.len() >= addend.len());

    let mut carry = 0;
    let (low, high) = accumulator.split_at_mut(addend.len());

    for (digit, &addend_digit) in low.iter_mut().zip(addend) {
        carry = add_with_carry(carry, *digit, addend_digit, digit);
    }

    if carry != 0 {
        for digit in high {
            carry = add_with_carry(carry, *digit, 0, digit);
            if carry == 0 {
                break;
            }
        }
    }

    u64::from(carry)
}

/// `accumulator += addend`, where the caller has made the accumulator long
/// enough that no carry leaves it.
pub(super) fn add_in_place(accumulator: &mut [u64], addend: &[u64]) {
    let carry = add_in_place_with_carry(accumulator, addend);
    debug_assert!(carry == 0);
}

/// `minuend -= subtrahend`, where the subtrahend is known to be no larger.
///
/// # Panics
///
/// When the subtrahend is larger: a caller that got here has broken an
/// arithmetic invariant, and a wrapped-around magnitude would be a silently
/// wrong answer.
pub(super) fn subtract_in_place(minuend: &mut [u64], subtrahend: &[u64]) {
    let mut borrow = 0;

    let length = Ord::min(minuend.len(), subtrahend.len());
    let (minuend_low, minuend_high) = minuend.split_at_mut(length);
    let (subtrahend_low, subtrahend_high) = subtrahend.split_at(length);

    for (digit, &subtrahend_digit) in minuend_low.iter_mut().zip(subtrahend_low) {
        borrow = subtract_with_borrow(borrow, *digit, subtrahend_digit, digit);
    }

    if borrow != 0 {
        for digit in minuend_high {
            borrow = subtract_with_borrow(borrow, *digit, 0, digit);
            if borrow == 0 {
                break;
            }
        }
    }

    assert!(
        borrow == 0 && subtrahend_high.iter().all(|&digit| digit == 0),
        "a magnitude was subtracted from a smaller one"
    );
}

/// Orders two magnitudes numerically. Zero top digits are ignored, so a
/// slice need not be trimmed.
pub(super) fn compare_magnitudes(left: &[u64], right: &[u64]) -> Ordering {
    let left = &left[..significant_length(left)];
    let right = &right[..significant_length(right)];
    match Ord::cmp(&left.len(), &right.len()) {
        Ordering::Equal => Iterator::cmp(left.iter().rev(), right.iter().rev()),
        other => other,
    }
}

/// `left + right`, trimmed.
pub(super) fn add_magnitudes(left: &[u64], right: &[u64]) -> Vec<u64> {
    let (longer, shorter) = if left.len() >= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut sum = Vec::with_capacity(longer.len() + 1);
    sum.extend_from_slice(longer);
    let carry = add_in_place_with_carry(&mut sum, shorter);
    if carry != 0 {
        sum.push(carry);
    }
    trim_zero_top_digits(&mut sum);
    sum
}

/// `larger - smaller`, trimmed; the caller has established the order.
pub(super) fn subtract_magnitudes(larger: &[u64], smaller: &[u64]) -> Vec<u64> {
    let mut difference = larger.to_vec();
    subtract_in_place(&mut difference, smaller);
    trim_zero_top_digits(&mut difference);
    difference
}
