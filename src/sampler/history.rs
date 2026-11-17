// SPDX-License-Identifier: MPL-2.0

//! History: the last [`HISTORY_LEN`] samples of a value, kept for the popup's graphs (D2).

use std::collections::VecDeque;

/// Samples kept: 2 minutes at the 2 s interval.
pub const HISTORY_LEN: usize = 60;

/// A ring buffer of the last [`HISTORY_LEN`] values, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    values: VecDeque<f32>,
}

impl History {
    /// Appends `value`, dropping the oldest one when full.
    pub fn push(&mut self, value: f32) {
        if self.values.len() == HISTORY_LEN {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    /// The values, oldest first.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        self.values.iter().copied()
    }

    /// The newest value.
    #[must_use]
    pub fn latest(&self) -> Option<f32> {
        self.values.back().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_values_oldest_first() {
        let mut history = History::default();
        history.push(1.0);
        history.push(2.0);
        assert_eq!(history.iter().collect::<Vec<_>>(), [1.0, 2.0]);
        assert_eq!(history.latest(), Some(2.0));
    }

    #[test]
    fn drops_the_oldest_value_once_full() {
        let mut history = History::default();
        for i in 0..=HISTORY_LEN {
            #[expect(clippy::cast_precision_loss, reason = "small numbers")]
            history.push(i as f32);
        }
        assert_eq!(history.iter().len(), HISTORY_LEN);
        assert_eq!(history.iter().next(), Some(1.0));
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        let newest = HISTORY_LEN as f32;
        assert_eq!(history.latest(), Some(newest));
    }

    #[test]
    fn starts_empty() {
        assert_eq!(History::default().iter().len(), 0);
        assert_eq!(History::default().latest(), None);
    }
}
