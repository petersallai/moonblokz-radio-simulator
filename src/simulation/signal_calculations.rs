//! Radio signal and timing calculations.
//!
//! Contains helpers for:
//! - Log-distance path loss with optional log-normal shadowing
//! - LoRa-inspired airtime, preamble, and CAD duration estimates
//! - Effective communication distance estimation given a simple link budget
//! - SNR thresholds and RSSI sampling utilities
//!
//! Units:
//! - Power: dBm, mW (conversion provided)
//! - Time: seconds (f32) for mathematical expressions, embassy `Duration` for API
//! - Distance: meters in the physical model; world units are interpreted by the caller

use embassy_time::Duration;
use rand::thread_rng;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;

/// Parameters defining the radio channel propagation model.
///
/// This struct encapsulates the constants used in the log-distance path loss model
/// with log-normal shadowing. These parameters determine how signal strength
/// decays over distance and how much random variation (shadowing) is applied.
#[derive(Deserialize, Clone)]
pub(crate) struct PathLossParameters {
    /// Path loss exponent (n).
    ///
    /// Determines how quickly the signal power decays with distance.
    /// - n = 2.0: Free space (vacuum)
    /// - n = 2.7 to 3.5: Urban areas
    /// - n = 3.0 to 5.0: Indoor obstructed environments
    pub(crate) path_loss_exponent: f32,

    /// Standard deviation for log-normal shadowing (σ) in dB.
    ///
    /// Represents the random variation in path loss due to environmental clutter
    /// (buildings, trees, etc.) that is not captured by the deterministic path loss.
    /// A value of 0.0 disables shadowing.
    pub(crate) shadowing_sigma: f32,

    /// Path loss at the reference distance d₀ (typically 1 meter) in dB.
    ///
    /// This is the baseline loss measured or calculated at a short distance from
    /// the transmitter.
    pub(crate) path_loss_at_reference_distance: f32,

    /// The thermal noise floor of the receiver in dBm.
    ///
    /// Represents the minimum signal power below which the signal is indistinguishable
    /// from background noise (before considering SNR requirements).
    /// Typical values are around -120 dBm to -100 dBm depending on bandwidth.
    pub(crate) noise_floor: f32,
}

#[derive(Deserialize, Clone)]
pub(crate) struct LoraParameters {
    bandwidth: u32,
    spreading_factor: u8,
    coding_rate: u32,
    // Number of preamble symbols (typically 8 for LoRa)
    preamble_symbols: f32,
    // Whether a 16-bit CRC is enabled on the payload
    crc_enabled: bool,
    // Low Data Rate Optimization (DE) flag; typically enabled when T_sym >= 16ms
    low_data_rate_optimization: bool,
}

/// Calculate the path loss (in dB) at a given distance using a log-distance
/// path loss model with log-normal shadowing.
///
/// # Formula
///
/// ```text
/// PL(d) = PL(d₀) + 10 × n × log₁₀(d/d₀) + X_σ
/// where d₀ = 1 meter (reference distance)
/// ```
///
/// Where:
/// - `PL(d₀)`: Path loss at reference distance (1 meter), from `params.path_loss_at_reference_distance`
/// - `n`: Path loss exponent, from `params.path_loss_exponent` (typical values: 2.0 for free space, 2-4 for urban environments)
/// - `d`: Distance in meters
/// - `X_σ`: Log-normal shadowing term sampled from Normal(0, σ) where σ = `params.shadowing_sigma`
///
/// # Parameters
///
/// - `distance`: The distance between transmitter and receiver in meters
/// - `params`: Path loss model parameters including exponent, shadowing sigma, and reference loss
///
/// # Returns
///
/// Path loss in decibels (dB). This is a stochastic value due to the shadowing component.
///
/// # Notes
///
/// - For distances < 1.0 meter, returns the reference path loss without further attenuation
/// - The shadowing term introduces randomness to model environmental variations and multipath effects
/// - Each call samples a new shadowing value, so repeated calls with the same distance yield different results
pub(crate) fn calculate_path_loss(distance: f32, params: &PathLossParameters) -> f32 {
    if distance < 1.0 {
        return params.path_loss_at_reference_distance;
    }
    let path_loss = params.path_loss_at_reference_distance + 10.0 * params.path_loss_exponent * distance.log10();
    // Sample log-normal shadowing as a Normal(0, sigma) in dB
    let shadowing = if params.shadowing_sigma > 0.0 {
        let normal = Normal::new(0.0_f32, params.shadowing_sigma).expect("invalid normal sigma");
        let mut rng = thread_rng();
        normal.sample(&mut rng)
    } else {
        0.0
    };
    path_loss + shadowing
}

