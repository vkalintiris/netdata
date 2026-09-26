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

/// `-W simple-pattern` without its two words.
pub const SIMPLE_PATTERN_USAGE: &str = "\nUSAGE: -W simple-pattern 'pattern' 'string'\n\n Checks if 'pattern' matches the given 'string'.\n - 'pattern' can be one or more space separated words.\n - each 'word' can contain one or more asterisks.\n - words starting with '!' give negative matches.\n - words are processed left to right\n\nExamples:\n\n > match all veth interfaces, except veth0:\n\n   -W simple-pattern '!veth0 veth*' 'veth12'\n\n\n > match all *.ext files directly in /path/:\n   (this will not match *.ext files in a subdir of /path/)\n\n   -W simple-pattern '!/path/*/*.ext /path/*.ext' '/path/test.ext'\n\n";

/// `-W set` without its three words.
pub const SET_USAGE: &str = "\nUSAGE: -W set 'section' 'key' 'value'\n\n Overwrites settings of netdata.conf.\n\n These options interact with: -c netdata.conf\n If -c netdata.conf is given on the command line,\n before -W set... the user may overwrite command\n line parameters at netdata.conf\n If -c netdata.conf is given after (or missing)\n -W set... the user cannot overwrite the command line\n parameters.\n";

/// `-W set2` without its four words (C's text names `-W set`).
pub const SET2_USAGE: &str = "\nUSAGE: -W set 'conf_file' 'section' 'key' 'value'\n\n Overwrites settings of netdata.conf or cloud.conf\n\n These options interact with: -c netdata.conf\n If -c netdata.conf is given on the command line,\n before -W set... the user may overwrite command\n line parameters at netdata.conf\n If -c netdata.conf is given after (or missing)\n -W set... the user cannot overwrite the command line\n parameters. conf_file can be \"cloud\" or \"netdata\".\n\n";

/// `-W get` without its three words.
pub const GET_USAGE: &str = "\nUSAGE: -W get 'section' 'key' 'value'\n\n Prints settings of netdata.conf.\n\n These options interact with: -c netdata.conf\n -c netdata.conf has to be given before -W get.\n\n";

/// `-W get2` without its four words.
pub const GET2_USAGE: &str = "\nUSAGE: -W get2 'conf_file' 'section' 'key' 'value'\n\n Prints settings of netdata.conf or cloud.conf\n\n These options interact with: -c netdata.conf\n -c netdata.conf has to be given before -W get2.\n conf_file can be \"cloud\" or \"netdata\".\n\n";

/// `-W simple-pattern PATTERN STRING`: the verdict on stdout, and the exit status (0 for a match of either sign).
pub fn simple_pattern_check(pattern: &[u8], string: &[u8], out: &mut impl std::io::Write) -> i32 {
    use netdata_agent_text::simple_pattern::{
        Separators, SimplePattern, SimplePatternMode, SimplePatternResult,
    };
    let p = SimplePattern::new(
        pattern,
        Separators::Whitespace,
        SimplePatternMode::Exact,
        true,
    );
    let (result, wildcarded) = p.matches_extract(string, string.len() + 1);
    let (verdict, code) = match result {
        SimplePatternResult::MatchedPositive => ("POSITIVE MATCHED - pattern '", 0),
        SimplePatternResult::MatchedNegative => ("NEGATIVE MATCHED - pattern '", 0),
        SimplePatternResult::NotMatched => ("NOT MATCHED - pattern '", 1),
    };
    let mut line = b"RESULT: ".to_vec();
    line.extend_from_slice(verdict.as_bytes());
    line.extend_from_slice(pattern);
    line.extend_from_slice(if code == 0 {
        b"' matches '"
    } else {
        b"' does not match '"
    });
    line.extend_from_slice(string);
    line.extend_from_slice(b"', wildcarded '");
    line.extend_from_slice(&wildcarded);
    line.extend_from_slice(b"'\n");
    let _ = out.write_all(&line);
    code
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

/// GNU `getopt()` over `args` (without the program name), one option at a time: options may follow operands (argv
/// is permuted), `--` ends option parsing, clustered flags are split, and an option's argument is the rest of its
/// word or the next word. Operands are collected separately; the C daemon ignores them. `prog` prefixes glibc's
/// messages, which name the offending byte as glibc does.
pub struct Getopt<'a> {
    prog: &'a [u8],
    args: &'a [Vec<u8>],
    /// `optind`: the next word to look at.
    next: usize,
    /// A word of clustered flags being split: its index and the next byte.
    cluster: Option<(usize, usize)>,
    pub operands: Vec<Vec<u8>>,
}

