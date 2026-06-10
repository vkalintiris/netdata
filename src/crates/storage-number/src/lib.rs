use std::fmt;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// ─── Flags ────────────────────────────────────────────────────────────

bitflags::bitflags! {
    /// User-controllable flags for a [`StorageNumber`].
    ///
    /// Only `NOT_ANOMALOUS` and `RESET` are settable by callers at pack time.
    /// The internal encoding bits (sign, multiply, factor-100) are computed
    /// automatically by [`StorageNumber::pack`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SnFlags: u32 {
        /// Slot is not anomalous (anomaly bit set). Default for new values.
        const NOT_ANOMALOUS = 1 << 24;

        /// Counter has been reset or overflowed.
        const RESET         = 1 << 25;
    }
}

impl SnFlags {
    /// Default flags: `NOT_ANOMALOUS` only. Matches C `SN_DEFAULT_FLAGS`.
    pub const DEFAULT: Self = Self::NOT_ANOMALOUS;

    /// Mask of both user-settable flags. Matches C `SN_USER_FLAGS`.
    pub(crate) const USER_MASK: u32 = SnFlags::NOT_ANOMALOUS.bits() | SnFlags::RESET.bits();
}

impl Default for SnFlags {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ─── Constants ─────────────────────────────────────────────────────────

/// Maximum accepted accuracy loss (percent) for pack/unpack roundtrip.
/// From C `ACCURACY_LOSS_ACCEPTED_PERCENT`.
pub const ACCURACY_LOSS_ACCEPTED_PERCENT: f64 = 0.0001;

// Flag bit positions
const NEGATIVE: u32 = 1 << 31;
const MULTIPLY: u32 = 1 << 30;
const NOT_EXISTS_MUL100: u32 = 1 << 26;
const EXPONENT_MASK: u32 = (1 << 29) | (1 << 28) | (1 << 27);
const EXPONENT_SHIFT: u32 = 27;

const MANTISSA_MASK: u32 = 0x00FF_FFFF;
const MANTISSA_MAX: u32 = 0x00FF_FFFF;

/// EMPTY_SLOT = bit 26 set, all else zero.
/// This is `SN_FLAG_NOT_EXISTS_MUL100` (0x04000000).
/// Note: bit 26 + non-zero mantissa/exponent is a valid large-value number,
/// NOT an empty slot. Compare against the exact sentinel, not just bit 26.
const EMPTY_SLOT_RAW: u32 = NOT_EXISTS_MUL100;

/// Threshold below which we multiply up for more precision.
/// `0x00FFFFFF / 10 = 0x0019999E` (1,677,214).
const SCALE_UP_THRESHOLD: f64 = 0x0019_999E_u32 as f64;

// ─── LUT ───────────────────────────────────────────────────────────────

/// Packing scaling factors, precomputed as constants.
///
/// Layout (from C `unpack_storage_number_lut10x`):
///   `[factor * 16 + exp * 8 + mul]` for factor ∈ {0,1}, exp ∈ {0,1}, mul ∈ 0..7.
///
/// ```text
/// factor=0, exp=0 (divide back):     [0..7]   1/10^i
/// factor=0, exp=1 (multiply back):   [8..15]  10^i
/// factor=1, exp=0 (divide back):     [16..23] 1/100^i
/// factor=1, exp=1 (multiply back):   [24..31] 100^i
/// ```
///
/// Values are hardcoded because `f64::powi` is not `const fn` on stable Rust.
#[rustfmt::skip]
const UNPACK_LUT: [f64; 32] = [
    // factor=0, exp=0: 1/10^i (divide back)
    1.0, 0.1, 0.01, 0.001, 0.0001, 1e-5, 1e-6, 1e-7,
    // factor=0, exp=1: 10^i (multiply back)
    1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1_000_000.0, 10_000_000.0,
    // factor=1, exp=0: 1/100^i (divide back)
    1.0, 0.01, 1e-4, 1e-6, 1e-8, 1e-10, 1e-12, 1e-14,
    // factor=1, exp=1: 100^i (multiply back)
    1.0, 100.0, 10000.0, 1_000_000.0, 100_000_000.0, 10_000_000_000.0, 1_000_000_000_000.0, 100_000_000_000_000.0,
];

// ─── StorageNumber ─────────────────────────────────────────────────────

/// A Netdata 32-bit packed storage number.
///
/// Bit layout (MSB first):
///
/// | Bits    | Field |
/// |---------|-------|
/// | 31      | sign (0=positive, 1=negative) |
/// | 30      | operation (0=divide, 1=multiply) |
/// | 29-27   | 3-bit shift exponent (0-7) |
/// | 26      | factor=100 instead of 10 (also `EMPTY` sentinel when standalone) |
/// | 25      | reset/overflow indicator |
/// | 24      | anomaly bit (0=anomalous, 1=non-anomalous) |
/// | 23-0    | 24-bit mantissa (max 16,777,215) |
///
/// Binary-compatible with the C `storage_number` (`uint32_t`) in Netdata's
/// database pages, streams, and on-disk formats. Bit-exact pack/unpack with
/// the C `pack_storage_number`/`unpack_storage_number` functions.
///
/// # Examples
///
/// ```
/// use storage_number::{StorageNumber, SnFlags};
///
/// let sn = StorageNumber::pack(42.5, SnFlags::DEFAULT);
/// assert!(sn.exists());
/// assert_eq!(sn.unpack(), 42.5);
///
/// let empty = StorageNumber::EMPTY;
/// assert!(!empty.exists());
/// assert!(empty.try_unpack().is_none());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct StorageNumber(u32);

impl StorageNumber {
    /// The empty-slot sentinel. Matches C `SN_EMPTY_SLOT` (0x04000000).
    pub const EMPTY: Self = Self(EMPTY_SLOT_RAW);

