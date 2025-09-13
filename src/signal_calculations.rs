use embassy_time::Duration;
use rand::thread_rng;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub(crate) struct PathLossParameters {
    pub(crate) path_loss_exponent: f32,
    pub(crate) shadowing_sigma: f32,
    pub(crate) path_loss_at_reference_distance: f32,
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

pub(crate) fn calculate_path_loss(distance: f32, params: &PathLossParameters) -> f32 {
    if distance < 1.0 {
        return params.path_loss_at_reference_distance;
    }
    let path_loss = params.path_loss_at_reference_distance + 10.0 * params.path_loss_exponent * distance.log10();
    // Sample log-normal shadowing as a Normal(0, sigma) in dB
    let normal = Normal::new(0.0_f32, params.shadowing_sigma).expect("invalid normal sigma");
    let mut rng = thread_rng();
    let shadowing: f32 = normal.sample(&mut rng);
    path_loss + shadowing
}

// Estimate an "effective" communication distance from a simplified log-distance path loss model.
// Model: PL(d) = PL(d0) + 10 * n * log10(d / d0).
// Solving for distance with a link budget L = P_tx - PL(d0) gives:
//   d = d0 * 10^((P_tx - PL(d0)) / (10 * n))
// Assumptions:
// - d0 = 1 meter (anchor). We take PL(d0) from the provided parameters.
// - Receiver sensitivity, antenna gains, margins, and noise floor are ignored; this yields an upper-bound distance.
// - Shadowing is intentionally not sampled here to keep the estimate stable across calls.
pub(crate) fn calculate_effective_distance(tx_power_dbm: f32, lora_parameters: &LoraParameters, path_loss_parameters: &PathLossParameters) -> f32 {
    // Find distance d where received power equals the receiving limit (sensitivity threshold):
    //   P_rx(dBm) = P_tx(dBm) - PL(d) = receiving_limit
    // With PL(d) = PL(d0) + 10 * n * log10(d/d0) and d0 = 1 m, PL(d) = PL0 + 10*n*log10(d)
    // Solve for d:
    //   P_tx - (PL0 + 10n log10 d) = RL  =>  10n log10 d = P_tx - RL - PL0  =>
    //   d = 10^((P_tx - RL - PL0) / (10n))
    let pl0 = path_loss_parameters.path_loss_at_reference_distance;
    let rl = calculate_receiving_limit_with_basic_noise(lora_parameters, path_loss_parameters);
    let numerator = tx_power_dbm as f32 - rl - pl0;
    let denom = 10.0 * path_loss_parameters.path_loss_exponent;
    10.0_f32.powf(numerator / denom)
}

pub(crate) fn calculate_receiving_limit_with_basic_noise(lora_parameters: &LoraParameters, path_loss_parameters: &PathLossParameters) -> f32 {
    let snr_limit = calculate_snr_limit(lora_parameters);

    return path_loss_parameters.noise_floor + snr_limit;
}

pub(crate) fn calculate_snr_limit(lora_parameters: &LoraParameters) -> f32 {
    let snr_limit = match lora_parameters.spreading_factor {
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

pub(crate) fn calculate_air_time(lora_parameters: LoraParameters, payload_size: usize) -> f32 {
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
    let payload_symbols = 8.0 + (base.max(0.0)) * (cr + 4.0);

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

pub(crate) fn dbm_to_mw(dbm: f32) -> f32 {
    10f32.powf(dbm / 10.0)
}
pub(crate) fn mw_to_dbm(mw: f32) -> f32 {
    10.0 * mw.log10()
}

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