// Estimate an "effective" communication distance from a simplified log-distance path loss model.
// Model: PL(d) = PL(d0) + 10 * n * log10(d / d0).
// Solving for distance with a link budget L = P_tx - PL(d0) gives:
//   d = d0 * 10^((P_tx - PL(d0)) / (10 * n))
// Assumptions:
// - d0 = 1 meter (anchor). We take PL(d0) from the provided parameters.
// - Receiver sensitivity, antenna gains, margins, and noise floor are ignored; this yields an upper-bound distance.
// - Shadowing is intentionally not sampled here to keep the estimate stable across calls. The result is a statistical average, not a specific link instance.
/// Estimate a deterministic “effective distance” based on a basic link budget
/// without sampling shadowing. Used for range checks and UI visualization.
pub(crate) fn calculate_effective_distance(tx_power_dbm: f32, lora_parameters: &LoraParameters, path_loss_parameters: &PathLossParameters) -> f32 {
    // Find distance d where received power equals the receiving limit (sensitivity threshold):
    //   P_rx(dBm) = P_tx(dBm) - PL(d) = receiving_limit
    // With PL(d) = PL(d0) + 10 * n * log10(d/d0) and d0 = 1 m, PL(d) = PL0 + 10n*log10(d)
    // Solve for d:
    //   P_tx - (PL0 + 10n log10 d) = RL  =>  10n log10 d = P_tx - RL - PL0  =>
    //   d = 10^((P_tx - RL - PL0) / (10n))
    let pl0 = path_loss_parameters.path_loss_at_reference_distance;
    let rl = calculate_receiving_limit_with_basic_noise(lora_parameters, path_loss_parameters);
    let numerator = tx_power_dbm - rl - pl0;
    let denom = 10.0 * path_loss_parameters.path_loss_exponent;
    if numerator <= 0.0 {
        return 0.0; // or 1.0, depending on desired semantics
    }
    10.0_f32.powf(numerator / denom)
}

pub(crate) fn calculate_receiving_limit_with_basic_noise(lora_parameters: &LoraParameters, path_loss_parameters: &PathLossParameters) -> f32 {
    let snr_limit = calculate_snr_limit(lora_parameters);

    return path_loss_parameters.noise_floor + snr_limit;
}

pub(crate) fn calculate_snr_limit(lora_parameters: &LoraParameters) -> f32 {
    let snr_limit = match lora_parameters.spreading_factor {
        5 => -2.5,
        6 => -5.0,
        7 => -7.5,
        8 => -10.0,
        9 => -12.5,
        10 => -15.0,
        11 => -17.5,
        12 => -20.0,
        _ => -20.0, // Default to the worst case if SF is out of range
    };

    return snr_limit;
}

pub(crate) fn calculate_air_time(lora_parameters: &LoraParameters, payload_size: usize) -> f32 {
    // LoRa symbol time in seconds: T_sym = 2^SF / BW
    let symbol_time = 2.0_f32.powi(lora_parameters.spreading_factor as i32) / lora_parameters.bandwidth as f32;

    // Include preamble time. Default LoRa preamble is typically 8 symbols.
    // Effective preamble duration: (N_preamble + 4.25) * T_sym
    let preamble_time = (lora_parameters.preamble_symbols + 4.25) * symbol_time;

    // Standard LoRa payload symbol calculation (SX127x/LoRa spec)
    // N_payload = 8 + max( ceil((8*PL - 4*SF + 28 + 16*CRC - 20*IH) / (4 * (SF - 2*DE))) * (CR + 4), 0 )
    // Assumptions:
    // - Explicit header mode (IH = 0). If you later support implicit header, add a flag and set IH=1.
    let sf = lora_parameters.spreading_factor as f32;
    let pl = payload_size as f32;
    let crc = if lora_parameters.crc_enabled { 1.0 } else { 0.0 };
    let de = if lora_parameters.low_data_rate_optimization { 1.0 } else { 0.0 };
    let ih = 0.0_f32; // explicit header
    let cr = lora_parameters.coding_rate as f32; // expected 1..4 representing 4/5..4/8

    let denom = 4.0 * (sf - 2.0 * de);
    let numerator = 8.0 * pl - 4.0 * sf + 28.0 + 16.0 * crc - 20.0 * ih;
    let base = (numerator / denom).ceil();
    let payload_symbols = 8.0 + (base * (cr + 4.0)).max(0.0);

    preamble_time + payload_symbols * symbol_time
}

/// Calculate the instantaneous RSSI (in dBm) at a given distance using the
/// existing log-distance path loss model with log-normal shadowing.
///
/// Formula: RSSI(dBm) = P_tx(dBm) - PL(dB)
/// - P_tx(dBm): transmit power at the antenna port
/// - PL(dB): path loss sampled via `calculate_path_loss(distance, params)`
///
/// Note: Because `calculate_path_loss` includes a random shadowing term, the
/// returned RSSI is a stochastic sample. For a deterministic estimate, consider
/// using the reference path loss without shadowing.
pub(crate) fn calculate_rssi(distance: f32, tx_power_dbm: f32, params: &PathLossParameters) -> f32 {
    let path_loss_db = calculate_path_loss(distance, params);
    tx_power_dbm - path_loss_db
}

