//! PipeWire stores volume linearly (amplitude), but pavucontrol/wpctl display and accept it on a
//! cubic perceptual scale - e.g. a Route `channelVolumes` of `0.404306` is what wpctl shows as
//! 74% (`0.404306.cbrt() ~= 0.7396`). Faders in this app work in the same perceptual units so the
//! displayed percentage matches other PipeWire tools, and so equal-looking fader movements sound
//! equally loud.

/// Linear amplitude (as stored/sent over PipeWire) -> perceptual value (as shown on a fader).
pub fn linear_to_perceptual(linear: f32) -> f32 {
    linear.max(0.0).cbrt()
}

/// Perceptual value (as set on a fader) -> linear amplitude (as sent over PipeWire).
pub fn perceptual_to_linear(perceptual: f32) -> f32 {
    let p = perceptual.max(0.0);
    p * p * p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for p in [0.0_f32, 0.25, 0.5, 0.74, 1.0, 1.5] {
            let linear = perceptual_to_linear(p);
            assert!((linear_to_perceptual(linear) - p).abs() < 1e-5, "p={p} linear={linear}");
        }
    }

    #[test]
    fn matches_observed_wpctl_value() {
        // A real Route channelVolumes value seen via pw-dump, and the percentage wpctl showed
        // for it (74%) - the exact case that motivated this module.
        let displayed = linear_to_perceptual(0.404306);
        assert!((displayed - 0.7396).abs() < 1e-3, "displayed={displayed}");
    }

    #[test]
    fn negative_input_clamps_to_zero() {
        assert_eq!(linear_to_perceptual(-1.0), 0.0);
        assert_eq!(perceptual_to_linear(-1.0), 0.0);
    }
}