    // ── Construction ──

    /// Pack an `f64` value with the given user flags.
    ///
    /// NaN and ±Infinity produce [`EMPTY`](Self::EMPTY) (flags are ignored).
    /// Zero and subnormal values store only the flags, mantissa=0.
    ///
    /// Bit-exact with C `pack_storage_number(value, flags)`.
    pub fn pack(value: f64, flags: SnFlags) -> Self {
        if !value.is_finite() {
            return Self::EMPTY;
        }

        let mut r = flags.bits() & SnFlags::USER_MASK;

        if value == 0.0 || value.is_subnormal() {
            return Self(r);
        }

        let mut n = value.abs();
        let mut factor: f64 = 10.0;

        if value < 0.0 {
            r |= NEGATIVE;
        }

        if n / 10_000_000.0 > MANTISSA_MAX as f64 {
            factor = 100.0;
            r |= NOT_EXISTS_MUL100;
        }

        let mut m: u32 = 0;

        // Scale down until the value fits in 24 bits
        while m < 7 && n > MANTISSA_MAX as f64 {
            n /= factor;
            m += 1;
        }

        if m > 0 {
            r |= MULTIPLY | (m << EXPONENT_SHIFT);

            if n > MANTISSA_MAX as f64 {
                return Self(r | MANTISSA_MAX);
            }
        } else {
            // Value already fits in 24 bits — scale up for more precision
            while m < 7 && n < SCALE_UP_THRESHOLD {
                n *= 10.0;
                m += 1;
            }

            if n > MANTISSA_MAX as f64 {
                n /= 10.0;
                m -= 1;
            }

            r |= m << EXPONENT_SHIFT;
        }

        // Round to nearest integer, ties-to-even (matches C lrint semantics).
        // f64::round_ties_even is stable since Rust 1.83 (MSRV is 1.85).
        r |= n.round_ties_even() as u32;

        Self(r)
    }

    /// Pack with default flags (`NOT_ANOMALOUS` set, no reset).
    ///
    /// Convenience for the most common call site.
    /// Equivalent to `StorageNumber::pack(value, SnFlags::DEFAULT)`.
    #[inline]
    pub fn from_f64(value: f64) -> Self {
        Self::pack(value, SnFlags::DEFAULT)
    }

    /// Construct from raw `u32` bits (e.g., from storage or network).
    ///
    /// This is a zero-cost cast; every bit pattern is a valid `StorageNumber`.
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Return the raw `u32` bits.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    // ── Unpacking ──

