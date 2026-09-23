//! `storage_number`, the 32-bit sample encoding of tier 0, ported from `src/libnetdata/storage_number/`.
//!
//! Layout: bit 31 negative, bit 30 multiply (else divide), bits 27-29 the power `m` (0-7), bit 26 factor 100 (else
//! 10), bit 25 reset, bit 24 not-anomalous, bits 0-23 the mantissa. `0x04000000` (factor 100, nothing else) marks an
//! empty slot.

/// `SN_FLAG_NOT_ANOMALOUS`: set when the sample is *not* anomalous.
pub const SN_FLAG_NOT_ANOMALOUS: u32 = 1 << 24;
/// `SN_FLAG_RESET`: the counter wrapped or reset at this sample.
pub const SN_FLAG_RESET: u32 = 1 << 25;
/// `SN_FLAG_NOT_EXISTS_MUL100`.
pub const SN_FLAG_NOT_EXISTS_MUL100: u32 = 1 << 26;
/// `SN_FLAG_MULTIPLY`.
pub const SN_FLAG_MULTIPLY: u32 = 1 << 30;
/// `SN_FLAG_NEGATIVE`.
pub const SN_FLAG_NEGATIVE: u32 = 1 << 31;
/// `SN_USER_FLAGS`: the flags callers pass to [`pack`].
pub const SN_USER_FLAGS: u32 = SN_FLAG_NOT_ANOMALOUS | SN_FLAG_RESET;
/// `SN_DEFAULT_FLAGS`.
pub const SN_DEFAULT_FLAGS: u32 = SN_FLAG_NOT_ANOMALOUS;
/// `SN_EMPTY_SLOT`.
pub const SN_EMPTY_SLOT: u32 = SN_FLAG_NOT_EXISTS_MUL100;

const MANTISSA_MAX: f64 = 0x00ff_ffff as f64;

/// `pack_storage_number()`.
pub fn pack(value: f64, flags: u32) -> u32 {
    if !value.is_finite() {
        return SN_EMPTY_SLOT;
    }
    let mut r: u32 = flags & SN_USER_FLAGS;
    if value == 0.0 || value.is_subnormal() {
        return r;
    }
    let mut m: i32 = 0;
    let mut n = value;
    let mut factor = 10.0;
    if n < 0.0 {
        r = r.wrapping_add(SN_FLAG_NEGATIVE);
        n = -n;
    }
    if n / 10_000_000.0 > MANTISSA_MAX {
        factor = 100.0;
        r |= SN_FLAG_NOT_EXISTS_MUL100;
    }
    while m < 7 && n > MANTISSA_MAX {
        n /= factor;
        m += 1;
    }
    if m != 0 {
        r = r
            .wrapping_add(SN_FLAG_MULTIPLY)
            .wrapping_add((m as u32) << 27);
        if n > MANTISSA_MAX {
            return r.wrapping_add(0x00ff_ffff);
        }
    } else {
        while m < 7 && n < 0x0019_999e as f64 {
            n *= 10.0;
            m += 1;
        }
        if n > MANTISSA_MAX {
            n /= 10.0;
            m -= 1;
        }
        r = r.wrapping_add((m as u32) << 27);
    }
    // lrint(): round half to even in the default rounding mode.
    r.wrapping_add(n.round_ties_even() as i64 as u32)
}

/// `unpack_storage_number_lut10x[]`: 1/10^i, 10^i, 1/100^i, 100^i for i in 0..8.
fn lut(index: usize) -> f64 {
    const POW10: [f64; 8] = [1.0, 10.0, 100.0, 1e3, 1e4, 1e5, 1e6, 1e7];
    const POW100: [f64; 8] = [1.0, 100.0, 1e4, 1e6, 1e8, 1e10, 1e12, 1e14];
    let i = index % 8;
    match index / 8 {
        0 => 1.0 / POW10[i],
        1 => POW10[i],
        2 => 1.0 / POW100[i],
        _ => POW100[i],
    }
}

/// `unpack_storage_number()`: NaN for an empty slot.
pub fn unpack(value: u32) -> f64 {
    if value == SN_EMPTY_SLOT {
        return f64::NAN;
    }
    let sign = if value & SN_FLAG_NEGATIVE != 0 {
        -1.0
    } else {
        1.0
    };
    let exp = usize::from(value & SN_FLAG_MULTIPLY != 0);
    let factor = usize::from(value & SN_FLAG_NOT_EXISTS_MUL100 != 0);
    let mul = ((value & ((1 << 29) | (1 << 28) | (1 << 27))) >> 27) as usize;
    let n = f64::from(value & 0x00ff_ffff);
    sign * lut(factor * 16 + exp * 8 + mul) * n
}

/// `does_storage_number_exist()`.
pub fn exists(value: u32) -> bool {
    value != SN_EMPTY_SLOT
}

/// `did_storage_number_reset()`.
pub fn did_reset(value: u32) -> bool {
    value & SN_FLAG_RESET != 0
}

/// `is_storage_number_anomalous()`.
pub fn is_anomalous(value: u32) -> bool {
    exists(value) && value & SN_FLAG_NOT_ANOMALOUS == 0
}
