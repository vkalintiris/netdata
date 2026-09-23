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

/// One `getopt()` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opt {
    Flag(char),
    WithArg(char, String),
    /// `getopt()` returned `'?'`; the message glibc printed to stderr is included.
    Invalid(String),
}

fn takes_argument(c: char) -> Option<bool> {
    OPTIONS.iter().find(|o| o.0 == c).map(|o| o.2.is_some())
}

/// GNU `getopt()` over `args` (without the program name): options may follow operands (argv is permuted), `--`
/// ends option parsing, clustered flags are split, and an option's argument is the rest of its word or the next
/// word. Operands are returned separately; the C daemon ignores them. `prog` prefixes glibc's messages.
pub fn getopt(prog: &str, args: &[String]) -> (Vec<Opt>, Vec<String>) {
    let mut opts = Vec::new();
    let mut operands = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            operands.extend(args[i..].iter().cloned());
            break;
        }
        if !arg.starts_with('-') || arg.len() == 1 {
            operands.push(arg.clone());
            continue;
        }
        let chars: Vec<char> = arg.chars().skip(1).collect();
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            k += 1;
            match takes_argument(c) {
                None => {
                    opts.push(Opt::Invalid(format!("{prog}: invalid option -- '{c}'\n")));
                }
                Some(false) => opts.push(Opt::Flag(c)),
                Some(true) => {
                    if k < chars.len() {
                        opts.push(Opt::WithArg(c, chars[k..].iter().collect()));
                    } else if i < args.len() {
                        opts.push(Opt::WithArg(c, args[i].clone()));
                        i += 1;
                    } else {
                        opts.push(Opt::Invalid(format!(
                            "{prog}: option requires an argument -- '{c}'\n"
                        )));
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
        let args: Vec<String> = [
            "-Dc",
            "/x.conf",
            "operand",
            "-p19998",
            "-xP",
            "-W",
            "buildinfo",
            "--",
            "-D",
            "-t",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (opts, operands) = getopt("netdata", &args);
        assert_eq!(
            opts,
            vec![
                Opt::Flag('D'),
                Opt::WithArg('c', "/x.conf".into()),
                Opt::WithArg('p', "19998".into()),
                Opt::Invalid("netdata: invalid option -- 'x'\n".into()),
                Opt::WithArg('P', "-W".into()),
                // "buildinfo" is an operand once -P took "-W" as its argument.
            ]
        );
        assert_eq!(operands, vec!["operand", "buildinfo", "-D", "-t"]);
        let (opts, _) = getopt("netdata", &["-t".to_string()]);
        assert_eq!(
            opts,
            vec![Opt::Invalid(
                "netdata: option requires an argument -- 't'\n".into()
            )]
        );
    }
}