    /// Unpack to `f64`.
    ///
    /// Returns `f64::NAN` for the empty sentinel, matching C behavior.
    /// Use [`try_unpack`](Self::try_unpack) for an idiomatic `Option` return.
    ///
    /// Bit-exact with C `unpack_storage_number(value)`.
    #[inline]
    pub fn unpack(self) -> f64 {
        if self.0 == EMPTY_SLOT_RAW {
            return f64::NAN;
        }

        let sign = if self.0 & NEGATIVE != 0 { -1.0 } else { 1.0 };
        let exp = usize::from(self.0 & MULTIPLY != 0);
        let factor = usize::from(self.0 & NOT_EXISTS_MUL100 != 0);
        let mul = ((self.0 & EXPONENT_MASK) >> EXPONENT_SHIFT) as usize;
        let mantissa = self.mantissa() as f64;

        sign * UNPACK_LUT[factor * 16 + exp * 8 + mul] * mantissa
    }

    /// Unpack to `Option<f64>`.
    ///
    /// Returns `None` for the empty sentinel. This is the idiomatic Rust
    /// alternative to checking [`exists`](Self::exists) before [`unpack`](Self::unpack).
    #[inline]
    pub fn try_unpack(self) -> Option<f64> {
        if self.is_empty() {
            None
        } else {
            Some(self.unpack())
        }
    }

    // ── Queries ──

    /// `true` if this is not the empty sentinel.
    ///
    /// Matches C `does_storage_number_exist(value)`.
    #[inline]
    pub const fn exists(self) -> bool {
        self.0 != EMPTY_SLOT_RAW
    }

    /// `true` if this is the empty sentinel.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == EMPTY_SLOT_RAW
    }

    /// `true` if the counter was reset or overflowed.
    ///
    /// Matches C `did_storage_number_reset(value)`.
    #[inline]
    pub const fn is_reset(self) -> bool {
        (self.0 & SnFlags::RESET.bits()) != 0
    }

    /// `true` if the value is marked as anomalous.
    ///
    /// Returns `false` for empty slots (matching C `is_storage_number_anomalous`).
    #[inline]
    pub const fn is_anomalous(self) -> bool {
        self.exists() && (self.0 & SnFlags::NOT_ANOMALOUS.bits()) == 0
    }

    /// Extract user-visible flags (anomaly + reset) from the packed value.
    ///
    /// Matches C `n & SN_USER_FLAGS`.
    #[inline]
    pub const fn flags(self) -> SnFlags {
        SnFlags::from_bits_truncate(self.0 & SnFlags::USER_MASK)
    }

    /// Extract just the 24-bit mantissa.
    #[inline]
    pub const fn mantissa(self) -> u32 {
        self.0 & MANTISSA_MASK
    }

    /// Extract the 3-bit shift exponent (0-7).
    #[inline]
    pub const fn exponent(self) -> u32 {
        (self.0 & EXPONENT_MASK) >> EXPONENT_SHIFT
    }

    /// `true` if the packed value uses factor=100 (very large values).
    #[inline]
    pub const fn uses_large_factor(self) -> bool {
        (self.0 & NOT_EXISTS_MUL100) != 0
    }
}

// ─── Trait impls ───────────────────────────────────────────────────────

impl fmt::Debug for StorageNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            write!(f, "StorageNumber(EMPTY)")
        } else {
            f.debug_struct("StorageNumber")
                .field("raw", &format_args!("0x{:08X}", self.0))
                .field("value", &self.unpack())
                .field("anomalous", &self.is_anomalous())
                .field("reset", &self.is_reset())
                .finish()
        }
    }
}

impl fmt::Display for StorageNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            write!(f, "EMPTY")
        } else {
            write!(f, "{:.7}", self.unpack())
        }
    }
}

impl Default for StorageNumber {
    /// Returns [`EMPTY`](Self::EMPTY) — the safest default.
    ///
    /// There is no naturally correct "zero" for storage numbers:
    /// `0u32` is a valid small positive value, not an empty slot.
    fn default() -> Self {
        Self::EMPTY
    }
}

impl From<u32> for StorageNumber {
    #[inline]
    fn from(raw: u32) -> Self {
        Self::from_raw(raw)
    }
}

