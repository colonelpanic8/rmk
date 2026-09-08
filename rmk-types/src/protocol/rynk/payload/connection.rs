//! Connection-management endpoint types.

pub use crate::ble::{BLE_NAME_MAX_LEN, BleName};

#[cfg(test)]
mod tests {
    use heapless::String;

    use super::*;
    use crate::protocol::rynk::tests::{assert_max_size_bound, round_trip};

    #[test]
    fn round_trip_ble_name() {
        let name = BleName {
            template: String::try_from("Glove80 {slot}").unwrap(),
        };
        round_trip(&name);
        assert_max_size_bound(&name);

        let max = BleName {
            template: String::try_from("1234567890abcdef").unwrap(),
        };
        round_trip(&max);
        assert_max_size_bound(&max);
    }
}
