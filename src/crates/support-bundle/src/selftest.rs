//! Adversarial sanitizer regression suite. This is the union of the vector
//! suites embedded in the original netdata-support-bundle.sh (--selftest) and
//! netdata-support-bundle.ps1 (-SelfTest); it runs both at runtime via
//! `--selftest` and under `cargo test`. Every rule change must keep it green.

use crate::sanitize::{Identity, Sanitizer};

fn test_identity() -> Identity {
    Identity::gated("testhost99", "testhost99.example.com", "testuser9")
}

/// The .sh heredoc vector file, verbatim (plus the runtime-assembled
/// Authorization line, split here so secret scanners do not flag the source).
fn sh_vector_text() -> String {
    let text = r#"api key = SENTINEL-1
password: SENTINEL-3
"claim_token": "SENTINEL-4"
url: https://admin:SENTINEL-5@app.example.com/x
dsn: user:SENTINEL-6@tcp(10.1.2.3:3306)/db
TELEGRAM_BOT_TOKEN="SENTINEL-8"
TOKEN=false
PASSWORD=/etc/SENTINEL-9
GET /api/v1/data?chart=x&token=SENTINEL-10&after=-60
/usr/sbin/netdata-claim.sh -token=SENTINEL-11 -rooms=abc
cmdline: /usr/sbin/agent -token=SENTINEL-14 --verbose
connect user:SENTINEL-15@unix(/run/x)/db ok
/etc/netdata/claim_token: SENTINEL-16
cmdline: claim.sh api key = SENTINEL-12 end
password: q
"api_token": 731942
private_key: |
  SENTINEL-YAML-LINE1
  SENTINEL-YAML-LINE2
after_block = ok
[11111111-2222-3333-4444-555555555555]
-----BEGIN RSA PRIVATE KEY-----
U0VOVElORUwtMTMtUEVNLUJPRFk=
-----END RSA PRIVATE KEY-----
bearer token protection = no
netdata management api key file = /var/lib/netdata/netdata.api.key
TCP SYN cookies = auto
destination = parent.bigcorp.example:19999
destination = tcp:protoparent.example.com:19999
# destination = old-parent.example.org:19999
destination = [2001:db8::77]:19999 unix:/run/nd.sock 10.7.7.7:19999
tcp LISTEN 0 4096 later-line
server at 10.1.2.3 and 2606:4700:10::ac42:aad8 and 2001:470:26:307:0:0:0:1
mail ops@example.com mac aa:bb:cc:dd:ee:ff at 2026-07-16T13:38:34Z
"password_escq": "ab\"SENTINEL-ESCQ"
PWD=SENTINEL-PWD
"api_token": -98765
"access_key": ["SENTINEL-ARR"]
tabbed_secret_block: |
\tSENTINEL-TAB-LINE
after_tab = ok
password_ind: |2
  SENTINEL-IND2
after_ind = ok
password_chomp: |+2
  SENTINEL-CHOMP
after_chomp = ok
home /home/alice/x and /Users/bob/y
"#
    .to_string();
    // the raw string cannot hold a tab escape: splice the real tab in
    let mut text = text.replace("\\tSENTINEL-TAB-LINE", "\tSENTINEL-TAB-LINE");
    let bw = format!("{}{}", "Bea", "rer");
    text.push_str(&format!("Authorization: {bw} SENTINEL-2abc\n"));
    text
}

