use ns_memory::{MemoryError, Operation, Protection, Reservation};

fn page() -> usize {
    Reservation::page_size()
}

#[test]
fn a_reservation_is_at_least_as_large_as_asked_and_a_multiple_of_a_page() {
    let reservation = Reservation::reserve(10 * page() + 1).expect("reserve");
    assert!(reservation.size() > 10 * page());
    assert_eq!(reservation.size() % page(), 0);
}

#[test]
fn an_empty_reservation_is_an_invalid_range() {
    assert_eq!(
        Reservation::reserve(0).err(),
        Some(MemoryError::InvalidRange)
    );
}

#[test]
fn an_impossible_reservation_is_refused_as_a_value() {
    let bytes = usize::MAX - page() + 1;
    match Reservation::reserve(bytes) {
        Err(MemoryError::Refused {
            operation: Operation::Reserve,
            ..
        })
        | Err(MemoryError::InvalidRange) => {}
        other => panic!(
            "expected a refusal, got {:?}",
            other.map(|reservation| reservation.size())
        ),
    }
}

/// Windows answers error 87 to a reservation no address space holds, Linux
/// and macOS answer `ENOMEM`; either way it is a refusal as a value.
#[cfg(target_pointer_width = "64")]
#[test]
fn a_reservation_no_address_space_holds_is_refused_by_the_system() {
    assert!(matches!(
        Reservation::reserve(1 << 62),
        Err(MemoryError::Refused {
            operation: Operation::Reserve,
            ..
        })
    ));
}

#[test]
fn commit_decommit_and_protect_accept_page_aligned_ranges_inside_the_reservation() {
    let mut reservation = Reservation::reserve(8 * page()).expect("reserve");
    assert_eq!(reservation.commit(0..2 * page()), Ok(()));
    assert_eq!(
        reservation.protect(page()..2 * page(), Protection::NoAccess),
        Ok(())
    );
    assert_eq!(
        reservation.protect(page()..2 * page(), Protection::ReadWrite),
        Ok(())
    );
    assert_eq!(reservation.decommit(0..2 * page()), Ok(()));
}

#[test]
fn a_misaligned_or_outside_range_is_refused_without_a_system_call() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    let size = reservation.size();
    assert_eq!(
        reservation.commit(1..page()),
        Err(MemoryError::InvalidRange)
    );
    assert_eq!(
        reservation.commit(0..page() + 1),
        Err(MemoryError::InvalidRange)
    );
    assert_eq!(
        reservation.commit(0..size + page()),
        Err(MemoryError::InvalidRange)
    );
    assert_eq!(
        reservation.commit(page()..page()),
        Err(MemoryError::InvalidRange)
    );
    assert_eq!(
        reservation.decommit(size..size + page()),
        Err(MemoryError::InvalidRange)
    );
    let backwards = std::ops::Range {
        start: 2 * page(),
        end: page(),
    };
    assert_eq!(
        reservation.commit(backwards),
        Err(MemoryError::InvalidRange)
    );
}

#[test]
fn protecting_an_uncommitted_range_is_not_committed() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    assert_eq!(
        reservation.protect(0..2 * page(), Protection::NoAccess),
        Err(MemoryError::NotCommitted)
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn every_reservation_is_released_when_it_is_dropped() {
    // 1 TiB a time, 200 TiB in all: more than a 47-bit (128 TiB) address
    // space holds unless each one is given back before the next is made.
    for _ in 0..200 {
        let reservation = Reservation::reserve(1 << 40).expect("reserve");
        drop(reservation);
    }
}

#[test]
fn committed_memory_reads_as_zero_and_keeps_what_is_written() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    let words = reservation.words_mut(0..page()).expect("accessible");
    assert!(words.iter().all(|word| *word == 0));
    words[0] = 0xDEAD_BEEF;
    words[words.len() - 1] = 7;
    let read = reservation.words(0..page()).expect("accessible");
    assert_eq!(read[0], 0xDEAD_BEEF);
    assert_eq!(read[read.len() - 1], 7);
}

#[test]
fn a_slice_covers_only_committed_read_write_memory() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    assert!(reservation.words(0..2 * page()).is_none());
    assert!(reservation.words(page()..2 * page()).is_none());
    assert!(reservation.words(8..16).is_some());
}

#[test]
fn a_misaligned_or_empty_word_range_finds_no_slice() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    assert!(reservation.words(1..9).is_none());
    assert!(reservation.words(0..9).is_none());
    assert!(reservation.words(8..8).is_none());
    let backwards = std::ops::Range { start: 16, end: 8 };
    assert!(reservation.words(backwards).is_none());
}

#[test]
fn a_no_access_range_finds_no_slice_and_keeps_its_contents() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation
        .protect(0..page(), Protection::NoAccess)
        .expect("protect");
    assert!(reservation.words(0..8).is_none());
    reservation
        .protect(0..page(), Protection::ReadWrite)
        .expect("protect");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 42);
}

#[test]
fn committing_a_no_access_range_makes_it_read_write_again() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation
        .protect(0..page(), Protection::NoAccess)
        .expect("protect");
    reservation.commit(0..page()).expect("commit");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 42);
}

#[test]
fn a_word_range_ending_beyond_the_reservation_finds_no_slice() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    let size = reservation.size();
    assert!(reservation.words(0..size + 8).is_none());
    assert!(reservation.words_mut(size..size + 8).is_none());
}

#[test]
fn splitting_no_ranges_lends_no_slices() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    let slices = reservation.split_words_mut::<0>([]).expect("no ranges");
    assert!(slices.is_empty());
}

#[test]
fn decommitted_memory_reads_as_zero_when_committed_again() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation.decommit(0..page()).expect("decommit");
    assert!(reservation.words(0..8).is_none());
    reservation.commit(0..page()).expect("commit");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 0);
}

#[test]
fn committing_again_keeps_the_contents() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation.commit(0..2 * page()).expect("commit");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 42);
}

#[test]
fn disjoint_ranges_are_lent_at_once_and_overlapping_ones_are_not() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..2 * page()).expect("commit");
    let [first, second] = reservation
        .split_words_mut([0..page(), page()..2 * page()])
        .expect("disjoint and accessible");
    first[0] = 1;
    second[0] = 2;
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 1);
    assert_eq!(
        reservation.words(page()..page() + 8).expect("accessible")[0],
        2
    );
    assert!(reservation.split_words_mut([0..16, 8..24]).is_none());
    assert!(
        reservation
            .split_words_mut([0..page(), 2 * page()..3 * page()])
            .is_none()
    );
}
