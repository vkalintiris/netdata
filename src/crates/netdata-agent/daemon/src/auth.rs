//! The web server's authorization, ported from `src/web/api/http_auth.c` and `src/web/api/v3/api_v3_me.c`: bearer
//! token protection and what `/api/v3/me` reports. Decisions D96 in the status repository.

use std::sync::atomic::{AtomicBool, Ordering};

use netdata_agent_inicfg::{Config, SECTION_WEB};
use netdata_agent_nrpc::access;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::access_log::Auth;
use crate::server::Reply;

/// `netdata_is_protected_by_bearer`.
static BEARER_PROTECTION: AtomicBool = AtomicBool::new(false);

/// `netdata_bearer_protection_is_enabled()`.
pub fn bearer_protection() -> bool {
    BEARER_PROTECTION.load(Ordering::Relaxed)
}

/// `bearer_tokens_init()`'s key, at the "saved bearer tokens" step.
pub fn init(conf: &mut Config) {
    let enabled = conf.get_boolean(SECTION_WEB, "bearer token protection", bearer_protection());
    BEARER_PROTECTION.store(enabled, Ordering::Relaxed);
}

/// `api_v3_me()`: the caller's method, Cloud account, name, access and role (Cloud users arrive with the ACLK).
pub fn me(auth: &Auth) -> Reply {
    let (client_name, account) = auth.identity();
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    w.member_add_string("auth", if auth.is_bearer() { "bearer" } else { "none" });
    w.member_add_uuid("cloud_account_id", &account);
    w.member_add_string("client_name", &client_name);
    w.member_add_array(Some(b"access"));
    for name in access::names(auth.access()) {
        w.add_array_item_string(name);
    }
    w.array_close();
    w.member_add_string("user_role", auth.role());
    w.finalize();
    Reply {
        code: status::OK,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C's two anonymous bodies, protection off and on.
    #[test]
    fn me_as_c() {
        let auth = Auth::default();
        auth.authorize_anonymous(false);
        assert_eq!(
            String::from_utf8(me(&auth).body).unwrap(),
            r#"{"auth":"none","cloud_account_id":null,"client_name":"","access":["anonymous-data"],"user_role":"any"}"#
        );
        let auth = Auth::default();
        auth.authorize_anonymous(true);
        assert_eq!(
            String::from_utf8(me(&auth).body).unwrap(),
            r#"{"auth":"none","cloud_account_id":null,"client_name":"","access":[],"user_role":"none"}"#
        );
    }
}