const SH_ABSENT: &[(&str, &str)] = &[
    ("SENTINEL-", "a planted secret survived"),
    ("U0VOVElORUw", "PEM body survived"),
    (
        "TOKEN=false",
        "TOKEN=false survived (values are never exempt)",
    ),
    ("731942", "scalar JSON secret survived"),
    ("password: q", "one-character secret survived"),
    ("2606:4700", "compressed IPv6 survived"),
    (
        "2001:470:26:307:0:0:0:1",
        "uncompressed numeric IPv6 survived",
    ),
    ("10.1.2.3", "IPv4 survived"),
    ("10.7.7.7", "IP destination not pseudonymized as an IP"),
    (
        "parent.bigcorp.example",
        "stream destination hostname survived",
    ),
    (
        "protoparent.example.com",
        "protocol-prefixed destination hostname survived",
    ),
    (
        "old-parent.example.org",
        "commented-out destination hostname survived",
    ),
    ("2001:db8::77", "bracketed IPv6 destination leaked"),
    ("ops@example.com", "email survived"),
    ("aa:bb:cc:dd:ee:ff", "MAC survived"),
    (
        "SENTINEL-ESCQ",
        "escaped-quote JSON value leaked its suffix",
    ),
    ("SENTINEL-PWD", "PWD= secret alias survived"),
    ("98765", "negative-number JSON scalar survived"),
    (
        "SENTINEL-ARR",
        "structured (array) JSON secret value survived",
    ),
    (
        "SENTINEL-TAB",
        "tab-indented YAML block-scalar secret survived",
    ),
    (
        "SENTINEL-IND2",
        "explicit-indent (|2) YAML block scalar secret survived",
    ),
    (
        "SENTINEL-CHOMP",
        "chomp-then-indent (|+2) YAML block scalar secret survived",
    ),
    ("/home/alice", "other-user home path not pseudonymized"),
    ("/Users/bob", "other-user Users path not pseudonymized"),
];

const SH_PRESENT: &[(&str, &str)] = &[
    (
        "after_block = ok",
        "YAML block withholding ate following content",
    ),
    ("destination = tcp:", "destination protocol prefix lost"),
    ("unix:/run/nd.sock", "socket-path destination was mangled"),
    (
        "tcp LISTEN 0 4096 later-line",
        "literal tcp corrupted by fqmap pollution",
    ),
    (
        "bearer token protection = no",
        "diagnostic option lost (key-based exemption broken)",
    ),
    (
        "api key file = /var/lib/netdata/netdata.api.key",
        "key-file path lost",
    ),
    ("TCP SYN cookies = auto", "SYN cookies value lost"),
    ("[REDACTED PRIVATE KEY BLOCK]", "PEM block marker missing"),
    ("2026-07-16T13:38:34Z", "timestamp mangled by IPv6 rule"),
    (
        "--verbose",
        "path-bearing argv line was eaten by the kv rule",
    ),
    (
        "@unix(/run/x)/db ok",
        "mid-line unix( DSN rule broke the tail",
    ),
    ("after_tab = ok", "tab YAML block withholding overran"),
    ("after_ind = ok", "|2 block withholding overran"),
    ("after_chomp = ok", "|+2 block withholding overran"),
    ("/home/user-", "home path pseudonym missing"),
    ("/Users/user-", "Users path pseudonym missing"),
];

struct Ps1Vector {
    input: &'static str,
    must_not: &'static [&'static str],
    must: &'static [&'static str],
}