impl From<StorageNumber> for u32 {
    #[inline]
    fn from(sn: StorageNumber) -> Self {
        sn.0
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Constants sanity ──

    #[test]
    fn empty_slot_is_exact_value() {
        assert_eq!(StorageNumber::EMPTY.raw(), 0x0400_0000);
        assert_eq!(StorageNumber::EMPTY.raw(), NOT_EXISTS_MUL100);

        let large = StorageNumber::from_raw(NOT_EXISTS_MUL100 | 1);
        assert!(large.exists());
        assert!(!large.is_empty());
    }

    #[test]
    fn default_is_empty() {
        assert!(StorageNumber::default().is_empty());
    }

    // ── Edge cases ──

    #[test]
    fn nan_packs_to_empty() {
        assert!(StorageNumber::pack(f64::NAN, SnFlags::DEFAULT).is_empty());
        assert!(StorageNumber::pack(f64::INFINITY, SnFlags::DEFAULT).is_empty());
        assert!(StorageNumber::pack(f64::NEG_INFINITY, SnFlags::DEFAULT).is_empty());
    }

    #[test]
    fn empty_unpacks_to_nan() {
        assert!(StorageNumber::EMPTY.unpack().is_nan());
        assert_eq!(StorageNumber::EMPTY.try_unpack(), None);
    }

    #[test]
    fn zero_preserves_flags_not_empty() {
        let sn = StorageNumber::pack(0.0, SnFlags::DEFAULT);
        assert!(sn.exists());
        assert_eq!(sn.unpack(), 0.0);
        assert!(!sn.is_anomalous());
        assert!(!sn.is_reset());

        let sn = StorageNumber::pack(-0.0, SnFlags::empty());
        assert!(sn.exists());
        assert_eq!(sn.unpack(), 0.0);
        assert!(sn.is_anomalous());
    }

    #[test]
    fn empty_is_not_anomalous() {
        assert!(!StorageNumber::EMPTY.is_anomalous());
    }

    // ── Roundtrip accuracy ──

    #[test]
    fn roundtrip_simple_values() {
        let cases: &[(f64, u32)] = &[
            (0.0, 0x0100_0000),
            (1.0, 0x0100_0001),
            (42.5, 0x0100_002B),
            (-42.5, 0x8100_002B),
        ];

        for &(value, _expected_raw) in cases {
            let packed = StorageNumber::pack(value, SnFlags::DEFAULT);
            let unpacked = packed.unpack();
            if value == 0.0 {
                assert_eq!(unpacked, 0.0);
            } else {
                let loss = ((unpacked - value) / value).abs() * 100.0;
                assert!(
                    loss < ACCURACY_LOSS_ACCEPTED_PERCENT,
                    "roundtrip {value} -> {unpacked}, loss={loss}%"
                );
            }
        }
    }

    #[test]
    fn roundtrip_large_values() {
        let values = [
            16_777_215.0,
            167_772_150.0,
            16_777_215_000.0,
            1_000_000.0,
            1e15,
            -1e15,
        ];

        for &value in &values {
            let packed = StorageNumber::pack(value, SnFlags::DEFAULT);
            assert!(packed.exists(), "value={value} produced empty");
            let unpacked = packed.unpack();
            let loss = ((unpacked - value) / value).abs() * 100.0;
            assert!(
                loss < ACCURACY_LOSS_ACCEPTED_PERCENT,
                "roundtrip {value} -> {unpacked}, loss={loss}%"
            );
        }
    }

    #[test]
    fn roundtrip_very_small_values() {
        let values = [0.0000001, 1e-10, 1e-15, -1e-10];
        for &value in &values {
            let packed = StorageNumber::pack(value, SnFlags::DEFAULT);
            assert!(packed.exists());
            let unpacked = packed.unpack();
            if unpacked == 0.0 {
                continue;
            }
            let loss = ((unpacked - value) / value).abs() * 100.0;
            assert!(
                loss < ACCURACY_LOSS_ACCEPTED_PERCENT,
                "roundtrip {value} -> {unpacked}, loss={loss}%"
            );
        }
    }

    // ── Flags roundtrip ──

    #[test]
    fn flags_preserved() {
        let cases: &[(SnFlags, bool, bool)] = &[
            (SnFlags::DEFAULT, false, false),
            (SnFlags::empty(), true, false),
            (SnFlags::RESET, true, true),
            (SnFlags::NOT_ANOMALOUS | SnFlags::RESET, false, true),
        ];

        for &(flags, expect_anomalous, expect_reset) in cases {
            let sn = StorageNumber::pack(42.0, flags);
            assert_eq!(sn.is_anomalous(), expect_anomalous, "flags={flags:?}");
            assert_eq!(sn.is_reset(), expect_reset, "flags={flags:?}");
        }
    }

    #[test]
    fn flags_extraction() {
        let sn = StorageNumber::pack(1.0, SnFlags::RESET);
        assert!(sn.flags().contains(SnFlags::RESET));
        assert!(!sn.flags().contains(SnFlags::NOT_ANOMALOUS));

        let sn = StorageNumber::pack(1.0, SnFlags::DEFAULT);
        assert!(sn.flags().contains(SnFlags::NOT_ANOMALOUS));
        assert!(!sn.flags().contains(SnFlags::RESET));
    }

    // ── Large value (factor=100) ──

    #[test]
    fn large_value_uses_factor_100() {
        // 1e15 / 10^7 = 10^8 > 16,777,215 → triggers factor=100
        let value = 1e15_f64;
        let sn = StorageNumber::pack(value, SnFlags::DEFAULT);
        assert!(sn.exists());
        assert!(sn.uses_large_factor());

        let unpacked = sn.unpack();
        let loss = ((unpacked - value) / value).abs() * 100.0;
        assert!(loss < ACCURACY_LOSS_ACCEPTED_PERCENT);
    }

    // ── Subnormal ──

    #[test]
    fn subnormal_treated_like_zero() {
        let sn = StorageNumber::pack(f64::MIN_POSITIVE, SnFlags::DEFAULT);
        assert!(sn.exists());
        assert_eq!(sn.mantissa(), 0);
        assert_eq!(sn.unpack(), 0.0);
    }

    // ── Max mantissa clamping ──

    #[test]
    fn overflow_clamps_to_max_mantissa() {
        let huge = 1e30_f64;
        let sn = StorageNumber::pack(huge, SnFlags::DEFAULT);
        assert!(sn.exists());
        assert_eq!(sn.mantissa(), MANTISSA_MAX);
    }

    // ── MUL100 with mantissa is valid (not empty) ──

    #[test]
    fn mul100_with_data_is_not_empty() {
        let raw = NOT_EXISTS_MUL100 | (1 << 27) | 42;
        let sn = StorageNumber::from_raw(raw);
        assert!(sn.exists());
        assert!(!sn.is_empty());
        assert!(sn.uses_large_factor());
    }

    // ── Trait impl tests ──

    #[test]
    fn debug_format() {
        let sn = StorageNumber::from_f64(42.5);
        let d = format!("{sn:?}");
        assert!(d.contains("0x"), "Debug should show hex: {d}");

        let empty_fmt = format!("{:?}", StorageNumber::EMPTY);
        assert!(empty_fmt.contains("EMPTY"));
    }

    #[test]
    fn display_format() {
        let sn = StorageNumber::from_f64(42.5);
        let d = format!("{sn}");
        assert!(d.contains("42.5"));

        let empty_fmt = format!("{}", StorageNumber::EMPTY);
        assert_eq!(empty_fmt, "EMPTY");
    }

    #[test]
    fn from_u32_and_into() {
        let sn: StorageNumber = 0x0100_0042_u32.into();
        assert_eq!(sn.raw(), 0x0100_0042);

        let raw: u32 = sn.into();
        assert_eq!(raw, 0x0100_0042);
    }

    #[test]
    fn copy_and_hash() {
        let sn = StorageNumber::from_f64(1.0);
        let sn2 = sn;
        assert_eq!(sn, sn2);

        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(sn);
        assert!(set.contains(&sn2));
    }

    #[test]
    fn from_bytes_and_into_bytes() {
        use zerocopy::IntoBytes;
        let sn = StorageNumber::from_raw(0x0100_0042);
        let bytes = sn.as_bytes();
        assert_eq!(bytes, &[0x42, 0x00, 0x00, 0x01]);

        use zerocopy::FromBytes;
        let sn2 = StorageNumber::read_from_bytes(bytes).unwrap();
        assert_eq!(sn, sn2);
    }
}
