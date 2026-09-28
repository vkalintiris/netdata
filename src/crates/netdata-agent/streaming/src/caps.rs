//! Capability negotiation, ported from `src/streaming/stream-capabilities.c` and the receiver side of
//! `stream_select_receiver_compression_algorithm()` (`src/streaming/stream-compression/compression.c`). The bits and
//! their names are shared with `ingest` (`netdata_agent_pluginsd_proto::caps`).

pub use netdata_agent_pluginsd_proto::caps::*;
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

const STREAM_OLD_VERSION_CLAIM: u32 = 3;
const STREAM_OLD_VERSION_CLABELS: u32 = 4;
const STREAM_OLD_VERSION_LZ4: u32 = 5;

/// `convert_stream_version_to_capabilities(version, NULL, false)`: the receiver's, against what it offers. The C
/// parameter is an `int32_t`, so callers pass `strtoul()` results truncated to 32 bits.
pub fn from_version(version: i32) -> u32 {
    negotiate(version, ours(GLOBALLY_DISABLED))
}

/// `stream_our_capabilities(host, true)` of a sender: no ML runs here and the host's receiver, if any, did not
/// negotiate ML_MODELS; `disabled` is the sender's own (`sender->disabled_capabilities`).
pub fn sender_ours(disabled: u32) -> u32 {
    ours(GLOBALLY_DISABLED | ML_MODELS | disabled)
}

/// `convert_stream_version_to_capabilities()` given `stream_our_capabilities()`: legacy versions (and negative ones)
/// map to fixed sets, anything above 5 is the bitmap itself, ANDed with `ours`; ML_MODELS needs INTERPOLATED.
pub fn negotiate(version: i32, ours: u32) -> u32 {
    let mut caps = match version {
        i32::MIN..=1 => V1,
        2 => V2 | HLABELS,
        3 => VN | HLABELS | CLAIM,
        4 => VN | HLABELS | CLAIM | CLABELS,
        5 => VN | HLABELS | CLAIM | CLABELS | (LZ4 & COMPRESSIONS_AVAILABLE),
        v => v as u32,
    };
    if caps & VCAPS != 0 {
        caps &= !(V1 | V2 | VN);
    }
    if caps & VN != 0 {
        caps &= !(V1 | V2);
    }
    if caps & V2 != 0 {
        caps &= !V1;
    }
    let mut common = caps & ours;
    if common & INTERPOLATED == 0 {
        common &= !ML_MODELS;
    }
    common
}

/// `stream_capabilities_to_vn()`.
pub fn to_vn(caps: u32) -> u32 {
    if caps & LZ4 != 0 {
        STREAM_OLD_VERSION_LZ4
    } else if caps & CLABELS != 0 {
        STREAM_OLD_VERSION_CLABELS
    } else {
        STREAM_OLD_VERSION_CLAIM
    }
}

/// `COMPRESSION_ALGORITHM_MAX`: the slots of a priority list (none, zstd, lz4, brotli, gzip).
const COMPRESSION_ALGORITHM_MAX: usize = 5;

/// `stream_parse_compression_order()`: the named algorithms that are `available`, in order and without repeats
/// (names are case-insensitive), then every other available one in the default order.
pub fn parse_compression_order(order: &[u8], available: u32) -> Vec<u32> {
    const NAMES: [(&[u8], u32); 4] = [
        (b"zstd", ZSTD),
        (b"lz4", LZ4),
        (b"brotli", BROTLI),
        (b"gzip", GZIP),
    ];
    let mut priorities = Vec::new();
    let words = quoted_strings_splitter(
        order,
        COMPRESSION_ALGORITHM_MAX + 100,
        Separators::Whitespace,
    );
    for word in &words {
        if priorities.len() >= COMPRESSION_ALGORITHM_MAX {
            break;
        }
        if let Some(&(_, cap)) = NAMES.iter().find(|(name, cap)| {
            available & cap != 0 && word.eq_ignore_ascii_case(name) && !priorities.contains(cap)
        }) {
            priorities.push(cap);
        }
    }
    for (_, cap) in NAMES {
        if available & cap != 0
            && priorities.len() < COMPRESSION_ALGORITHM_MAX
            && !priorities.contains(&cap)
        {
            priorities.push(cap);
        }
    }
    priorities
}

