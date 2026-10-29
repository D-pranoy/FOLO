//! Stage 1: fit the projected elements to the target's `address.count` (ADR-0036 D16).

/// Fits `input` to `count` elements: the mean of each group when there are more elements than
/// addresses, repetition when there are fewer, and a copy when the counts agree (which keeps the
/// values bit for bit). No input gives black.
pub(crate) fn fit(input: &[[f32; 3]], count: usize) -> Vec<[f32; 3]> {
    let have = input.len();
    if have == count {
        return input.to_vec();
    }
    if have == 0 {
        return vec![[0.0; 3]; count];
    }
    if have < count {
        return (0..count).map(|j| input[j * have / count]).collect();
    }
    (0..count)
        .map(|j| {
            let group = &input[j * have / count..(j + 1) * have / count];
            let mut sum = [0.0_f32; 3];
            for pixel in group {
                for (s, v) in sum.iter_mut().zip(pixel) {
                    *s += v;
                }
            }
            #[allow(clippy::cast_precision_loss)]
            let n = group.len() as f32;
            sum.map(|s| s / n)
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::fit;

    #[test]
    fn equal_counts_copy_exactly() {
        let input = vec![[0.1, 254.9, 3.3], [255.0, 0.0, 17.5]];
        assert_eq!(fit(&input, 2), input);
    }

    #[test]
    fn more_elements_than_addresses_take_the_mean() {
        let input = vec![
            [10.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [30.0, 60.0, 0.0],
            [40.0, 0.0, 0.0],
        ];
        assert_eq!(fit(&input, 2), vec![[15.0, 0.0, 0.0], [35.0, 30.0, 0.0]]);
        assert_eq!(fit(&input, 1), vec![[25.0, 15.0, 0.0]]);
    }

    #[test]
    fn uneven_groups_cover_every_element_once() {
        let input: Vec<[f32; 3]> = (0..5).map(|i| [i as f32 * 10.0, 0.0, 0.0]).collect();
        // Groups of 1 and 2 elements: [0], [10, 20] over 5 → 0..1, 1..3? j*5/2 = 0, 2, 5.
        assert_eq!(fit(&input, 2), vec![[5.0, 0.0, 0.0], [30.0, 0.0, 0.0]]);
    }

    #[test]
    fn fewer_elements_than_addresses_repeat() {
        let input = vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        let out = fit(&input, 5);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], input[0]);
        assert_eq!(out[1], input[0]);
        assert_eq!(out[2], input[0]);
        assert_eq!(out[3], input[1]);
        assert_eq!(out[4], input[1]);
    }

    #[test]
    fn no_input_is_black() {
        assert_eq!(fit(&[], 3), vec![[0.0; 3]; 3]);
    }
}