/// Convert power from dBm (decibels relative to 1 milliwatt) to milliwatts.
///
/// # Formula
///
/// ```text
/// P(mW) = 10^(P(dBm) / 10)
/// ```
///
/// # Parameters
///
/// - `dbm`: Power level in dBm (decibels relative to 1 milliwatt)
///
/// # Returns
///
/// Power in milliwatts (mW)
///
/// # Examples
///
/// ```text
/// 0 dBm   → 1 mW
/// 10 dBm  → 10 mW
/// 20 dBm  → 100 mW
/// -10 dBm → 0.1 mW
/// ```
///
/// # Notes
///
/// This is a standard logarithmic power conversion used in RF engineering.
/// The dBm scale is logarithmic, making it easier to work with the wide
/// dynamic range of radio signal powers.
pub(crate) fn dbm_to_mw(dbm: f32) -> f32 {
    10f32.powf(dbm / 10.0)
}

/// Convert power from milliwatts to dBm (decibels relative to 1 milliwatt).
///
/// # Formula
///
/// ```text
/// P(dBm) = 10 × log₁₀(P(mW))
/// ```
///
/// # Parameters
///
/// - `mw`: Power level in milliwatts (must be positive)
///
/// # Returns
///
/// Power in dBm (decibels relative to 1 milliwatt)
///
/// # Examples
///
/// ```text
/// 1 mW    → 0 dBm
/// 10 mW   → 10 dBm
/// 100 mW  → 20 dBm
/// 0.1 mW  → -10 dBm
/// ```
///
/// # Notes
///
/// - This is the inverse operation of `dbm_to_mw()`
/// - For `mw <= 0`, this function will return NaN or -∞ (undefined behavior)
/// - In practice, power values should always be positive
pub(crate) fn mw_to_dbm(mw: f32) -> f32 {
    10.0 * mw.log10()
}

#[allow(dead_code)]
pub(crate) fn get_preamble_time(lora_parameters: &LoraParameters) -> Duration {
    // Calculate the preamble time based on LoRa parameters
    let symbol_time = 2.0_f32.powi(lora_parameters.spreading_factor as i32) / lora_parameters.bandwidth as f32;
    Duration::from_micros(((lora_parameters.preamble_symbols + 4.25) * symbol_time * 1000000.0) as u64)
}

pub(crate) fn get_cad_time(lora_parameters: &LoraParameters) -> Duration {
    // Typical length of Lora CAD is the time of 2 symbols
    let symbol_time = 2.0_f32.powi(lora_parameters.spreading_factor as i32) / lora_parameters.bandwidth as f32;
    Duration::from_micros((2.0 * symbol_time * 1000000.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params_sf_bw(sf: u8, bw: u32) -> LoraParameters {
        LoraParameters {
            bandwidth: bw,
            spreading_factor: sf,
            coding_rate: 1,
            preamble_symbols: 8.0,
            crc_enabled: true,
            low_data_rate_optimization: false,
        }
    }

    fn default_pathloss() -> PathLossParameters {
        PathLossParameters {
            path_loss_exponent: 2.0,
            shadowing_sigma: 0.0,
            path_loss_at_reference_distance: 40.0,
            noise_floor: -120.0,
        }
    }

    #[test]
    fn preamble_and_cad_times_scale_with_symbol_time() {
        let lp = params_sf_bw(7, 125_000); // T_sym = 2^7/125k ≈ 1.024 ms
        let pre = get_preamble_time(&lp);
        let cad = get_cad_time(&lp);
        // Expected preamble: (8 + 4.25) * 1.024ms ≈ 12.25 * 1.024ms ≈ 12.544 ms
        assert!((pre.as_micros() as i64 - 12_544).abs() <= 300); // allow small rounding
        // CAD should be roughly 2 symbols ≈ 2.048 ms
        assert!((cad.as_micros() as i64 - 2_048).abs() <= 200);
    }

    #[test]
    fn airtime_increases_with_payload_and_sf() {
        let mut lp = params_sf_bw(7, 125_000);
        let t_small = calculate_air_time(&lp, 10);
        let t_big = calculate_air_time(&lp, 100);
        assert!(t_big > t_small);

        lp.spreading_factor = 9;
        let t_sf9 = calculate_air_time(&lp, 10);
        assert!(t_sf9 > t_small);
    }

    #[test]
    fn snr_limits_match_expectations() {
        for (sf, expect) in [(7, -7.5), (8, -10.0), (9, -12.5), (10, -15.0), (11, -17.5), (12, -20.0)] {
            let lp = params_sf_bw(sf, 125_000);
            let lim = calculate_snr_limit(&lp);
            assert!((lim - expect).abs() < 0.51);
        }
    }

    #[test]
    fn effective_distance_monotonic_with_tx_power() {
        let lp = params_sf_bw(7, 125_000);
        let pl = default_pathloss();
        let d_low = calculate_effective_distance(0.0, &lp, &pl);
        let d_mid = calculate_effective_distance(10.0, &lp, &pl);
        let d_high = calculate_effective_distance(20.0, &lp, &pl);
        assert!(d_low < d_mid && d_mid < d_high);
    }

    #[test]
    fn dbm_mw_conversion_roundtrip_reasonable() {
        let vals = [-100.0, -50.0, 0.0, 10.0];
        for v in vals {
            let mw = dbm_to_mw(v);
            let v2 = mw_to_dbm(mw);
            assert!((v - v2).abs() < 1e-5);
        }
    }
}
