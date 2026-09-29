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
