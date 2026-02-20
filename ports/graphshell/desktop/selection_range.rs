/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::ops::RangeInclusive;

pub(crate) fn inclusive_index_range(
    anchor_index: usize,
    target_index: usize,
    len: usize,
) -> Option<RangeInclusive<usize>> {
    if len == 0 || anchor_index >= len || target_index >= len {
        return None;
    }
    let (start, end) = if anchor_index <= target_index {
        (anchor_index, target_index)
    } else {
        (target_index, anchor_index)
    };
    Some(start..=end)
}

#[cfg(test)]
mod tests {
    use super::inclusive_index_range;

    #[test]
    fn test_inclusive_index_range_forward() {
        let range = inclusive_index_range(1, 4, 6).unwrap();
        assert_eq!(range.collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_inclusive_index_range_backward() {
        let range = inclusive_index_range(4, 1, 6).unwrap();
        assert_eq!(range.collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_inclusive_index_range_rejects_out_of_bounds() {
        assert!(inclusive_index_range(1, 6, 6).is_none());
        assert!(inclusive_index_range(6, 1, 6).is_none());
    }
}
