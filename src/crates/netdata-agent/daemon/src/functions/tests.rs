//! `api_v1_function()`'s parameters and `user_auth_to_source_buffer()`.

use std::sync::Arc;

use super::*;
use crate::access_log::Auth;

/// A query and the `function` and timeout read from it.
type Case = (&'static [u8], (Option<&'static [u8]>, i32));

/// C's loop: `&`-separated pairs split at their first `=`, empty pairs and names skipped, the last occurrence winning
/// (`function` without `=` clears it), the timeout as `(int) strtoul(v, NULL, 0)`.
#[test]
fn parameters_read_as_c() {
    let cases: [Case; 12] = [
        (b"function=top&timeout=3", (Some(b"top"), 3)),
        (b"function=a&function=", (Some(b""), 0)),
        (b"function=a&function", (None, 0)),
        (b"&&function=a%01b&&", (Some(b"a%01b"), 0)),
        (b"function=k=v", (Some(b"k=v"), 0)),
        (b"=x&function==y", (Some(b"=y"), 0)),
        (b"timeout=0x5", (None, 5)),
        (b"timeout=010", (None, 8)),
        (b"timeout=-5", (None, -5)),
        (b"timeout=abc", (None, 0)),
        (b"timeout=4294967299&timeout=4x", (None, 4)),
        (b"timeout=4294967299", (None, 3)),
    ];
    for (query, want) in cases {
        assert_eq!(parameters(query), want, "{}", String::from_utf8_lossy(query));
    }
    // a bare timeout crashes C (D135.9): the method's
    assert_eq!(parameters(b"function=a&timeout"), (Some(&b"a"[..]), 0));
}

fn ctx(auth: Auth, ip: &str) -> RequestContext {
    RequestContext { ip: ip.into(), auth: Arc::new(auth), ..RequestContext::default() }
}

/// `user_auth_to_source_buffer()`: method, role, permissions, then the token's user and account, the ip and
/// `X-Forwarded-For` only when set.
#[test]
fn sources_are_cs() {
    let anonymous = Auth::default();
    anonymous.authorize_anonymous(false);
    assert_eq!(source(&ctx(anonymous, "localhost"), b""), b"method=none,role=any,permissions=0x8,ip=localhost");
    let admin = Auth::default();
    let mut account = [0u8; 16];
    account[15] = 0xab;
    admin.authorize_bearer(0x7ff, netdata_agent_nrpc::access::role::ADMIN, "fnhttp-admin", account);
    assert_eq!(
        source(&ctx(admin, ""), b"10.1.2.3"),
        b"method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,account=000000000000000000000000000000ab,\
          forwarded_for=10.1.2.3"
            .to_vec()
    );
}