/// `stream_select_receiver_compression_algorithm()`: compression off drops the `available` compression bits; when
/// more than one remains, only the first in `priorities` stays.
pub fn select_compression(mut caps: u32, enabled: bool, priorities: &[u32], available: u32) -> u32 {
    if !enabled {
        caps &= !available;
    }
    let compressions = caps & available;
    if compressions.count_ones() > 1 {
        if let Some(&chosen) = priorities
            .iter()
            .find(|&&c| c & available != 0 && compressions & c != 0)
        {
            caps &= !(compressions & !chosen);
        }
    }
    caps
}

/// `START_STREAMING_PROMPT_VN`.
pub const PROMPT_VN: &str = "Hit me baby, push them over with the version=";
/// `START_STREAMING_PROMPT_V2`.
pub const PROMPT_V2: &str = "Hit me baby, push them over and bring the host labels...";
/// `START_STREAMING_PROMPT_V1`.
pub const PROMPT_V1: &str = "Hit me baby, push them over...";

/// The prompt of an accepted connection, chosen by its highest protocol capability.
pub fn prompt(caps: u32) -> String {
    if caps & VCAPS != 0 {
        format!("{PROMPT_VN}{caps}")
    } else if caps & VN != 0 {
        format!("{PROMPT_VN}{}", to_vn(caps))
    } else if caps & V2 != 0 {
        PROMPT_V2.to_string()
    } else {
        PROMPT_V1.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_child_gets_its_offer_back() {
        for offered in [17088, 704, 21184] {
            assert_eq!(
                prompt(from_version(offered)),
                format!("{PROMPT_VN}{offered}")
            );
        }
        assert_eq!(prompt(from_version(-1)), PROMPT_V1);
        assert_eq!(prompt(from_version(0)), PROMPT_V1);
        assert_eq!(prompt(from_version(2)), PROMPT_V2);
        assert_eq!(prompt(from_version(4)), format!("{PROMPT_VN}4"));
        // ML_MODELS needs INTERPOLATED; unknown bits and bit 16 are dropped.
        assert_eq!(
            from_version((VCAPS | ML_MODELS | DATA_WITH_ML | 1 << 29) as i32),
            VCAPS
        );
    }

    /// A child's offer: 469565432 with compression, as a C child's request; without, the harness default.
    #[test]
    fn a_senders_offer_is_c_s() {
        assert_eq!(sender_ours(0), 469_565_432);
        assert_eq!(sender_ours(COMPRESSIONS_AVAILABLE), 465_894_392);
        assert_eq!(negotiate(3, sender_ours(0)), VN | HLABELS | CLAIM);
        // the parent's VCAPS answer drops the legacy version bits
        assert_eq!(negotiate(469_565_432, sender_ours(0)), 469_565_432 & !(V1 | V2 | VN));
    }

    #[test]
    fn compression_selection() {
        // With the C production build's algorithms.
        let all = COMPRESSIONS;
        let order = parse_compression_order(b"GZIP  gzip lz4 bogus", all);
        assert_eq!(order, [GZIP, LZ4, ZSTD, BROTLI]);
        assert_eq!(parse_compression_order(b"", GZIP), [GZIP]);
        assert_eq!(
            select_compression(VCAPS | LZ4 | ZSTD, true, &order, all),
            VCAPS | LZ4
        );
        assert_eq!(
            select_compression(VCAPS | LZ4 | GZIP, false, &order, all),
            VCAPS
        );
        assert_eq!(
            select_compression(VCAPS | BROTLI, true, &order, all),
            VCAPS | BROTLI
        );
    }
}
