//! Command-line parsing and help, ported from `src/daemon/main.c` (`option_definitions[]`, `help()`) and the GNU
//! `getopt()` behaviour the C daemon relies on.

/// `option_definitions[]`: option letter, description, argument name, default text.
const OPTIONS: &[(char, &str, Option<&str>, Option<&str>)] = &[
    ('c', "Configuration file to load.", Some("filename"), None), // default is CONFIG_DIR/netdata.conf
    (
        'D',
        "Do not fork. Run in the foreground.",
        None,
        Some("run in the background"),
    ),
    (
        'd',
        "Fork. Run in the background.",
        None,
        Some("run in the background"),
    ),
    ('h', "Display this help message.", None, None),
    (
        'P',
        "File to save a pid while running.",
        Some("filename"),
        Some("do not save pid to a file"),
    ),
    (
        'i',
        "The IP address to listen to.",
        Some("IP"),
        Some("all IP addresses IPv4 and IPv6"),
    ),
    ('p', "API/Web port to use.", Some("port"), Some("19999")),
    (
        's',
        "Prefix for /proc and /sys (for containers).",
        Some("path"),
        Some("no prefix"),
    ),
    (
        't',
        "The internal clock of netdata.",
        Some("seconds"),
        Some("1"),
    ),
    ('u', "Run as user.", Some("username"), Some("netdata")),
    ('v', "Print netdata version and exit.", None, None),
    ('V', "Print netdata version and exit.", None, None),
    ('W', "See Advanced options below.", Some("options"), None),
];

const BANNER: &str = "\n ^\n |.-.   .-.   .-.   .-.   .  Netdata                                         \n |   '-'   '-'   '-'   '-'   X-Ray Vision for your infrastructure!           \n +----+-----+-----+-----+-----+-----+-----+-----+-----+-----+-----+-----+--->\n\n Copyright 2018-2025 Netdata Inc.\n Released under GNU General Public License v3 or later.\n\n Home Page  : https://netdata.cloud\n Source Code: https://github.com/netdata/netdata\n Docs       : https://learn.netdata.cloud\n Support    : https://github.com/netdata/netdata/issues\n License    : https://github.com/netdata/netdata/blob/master/LICENSE.md\n\n Twitter    : https://twitter.com/netdatahq\n LinkedIn   : https://linkedin.com/company/netdata-cloud/\n Facebook   : https://facebook.com/linuxnetdata/\n\n\n";

const ADVANCED: &str = "\n Advanced options:\n\n  -W stacksize=N           Set the stacksize (in bytes).\n\n  -W debug_flags=N         Set runtime tracing to debug.log.\n\n  -W unittest              Run internal unittests and exit.\n\n  -W sqlite-meta-recover   Run recovery on the metadata database and exit.\n\n  -W sqlite-compact        Reclaim metadata database unused space and exit.\n\n  -W sqlite-analyze        Run update statistics and exit.\n\n  -W sqlite-alert-cleanup  Perform maintenance on the alerts table.\n\n  -W createdataset=N       Create a DB engine dataset of N seconds and exit.\n\n  -W stresstest=A,B,C,D,E,F,G\n                           Run a DB engine stress test for A seconds,\n                           with B writers and C readers, with a ramp up\n                           time of D seconds for writers, a page cache\n                           size of E MiB, an optional disk space limit\n                           of F MiB, G libuv workers (default 16) and exit.\n\n  -W prd-array-stress      Run PRD_ARRAY refcount stress test and exit.\n\n  -W set section option value\n                           set netdata.conf option from the command line.\n\n  -W buildinfo             Print the version, the configure options,\n                           a list of optional features, and whether they\n                           are enabled or not.\n\n  -W buildinfojson         Print the version, the configure options,\n                           a list of optional features, and whether they\n                           are enabled or not, in JSON format.\n\n  -W cmakecache            Print the cmake cache used for building this agent\n  -W simple-pattern pattern string\n                           Check if string matches pattern and exit.\n\n\n Signals netdata handles:\n\n  - HUP                    Close and reopen log files.\n  - USR2                   Reload health configuration.\n\n";

