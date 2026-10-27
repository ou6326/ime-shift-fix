//! Physical Shift state, independent of Win32 calls so event sequences are testable.

#[derive(Clone, Copy, Default)]
pub struct ShiftState {
    down: u8,
    armed: u8,
    target: isize,
}

#[derive(Clone, Copy)]
pub enum Side {
    Left = 1,
    Right = 2,
}

impl ShiftState {
    pub fn press(&mut self, side: Side) {
        self.down |= side as u8;
    }

    pub fn click(&mut self, target: isize) {
        if self.down != 0 {
            self.armed = self.down;
            self.target = target;
        }
    }

    pub fn cancel_selection(&mut self) {
        self.armed = 0;
        self.target = 0;
    }

    pub fn release(&mut self, side: Side) -> Option<isize> {
        let bit = side as u8;
        self.down &= !bit;
        let target = (self.armed & bit != 0).then_some(self.target);
        self.armed &= !bit;
        if self.armed == 0 {
            self.target = 0;
        }
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_shift_and_unshifted_click_pass_through() {
        let mut state = ShiftState::default();
        state.click(10);
        state.press(Side::Left);
        assert_eq!(state.release(Side::Left), None);
    }

    #[test]
    fn selection_is_consumed_once_for_either_side() {
        for side in [Side::Left, Side::Right] {
            let mut state = ShiftState::default();
            state.press(side);
            state.press(side); // Keyboard autorepeat is idempotent.
            state.click(10);
            assert_eq!(state.release(side), Some(10));
            assert_eq!(state.release(side), None);
            state.press(side);
            assert_eq!(state.release(side), None);
        }
    }

    #[test]
    fn releasing_one_shift_keeps_the_other_held() {
        let mut state = ShiftState::default();
        state.press(Side::Left);
        state.press(Side::Right);
        assert_eq!(state.release(Side::Left), None);
        state.click(20);
        assert_eq!(state.release(Side::Right), Some(20));
    }

    #[test]
    fn both_shifts_at_click_are_protected() {
        for first in [Side::Left, Side::Right] {
            let second = match first {
                Side::Left => Side::Right,
                Side::Right => Side::Left,
            };
            let mut state = ShiftState::default();
            state.press(first);
            state.press(second);
            state.click(30);
            assert_eq!(state.release(first), Some(30));
            assert_eq!(state.release(second), Some(30));
        }
    }

    #[test]
    fn shift_pressed_after_click_is_not_protected() {
        let mut state = ShiftState::default();
        state.press(Side::Left);
        state.click(10);
        state.press(Side::Right);
        assert_eq!(state.release(Side::Right), None);
        assert_eq!(state.release(Side::Left), Some(10));
    }

    #[test]
    fn wheel_cancels_selection_but_preserves_held_keys() {
        let mut state = ShiftState::default();
        state.press(Side::Left);
        state.click(10);
        state.cancel_selection();
        state.click(20);
        assert_eq!(state.release(Side::Left), Some(20));
        state.press(Side::Right);
        state.click(30);
        state.cancel_selection();
        assert_eq!(state.release(Side::Right), None);
    }
}
