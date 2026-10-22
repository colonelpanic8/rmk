#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[derive(Default)]
pub enum Hand {
    #[default]
    Unknown,
    Left,
    Right,
    Bilateral,
}

/// Configuration that's only related to the key's position.
///
/// Now only the hand information is included.
/// In the future more fields can be added here for the future configurator GUI, such as
/// - physical key position and orientation
/// - key size,
/// - key shape,
/// - backlight sequence number, etc.
///
/// IDEA: For Keyboards with low memory, these should be compile time constants to save RAM?
#[derive(Debug)]
pub struct PositionalConfig<const ROW: usize, const COL: usize> {
    pub hand: [[Hand; COL]; ROW],
}

impl<const ROW: usize, const COL: usize> Default for PositionalConfig<ROW, COL> {
    fn default() -> Self {
        Self {
            hand: [[Hand::default(); COL]; ROW],
        }
    }
}

impl<const ROW: usize, const COL: usize> PositionalConfig<ROW, COL> {
    pub fn new(hand: [[Hand; COL]; ROW]) -> Self {
        Self { hand }
    }
}

impl Hand {
    /// Whether two keys sit on the same hand for unilateral-tap decisions.
    ///
    /// Only `Left`/`Left` and `Right`/`Right` qualify. `Unknown` keys and
    /// `Bilateral` keys (which are deliberately exempt from same-hand rules)
    /// always return `false`, even when paired with themselves.
    pub fn is_same_side(self, other: Hand) -> bool {
        matches!((self, other), (Hand::Left, Hand::Left) | (Hand::Right, Hand::Right))
    }

    /// Whether `other` is allowed to activate an opposite-hand-only hold.
    ///
    /// Bilateral keys deliberately activate holds from either hand. Unknown
    /// geometry is not treated as opposite: the policy promises not to emit a
    /// hold unless the compiled layout proves the triggering relationship.
    pub fn triggers_opposite_hand_hold(self, other: Hand) -> bool {
        matches!(
            (self, other),
            (Hand::Left, Hand::Right) | (Hand::Right, Hand::Left) | (Hand::Left | Hand::Right, Hand::Bilateral)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Hand;

    #[test]
    fn opposite_hand_hold_requires_proven_geometry() {
        assert!(Hand::Left.triggers_opposite_hand_hold(Hand::Right));
        assert!(Hand::Right.triggers_opposite_hand_hold(Hand::Left));
        assert!(Hand::Left.triggers_opposite_hand_hold(Hand::Bilateral));
        assert!(Hand::Right.triggers_opposite_hand_hold(Hand::Bilateral));
        assert!(!Hand::Left.triggers_opposite_hand_hold(Hand::Left));
        assert!(!Hand::Right.triggers_opposite_hand_hold(Hand::Right));
        assert!(!Hand::Left.triggers_opposite_hand_hold(Hand::Unknown));
        assert!(!Hand::Unknown.triggers_opposite_hand_hold(Hand::Right));
    }
}