/// `help()`'s text; `config_dir` is `CONFIG_DIR`, which the `-c` default names.
pub fn help_text(config_dir: &str) -> String {
    let config_default = format!("{config_dir}/{}", crate::build::CONFIG_FILENAME);
    let width = OPTIONS
        .iter()
        .filter_map(|o| o.2.map(str::len))
        .max()
        .unwrap_or(0)
        .clamp(20, 30);
    let mut out = String::from(BANNER);
    out.push_str(" SYNOPSIS: netdata [options]\n\n Options:\n\n");
    for &(letter, description, arg, default) in OPTIONS {
        out.push_str(&format!(
            "  -{letter} {:<width$}  {description}",
            arg.unwrap_or("")
        ));
        let default = if letter == 'c' {
            Some(config_default.as_str())
        } else {
            default
        };
        match default {
            Some(default) => out.push_str(&format!("\n     {:<width$}  Default: {default}\n", "")),
            None => out.push('\n'),
        }
        out.push('\n');
    }
    out.push_str(ADVANCED);
    out
}

/// One `getopt()` result. Arguments are bytes: the kernel passes no encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opt {
    Flag(u8),
    WithArg(u8, Vec<u8>),
    /// `getopt()` returned `'?'`; the message glibc printed to stderr is included.
    Invalid(Vec<u8>),
}

fn takes_argument(c: u8) -> Option<bool> {
    OPTIONS
        .iter()
        .find(|o| u32::from(c) == o.0 as u32)
        .map(|o| o.2.is_some())
}

/// GNU `getopt()` over `args` (without the program name): options may follow operands (argv is permuted), `--`
/// ends option parsing, clustered flags are split, and an option's argument is the rest of its word or the next
/// word. Operands are returned separately; the C daemon ignores them. `prog` prefixes glibc's messages, which name
/// the offending byte as glibc does.
pub fn getopt(prog: &[u8], args: &[Vec<u8>]) -> (Vec<Opt>, Vec<Vec<u8>>) {
    let mut opts = Vec::new();
    let mut operands = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg.as_slice() == b"--" {
            operands.extend(args[i..].iter().cloned());
            break;
        }
        if !arg.starts_with(b"-") || arg.len() == 1 {
            operands.push(arg.clone());
            continue;
        }
        let bytes = &arg[1..];
        let mut k = 0;
        while k < bytes.len() {
            let c = bytes[k];
            k += 1;
            let message = |what: &str| {
                let mut m = prog.to_vec();
                m.extend_from_slice(format!(": {what} -- '").as_bytes());
                m.push(c);
                m.extend_from_slice(b"'\n");
                m
            };
            match takes_argument(c) {
                None => opts.push(Opt::Invalid(message("invalid option"))),
                Some(false) => opts.push(Opt::Flag(c)),
                Some(true) => {
                    if k < bytes.len() {
                        opts.push(Opt::WithArg(c, bytes[k..].to_vec()));
                    } else if i < args.len() {
                        opts.push(Opt::WithArg(c, args[i].clone()));
                        i += 1;
                    } else {
                        opts.push(Opt::Invalid(message("option requires an argument")));
                    }
                    break;
                }
            }
        }
    }
    (opts, operands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_matches_the_c_binary() {
        let golden = include_str!("../tests/golden/help-master-prod.txt");
        assert_eq!(
            help_text("/home/dv/opt/master-prod/netdata/etc/netdata"),
            golden
        );
    }

    #[test]
    fn getopt_follows_gnu_rules() {
        let args: Vec<Vec<u8>> = [
            &b"-Dc"[..],
            b"/x.conf",
            b"operand",
            b"-p19998",
            b"-xP",
            b"-W",
            b"buildinfo",
            b"--",
            b"-D",
            b"-t",
        ]
        .iter()
        .map(|s| s.to_vec())
        .collect();
        let (opts, operands) = getopt(b"netdata", &args);
        assert_eq!(
            opts,
            vec![
                Opt::Flag(b'D'),
                Opt::WithArg(b'c', b"/x.conf".to_vec()),
                Opt::WithArg(b'p', b"19998".to_vec()),
                Opt::Invalid(b"netdata: invalid option -- 'x'\n".to_vec()),
                Opt::WithArg(b'P', b"-W".to_vec()),
                // "buildinfo" is an operand once -P took "-W" as its argument.
            ]
        );
        assert_eq!(operands, [&b"operand"[..], b"buildinfo", b"-D", b"-t"]);
        let (opts, _) = getopt(b"netdata", &[b"-t".to_vec()]);
        assert_eq!(
            opts,
            vec![Opt::Invalid(
                b"netdata: option requires an argument -- 't'\n".to_vec()
            )]
        );
        // Not UTF-8: glibc names the single byte.
        let (opts, operands) = getopt(b"netdata", &[vec![0xff], vec![b'-', 0xc3, 0xa9]]);
        assert_eq!(operands, [vec![0xff]]);
        assert_eq!(
            opts[0],
            Opt::Invalid(b"netdata: invalid option -- '\xc3'\n".to_vec())
        );
    }
}
