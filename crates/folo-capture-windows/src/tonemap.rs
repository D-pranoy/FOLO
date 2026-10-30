//! HDR to SDR (ADR-0015 D6): the reference the tone-mapping shader is tested
//! against. Windows hands an HDR desktop over as scRGB, linear light in
//! BT.709 primaries where 1.0 is 80 nits. The agent averages that in linear
//! light ([`folo_capture_core::color`]'s way), then maps it against the
//! user's SDR white level: the level Windows shows SDR content at becomes
//! white, so a desktop looks as it does in SDR mode, and what is brighter
//! than that is clipped to white with its hue kept.

/// The luminance, in nits, that scRGB's 1.0 stands for.
pub const SCRGB_NITS: f32 = 80.0;

/// Windows' SDR white level when it can't be read, and the least it can be.
pub const DEFAULT_SDR_WHITE_NITS: f32 = 80.0;
const MAX_SDR_WHITE_NITS: f32 = 1000.0;

/// The nits of a `DISPLAYCONFIG_SDR_WHITE_LEVEL.SDRWhiteLevel`: it is 1000 for
/// 80 nits and scales linearly. 0 (the call failed) means the default.
#[must_use]
pub fn sdr_white_nits(raw_level: u32) -> f32 {
    if raw_level == 0 {
        return DEFAULT_SDR_WHITE_NITS;
    }
    // The level is a small number (a few thousand at most), exact in f32.
    #[allow(clippy::cast_precision_loss)]
    let nits = raw_level as f32 / 1000.0 * SCRGB_NITS;
    nits.clamp(DEFAULT_SDR_WHITE_NITS, MAX_SDR_WHITE_NITS)
}

/// What scRGB is multiplied by to make the SDR white level 1.0.
#[must_use]
pub fn scale_for(sdr_white_nits: f32) -> f32 {
    SCRGB_NITS / sdr_white_nits.max(DEFAULT_SDR_WHITE_NITS)
}

/// Maps an averaged scRGB colour to SDR linear light (0 to 1). `scale` is
/// [`scale_for`]'s. Negative components (colours outside BT.709) go to 0. A
/// colour brighter than SDR white is divided by its brightest component
/// instead of clipping each component alone, so a highlight keeps its hue.
#[must_use]
pub fn tone_map(scrgb: [f32; 3], scale: f32) -> [f32; 3] {
    let scaled = scrgb.map(|component| (component * scale).max(0.0));
    let brightest = scaled[0].max(scaled[1]).max(scaled[2]);
    if brightest <= 1.0 {
        return scaled;
    }
    scaled.map(|component| component / brightest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_white_level_reads_as_nits_and_stays_in_range() {
        assert!((sdr_white_nits(1000) - 80.0).abs() < 1e-3);
        assert!((sdr_white_nits(3000) - 240.0).abs() < 1e-3);
        assert!((sdr_white_nits(0) - 80.0).abs() < 1e-3);
        assert!((sdr_white_nits(500) - 80.0).abs() < 1e-3);
        assert!((sdr_white_nits(u32::MAX) - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn sdr_white_maps_to_white_and_darker_colours_scale_by_the_white_level() {
        let scale = scale_for(240.0);
        assert_eq!(tone_map([3.0; 3], scale), [1.0; 3]);
        let dim = tone_map([1.5, 0.75, 0.3], scale);
        assert!(
            (dim[0] - 0.5).abs() < 1e-6 && (dim[1] - 0.25).abs() < 1e-6,
            "{dim:?}"
        );
        assert_eq!(
            tone_map([0.6, 0.3, 0.15], scale_for(80.0)),
            [0.6, 0.3, 0.15]
        );
    }

    #[test]
    fn highlights_never_exceed_one_and_keep_their_hue() {
        let mapped = tone_map([40.0, 20.0, 10.0], 1.0);
        assert_eq!(mapped, [1.0, 0.5, 0.25]);
    }

    #[test]
    fn negative_components_go_to_zero() {
        assert_eq!(tone_map([-0.5, 0.2, -1.0], 1.0), [0.0, 0.2, 0.0]);
    }
}
