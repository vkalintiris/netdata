//! The node profile (`src/daemon/config/netdata-conf-profile.c`): standalone, parent, child or IoT, detected from the
//! hardware and stream.conf unless `[global] profile` names one, and the malloc settings it implies.

use netdata_agent_inicfg::{Config, SECTION_GLOBAL};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

/// The system profiles (`ND_PROFILE`), as bits in the order of their names table.
const PARENT: u32 = 1 << 30;
const STANDALONE: u32 = 1 << 29;
const CHILD: u32 = 1 << 28;
const IOT: u32 = 1 << 27;
const SYSTEM: u32 = STANDALONE | PARENT | CHILD | IOT;
const NAMES: [(&str, u32); 4] = [
    ("standalone", STANDALONE),
    ("parent", PARENT),
    ("child", CHILD),
    ("iot", IOT),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Standalone,
    Parent,
    Child,
    Iot,
}

/// `ND_PROFILE_2buffer(wb, bits, " ")`.
fn to_text(bits: u32) -> String {
    NAMES
        .iter()
        .filter(|(_, bit)| bits & bit == *bit)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `prefer_profile()`: a preferred profile present replaces the other system profiles.
fn prefer(bits: u32, preferred: u32) -> u32 {
    if bits & preferred != 0 {
        (bits & !SYSTEM) | preferred
    } else {
        bits
    }
}

/// `nd_profile_detect_and_configure()`: the default from the CPUs, the RAM (0 when unknown) and stream.conf, then
/// `[global] profile`, normalised to one system profile (written back when that changed its text).
pub fn detect(
    c: &mut Config,
    system_cpus: i64,
    ram_total: u64,
    is_parent: bool,
    is_child: bool,
) -> Profile {
    let default = if system_cpus <= 1 || (ram_total > 0 && ram_total < 1 << 30) {
        IOT
    } else if is_parent {
        PARENT
    } else if is_child {
        CHILD
    } else {
        STANDALONE
    };
    let text = c
        .get(SECTION_GLOBAL, "profile", Some(&to_text(default)))
        .unwrap_or_default();
    let mut bits = 0;
    for word in quoted_strings_splitter(&text, 100, Separators::Whitespace) {
        match NAMES.iter().find(|(name, _)| name.as_bytes() == word) {
            Some((_, bit)) => bits |= bit,
            None => nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot understand netdata.conf [global].profile = {}",
                String::from_utf8_lossy(&word)
            ),
        }
    }
    let started = bits;
    if bits & SYSTEM == 0 {
        bits |= default & SYSTEM;
    }
    for preferred in [PARENT, STANDALONE, CHILD, IOT] {
        bits = prefer(bits, preferred);
    }
    if bits != started {
        let text = to_text(bits);
        c.set(SECTION_GLOBAL, "profile", &text);
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "The netdata.conf setting [global].profile has been overwritten to '{text}'"
        );
    }
    match bits & SYSTEM {
        PARENT => Profile::Parent,
        CHILD => Profile::Child,
        IOT => Profile::Iot,
        _ => Profile::Standalone,
    }
}

/// `nd_profile_setup()` after the profile is known again: the glibc malloc arenas and trim threshold of the profile,
/// clamped to the system CPUs, exported to the plugins and applied to the daemon (`netdata_conf_glibc_malloc_initialize()`).
pub fn setup_malloc(c: &mut Config, profile: Profile, system_cpus: i64) {
    let (arenas, trim) = match profile {
        Profile::Iot => (1, 16 * 1024),
        Profile::Parent => (4, 128 * 1024),
        Profile::Child => (1, 32 * 1024),
        Profile::Standalone => (1, 64 * 1024),
    };
    let arenas = arena_option(c, "glibc malloc arena max for plugins", arenas, system_cpus);
    crate::conf::export("MALLOC_ARENA_MAX", &arenas.to_string());
    // HAVE_C_MALLOPT: glibc only; musl builds have neither the option nor the call.
    #[cfg(target_env = "gnu")]
    {
        let arenas = arena_option(c, "glibc malloc arena max for netdata", arenas, system_cpus);
        netdata_agent_sys::mallopt_arenas(arenas as i32, trim);
    }
    #[cfg(not(target_env = "gnu"))]
    let _ = trim;
}

/// One arena option: `1..=system_cpus`, written back with a notice otherwise.
fn arena_option(c: &mut Config, name: &str, default: i64, system_cpus: i64) -> i64 {
    // a size_t in C: a negative value is above the CPU count
    let wanted = c.get_number(SECTION_GLOBAL, name, default) as u64;
    let cpus = system_cpus as u64;
    if (1..=cpus).contains(&wanted) {
        return wanted as i64;
    }
    let arenas = if wanted < 1 { 1 } else { system_cpus };
    c.set_number(SECTION_GLOBAL, name, arenas);
    nd_log!(
        Source::Daemon,
        Priority::Notice,
        "malloc arenas can be from 1 to {system_cpus}. Setting it to {arenas}"
    );
    arenas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_values_are_a_size_t_as_in_c() {
        let name = "glibc malloc arena max for plugins";
        for (value, want) in [("-3", 4), ("0", 1), ("2", 2), ("9", 4)] {
            let mut c = Config::default();
            c.set(SECTION_GLOBAL, name, value);
            let (got, _) = netdata_agent_log::capture(|| arena_option(&mut c, name, 1, 4));
            assert_eq!(got, want, "{name} = {value}");
        }
    }

    #[test]
    fn profiles_normalise_like_c() {
        struct Case {
            configured: Option<&'static str>,
            system_cpus: i64,
            ram_total: u64,
            is_parent: bool,
            want: Profile,
            text: &'static str,
        }
        let cases = std::collections::BTreeMap::from([
            (
                "parent detected",
                Case {
                    configured: None,
                    system_cpus: 8,
                    ram_total: 8 << 30,
                    is_parent: true,
                    want: Profile::Parent,
                    text: "parent",
                },
            ),
            (
                "small box",
                Case {
                    configured: None,
                    system_cpus: 1,
                    ram_total: 8 << 30,
                    is_parent: true,
                    want: Profile::Iot,
                    text: "iot",
                },
            ),
            (
                "little ram",
                Case {
                    configured: None,
                    system_cpus: 8,
                    ram_total: 512 << 20,
                    is_parent: false,
                    want: Profile::Iot,
                    text: "iot",
                },
            ),
            (
                "unknown ram",
                Case {
                    configured: None,
                    system_cpus: 8,
                    ram_total: 0,
                    is_parent: false,
                    want: Profile::Standalone,
                    text: "standalone",
                },
            ),
            (
                "parent preferred",
                Case {
                    configured: Some("child parent"),
                    system_cpus: 8,
                    ram_total: 8 << 30,
                    is_parent: false,
                    want: Profile::Parent,
                    text: "parent",
                },
            ),
            (
                "unknown words keep the default",
                Case {
                    configured: Some("big"),
                    system_cpus: 8,
                    ram_total: 8 << 30,
                    is_parent: false,
                    want: Profile::Standalone,
                    text: "standalone",
                },
            ),
        ]);
        for (name, case) in cases {
            let mut c = Config::default();
            if let Some(text) = case.configured {
                c.set(SECTION_GLOBAL, "profile", text);
            }
            let got = detect(
                &mut c,
                case.system_cpus,
                case.ram_total,
                case.is_parent,
                false,
            );
            assert_eq!(got, case.want, "{name}");
            let text = c.get(SECTION_GLOBAL, "profile", None).unwrap_or_default();
            assert_eq!(String::from_utf8_lossy(&text), case.text, "{name}");
        }
    }
}
