use rug::{Float, Rational};
use rug::float::Round;
use rug::ops::MulAssignRound;

use nodeclient::leaderlog::is_overlay_slot;

use super::*;

#[test]
fn test_is_overlay_slot() {
    pretty_env_logger::init_timed();

    let first_slot_of_epoch = 15724800_i64;
    let mut current_slot = 16128499_i64;
    // let d = Float::with_val(120, 0.32);
    let mut d = Float::with_val(24, Float::parse("0.32").unwrap());
    d.mul_assign_round(100, Round::Nearest);
    let r: Rational = Rational::from((d.to_integer().unwrap(), 100));

    assert_eq!(is_overlay_slot(&first_slot_of_epoch, &current_slot, &r), false);

    // AD test
    current_slot = 15920150_i64;
    assert_eq!(is_overlay_slot(&first_slot_of_epoch, &current_slot, &r), true);
}