impl<'a> Getopt<'a> {
    pub fn new(prog: &'a [u8], args: &'a [Vec<u8>]) -> Self {
        Getopt {
            prog,
            args,
            next: 0,
            cluster: None,
            operands: Vec::new(),
        }
    }

    /// What `main()` does with `argv[optind]` after an option: the next `n` words, whatever they are, or `None`
    /// when fewer remain (`optind + n > argc`).
    pub fn take_words(&mut self, n: usize) -> Option<Vec<Vec<u8>>> {
        if self.cluster.is_some() || self.next + n > self.args.len() {
            return None;
        }
        let words = self.args[self.next..self.next + n].to_vec();
        self.next += n;
        Some(words)
    }
}

impl Iterator for Getopt<'_> {
    type Item = Opt;

    fn next(&mut self) -> Option<Opt> {
        loop {
            if let Some((word, k)) = self.cluster {
                let bytes = &self.args[word][1..];
                if k < bytes.len() {
                    let c = bytes[k];
                    let message = |what: &str| {
                        let mut m = self.prog.to_vec();
                        m.extend_from_slice(format!(": {what} -- '").as_bytes());
                        m.push(c);
                        m.extend_from_slice(b"'\n");
                        m
                    };
                    self.cluster = Some((word, k + 1));
                    return Some(match takes_argument(c) {
                        None => Opt::Invalid(message("invalid option")),
                        Some(false) => Opt::Flag(c),
                        Some(true) => {
                            self.cluster = None;
                            if k + 1 < bytes.len() {
                                Opt::WithArg(c, bytes[k + 1..].to_vec())
                            } else if self.next < self.args.len() {
                                self.next += 1;
                                Opt::WithArg(c, self.args[self.next - 1].clone())
                            } else {
                                Opt::Invalid(message("option requires an argument"))
                            }
                        }
                    });
                }
                self.cluster = None;
            }
            let arg = self.args.get(self.next)?;
            self.next += 1;
            if arg.as_slice() == b"--" {
                self.operands.extend(self.args[self.next..].iter().cloned());
                self.next = self.args.len();
                return None;
            }
            if !arg.starts_with(b"-") || arg.len() == 1 {
                self.operands.push(arg.clone());
                continue;
            }
            self.cluster = Some((self.next - 1, 0));
        }
    }
}

/// Every option of `args` at once, and the operands.
#[cfg(test)]
pub fn getopt(prog: &[u8], args: &[Vec<u8>]) -> (Vec<Opt>, Vec<Vec<u8>>) {
    let mut g = Getopt::new(prog, args);
    let opts = g.by_ref().collect();
    (opts, g.operands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_patterns_as_c() {
        let check = |p: &[u8], s: &[u8]| {
            let mut out = Vec::new();
            let code = simple_pattern_check(p, s, &mut out);
            (code, String::from_utf8(out).unwrap())
        };
        assert_eq!(
            check(b"!veth0 veth*", b"veth12"),
            (0, "RESULT: POSITIVE MATCHED - pattern '!veth0 veth*' matches 'veth12', wildcarded '12'\n".into())
        );
        assert_eq!(
            check(b"!veth0 veth*", b"veth0"),
            (0, "RESULT: NEGATIVE MATCHED - pattern '!veth0 veth*' matches 'veth0', wildcarded ''\n".into())
        );
        assert_eq!(
            check(b"a*", b"b"),
            (
                1,
                "RESULT: NOT MATCHED - pattern 'a*' does not match 'b', wildcarded ''\n".into()
            )
        );
    }

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
        // -W set reads the next three words, options or not, and scanning goes on after them
        let args: Vec<Vec<u8>> = [&b"-Wset"[..], b"web", b"-x", b"1", b"-D"]
            .iter()
            .map(|s| s.to_vec())
            .collect();
        let mut g = Getopt::new(b"netdata", &args);
        assert_eq!(g.next(), Some(Opt::WithArg(b'W', b"set".to_vec())));
        assert_eq!(
            g.take_words(3),
            Some(vec![b"web".to_vec(), b"-x".to_vec(), b"1".to_vec()])
        );
        assert_eq!(g.next(), Some(Opt::Flag(b'D')));
        assert_eq!(g.take_words(1), None);
        let (opts, operands) = getopt(b"netdata", &[vec![0xff], vec![b'-', 0xc3, 0xa9]]);
        assert_eq!(operands, [vec![0xff]]);
        assert_eq!(
            opts[0],
            Opt::Invalid(b"netdata: invalid option -- '\xc3'\n".to_vec())
        );
    }
}