/// The .ps1 single-line vectors, verbatim. Checked case-insensitively, as the
/// original does. The Authorization vector is assembled at runtime below.
const PS1_VECTORS: &[Ps1Vector] = &[
    Ps1Vector {
        input: "api key = SENTINEL-1",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "password: SENTINEL-3",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "\"claim_token\": \"SENTINEL-4\"",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "url: https://admin:SENTINEL-5@app.example.com/x",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]@"],
    },
    Ps1Vector {
        input: "dsn: user:SENTINEL-6@tcp(10.1.2.3:3306)/db",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "TELEGRAM_BOT_TOKEN=\"SENTINEL-7\"",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "DEFAULT_RECIPIENT_SLACK=\"SENTINEL-8\"",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "[11111111-2222-3333-4444-555555555555]",
        must_not: &["11111111"],
        must: &["[REDACTED-KEY-SECTION]"],
    },
    Ps1Vector {
        input: "destination = parent.example.internal:19999",
        must_not: &["parent.example"],
        must: &["private-host"],
    },
    Ps1Vector {
        input: "server at 10.1.2.3 talked to 192.168.5.7 then 10.1.2.3 again",
        must_not: &["10.1.2.3", "192.168"],
        must: &["ip-"],
    },
    Ps1Vector {
        input: "admin email is ops@customer-corp.com on host testhost99",
        must_not: &["customer-corp", "testhost99"],
        must: &["[EMAIL]", "redacted-host"],
    },
    Ps1Vector {
        input: "GET /api/v1/data?chart=x&token=SENTINEL-9&after=-60",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]", "after=-60"],
    },
    Ps1Vector {
        input: "netdata 1234 /usr/sbin/netdata-claim.sh -token=SENTINEL-10 -rooms=abc",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]", "-rooms=abc"],
    },
    Ps1Vector {
        input: "Environment: NETDATA_CLAIM_TOKEN=SENTINEL-11 PATH=/usr/bin",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "IPv6: 2606:4700:10::ac42:aad8 and fe80::1ff:fe23:4567:890a",
        must_not: &["2606:4700", "fe80::1ff"],
        must: &["ip6-"],
    },
    Ps1Vector {
        input: "peer 2001:470:26:307:0:0:0:1 connected",
        must_not: &["2001:470"],
        must: &["ip6-"],
    },
    Ps1Vector {
        input: "captured: 2026-07-16T13:38:34Z listening on ::1 and file.c:123",
        must_not: &[],
        must: &["13:38:34Z", "::1", "file.c:123"],
    },
    Ps1Vector {
        input: "# bearer token protection = no",
        must_not: &["[REDACTED]"],
        must: &["bearer token protection = no"],
    },
    Ps1Vector {
        input: "# TCP SYN cookies = auto",
        must_not: &["[REDACTED]"],
        must: &["auto"],
    },
    Ps1Vector {
        input: "# netdata management api key file = /var/lib/netdata/netdata.api.key",
        must_not: &["[REDACTED]"],
        must: &["/var/lib/netdata/netdata.api.key"],
    },
    Ps1Vector {
        input: "cmdline: /x/claim.sh api key = SENTINEL-12 end",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]", "end"],
    },
    Ps1Vector {
        input: "TOKEN=false",
        must_not: &["false"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "PASSWORD=/root/x",
        must_not: &["/root/x"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "destination = tcp:parent.example.com:19999",
        must_not: &["parent.example.com"],
        must: &["tcp:", "private-host", ":19999"],
    },
    Ps1Vector {
        input: "tcp LISTEN 0 4096 *:19999",
        must_not: &["private-host"],
        must: &["tcp LISTEN"],
    },
    Ps1Vector {
        input: "# destination = old-parent.example.org:19999",
        must_not: &["old-parent.example"],
        must: &["private-host", ":19999"],
    },
    Ps1Vector {
        input: "connect user:SENTINEL-13@unix(/run/x)/db ok",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]@unix(/run/x)/db", "ok"],
    },
    Ps1Vector {
        input: "/etc/netdata/claim_token: SENTINEL-14",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "destination = [2001:db8::77]:19999 unix:/run/nd.sock",
        must_not: &["2001:db8::77"],
        must: &["ip6-", "]:19999", "unix:/run/nd.sock"],
    },
    Ps1Vector {
        input: "password: q",
        must_not: &[": q"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "\"api_token\": 731942",
        must_not: &["731942"],
        must: &["[REDACTED]"],
    },
];

/// Vectors for the rules the Rust implementation adds beyond the scripts:
/// short credential aliases (pass/pwd/pat, whole-word), escape-aware JSON
/// strings, and secret-keyed JSON containers.
const RUST_VECTORS: &[Ps1Vector] = &[
    Ps1Vector {
        input: "pass: SENTINEL-R1",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "pwd = SENTINEL-R2",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "smtp_pass: SENTINEL-R3",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "/usr/bin/tool --pass=SENTINEL-R4 --other=x",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]", "--other=x"],
    },
    Ps1Vector {
        input: "bypass = enabled for compass mode",
        must_not: &["[REDACTED]"],
        must: &["bypass = enabled for compass mode"],
    },
    Ps1Vector {
        input: "pattern = abc123 and path = /etc/x",
        must_not: &["[REDACTED]"],
        must: &["pattern = abc123", "path = /etc/x"],
    },
    Ps1Vector {
        input: r#"{"password":"pre\"SENTINEL-R5 tail"}"#,
        must_not: &["SENTINEL", "tail"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: r#"{"api_token":{"value":"SENTINEL-R6"},"next":1}"#,
        must_not: &["SENTINEL"],
        must: &["[REDACTED]", "\"next\":1"],
    },
    Ps1Vector {
        input: r#"{"api_token":["SENTINEL-R7","SENTINEL-R7b"],"after":true}"#,
        must_not: &["SENTINEL"],
        must: &["[REDACTED]", "\"after\":true"],
    },
    Ps1Vector {
        input: r#"{"files":[{"name":"ok.txt"}],"count":1}"#,
        must_not: &["REDACTED"],
        must: &["ok.txt", "\"count\":1"],
    },
    Ps1Vector {
        input: ";password = SENTINEL-SEMI",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: ";destination = semi.example.net:19999",
        must_not: &["semi.example.net"],
        must: &["private-host"],
    },
    // dotted quads in version strings match the IPv4 rule on purpose:
    // over-redaction is the safe direction, and this pins that choice
    Ps1Vector {
        input: "library version 1.2.3.4 loaded",
        must_not: &["1.2.3.4"],
        must: &["ip-"],
    },
    // both spellings of the IPv6 loopback survive, like 127.0.0.1 does
    Ps1Vector {
        input: "listening on ::1 and 0:0:0:0:0:0:0:1",
        must_not: &["ip6-"],
        must: &["::1", "0:0:0:0:0:0:0:1"],
    },
    // toggle keys stay readable even when they contain a secret word
    Ps1Vector {
        input: "auth enabled = yes and sso_disabled = no",
        must_not: &["[REDACTED]"],
        must: &["auth enabled = yes", "sso_disabled = no"],
    },
    // a pure-hex hostname is still a hostname
    Ps1Vector {
        input: "destination = beef:19999",
        must_not: &["beef"],
        must: &["private-host", ":19999"],
    },
    Ps1Vector {
        input: r"profile at C:\Users\carol\AppData",
        must_not: &["carol"],
        must: &[r"C:\Users\user-", r"\AppData"],
    },
    // compact/camelCase two-word phrases normalize into the secret list
    Ps1Vector {
        input: "accessKey = SENTINEL-CC1",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
    Ps1Vector {
        input: "proxyPass: SENTINEL-CC2",
        must_not: &["SENTINEL"],
        must: &["[REDACTED]"],
    },
];

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// Run the whole suite; returns the failure descriptions (empty = all pass).
pub fn run_vectors() -> Vec<String> {
    let mut fails = Vec::new();

    // --- the .sh file-level suite ---
    let mut s = Sanitizer::new(true, test_identity());
    let sanitized = s.sanitize_text(&sh_vector_text());
    for (pat, msg) in SH_ABSENT {
        if sanitized.contains(pat) {
            fails.push(format!("sh (leak): {msg}"));
        }
    }
    for (pat, msg) in SH_PRESENT {
        if !sanitized.contains(pat) {
            fails.push(format!("sh (over-redaction): {msg}"));
        }
    }

    // --- the .ps1 per-line suite (one sanitizer across vectors, like ps1) ---
    let mut s = Sanitizer::new(true, test_identity());
    for v in PS1_VECTORS {
        let out = s.sanitize_line(v.input);
        for pat in v.must_not {
            if contains_ci(&out, pat) {
                fails.push(format!(
                    "ps1 (leak): {:?} -> {:?} still has {:?}",
                    v.input, out, pat
                ));
            }
        }
        for pat in v.must {
            if !contains_ci(&out, pat) {
                fails.push(format!(
                    "ps1 (lost): {:?} -> {:?} misses {:?}",
                    v.input, out, pat
                ));
            }
        }
    }
    let bw = format!("{}{}", "Bea", "rer");
    let auth = format!("    Authorization: {bw} SENTINEL-2abc123");
    let out = s.sanitize_line(&auth);
    if contains_ci(&out, "SENTINEL") || !contains_ci(&out, "[REDACTED]") {
        fails.push(format!("ps1 (leak): auth header -> {out:?}"));
    }

    // --- the Rust-added hardening vectors ---
    let mut s = Sanitizer::new(true, test_identity());
    for v in RUST_VECTORS {
        let out = s.sanitize_line(v.input);
        for pat in v.must_not {
            if contains_ci(&out, pat) {
                fails.push(format!(
                    "rust (leak): {:?} -> {:?} still has {:?}",
                    v.input, out, pat
                ));
            }
        }
        for pat in v.must {
            if !contains_ci(&out, pat) {
                fails.push(format!(
                    "rust (lost): {:?} -> {:?} misses {:?}",
                    v.input, out, pat
                ));
            }
        }
    }
    // multi-line secret-keyed JSON container must be withheld through its
    // closing boundary, keeping unrelated content after it
    let json_in = concat!(
        "{\n",
        "  \"claim_token\": {\n",
        "    \"v\": \"SENTINEL-RJSON\"\n",
        "  },\n",
        "  \"keep\": \"yes\"\n",
        "}\n",
    );
    let json = s.sanitize_text(json_in);
    if json.contains("SENTINEL-RJSON")
        || !json.contains("[REDACTED-BLOCK]")
        || !json.contains("\"keep\": \"yes\"")
    {
        fails.push(format!(
            "rust: multi-line JSON container not withheld correctly -> {json:?}"
        ));
    }
    // an unterminated container fails closed to EOF
    let json_open = "{\n  \"api_token\": [\n    \"SENTINEL-ROPEN\"\n";
    let json = s.sanitize_text(json_open);
    if json.contains("SENTINEL-ROPEN") {
        fails.push("rust: unterminated JSON container leaked its body".to_string());
    }

    // --- the .ps1 file-level block tests ---
    let mut s = Sanitizer::new(true, test_identity());
    let pem_in = "before line\n-----BEGIN RSA PRIVATE KEY-----\n\
                  MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQ\n\
                  aGVsbG8gd29ybGQgdGhpcyBpcyBrZXkgbWF0ZXJpYWw=\n\
                  -----END RSA PRIVATE KEY-----\nafter line\n";
    let pem = s.sanitize_text(pem_in);
    if pem.contains("MIIEvQ")
        || pem.contains("aGVsbG8")
        || !pem.contains("[REDACTED PRIVATE KEY BLOCK]")
        || !pem.contains("before line")
        || !pem.contains("after line")
    {
        fails.push("ps1: PEM block not fully withheld".to_string());
    }
    let yaml_in = concat!(
        "jobs:\n",
        "  - name: x\n",
        "    private_key: |\n",
        "      SENTINEL-YAML-LINE1\n",
        "      SENTINEL-YAML-LINE2\n",
        "    after: ok\n",
    );
    let yaml = s.sanitize_text(yaml_in);
    if yaml.contains("SENTINEL-YAML")
        || !yaml.contains("[REDACTED BLOCK]")
        || !yaml.contains("after: ok")
    {
        fails.push("ps1: YAML block scalar not withheld correctly".to_string());
    }

    fails
}

/// NUL-byte withholding check; needs a scratch directory to write in.
pub fn run_nul_check(scratch: &std::path::Path) -> Vec<String> {
    let path = scratch.join("selftest-nul.txt");
    let mut fails = Vec::new();
    let payload = b"nul-test \x00 password=SENTINEL-NUL\n";
    if let Err(e) = std::fs::write(&path, payload) {
        fails.push(format!("nul check: cannot write scratch file: {e}"));
        return fails;
    }
    let mut s = Sanitizer::new(true, test_identity());
    if let Err(e) = s.sanitize_file(&path) {
        fails.push(format!("nul check: sanitize_file failed: {e}"));
    } else {
        let out = std::fs::read_to_string(&path).unwrap_or_default();
        if !out.contains("content withheld") {
            fails.push("NUL-bearing file was not withheld".to_string());
        }
    }
    let _ = std::fs::remove_file(&path);
    fails
}

/// The `--selftest` entry point: prints results, returns the process exit code.
pub fn run() -> i32 {
    let mut fails = run_vectors();
    fails.extend(run_nul_check(&std::env::temp_dir()));
    if fails.is_empty() {
        println!("netdata-support-bundle selftest: ALL PASS");
        return 0;
    }
    for f in &fails {
        eprintln!("FAIL: {f}");
    }
    eprintln!(
        "netdata-support-bundle selftest: {} FAILURE(S)",
        fails.len()
    );
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_vector_suite() {
        let fails = run_vectors();
        assert!(
            fails.is_empty(),
            "sanitizer vector failures:\n{}",
            fails.join("\n")
        );
    }

    #[test]
    fn nul_bytes_are_withheld() {
        let dir = std::env::temp_dir();
        let fails = run_nul_check(&dir);
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }
}
