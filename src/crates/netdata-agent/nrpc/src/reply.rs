//! What a call answers with: C's result `BUFFER` as nRPC uses it (its bytes, content type and expiry), and
//! `nrpc_call_error()` (`src/libnetdata/json/json-c-parser-inline.c`).

use netdata_agent_text::json::{JsonOptions, JsonWriter};
pub use netdata_agent_web::content_type::ContentType;

/// A call's result buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub body: Vec<u8>,
    pub content_type: ContentType,
    /// Seconds since the epoch; 0 is none.
    pub expires: i64,
    /// `BUFFER_FLAG_NO_CACHE` cleared.
    pub cacheable: bool,
}

impl Reply {
    /// An empty buffer of `content_type` (`buffer_create()` then the caller's content type).
    pub fn new(content_type: ContentType) -> Self {
        Reply { body: Vec::new(), content_type, expires: 0, cacheable: false }
    }

    /// `nrpc_call_error()`: the buffer becomes `{"status":code,"errorMessage":msg}`, expiring in a second.
    pub fn error(&mut self, msg: &str, code: u16) -> u16 {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        w.member_add_int64("status", i64::from(code));
        w.member_add_string("errorMessage", msg);
        w.finalize();
        self.body = w.as_bytes().to_vec();
        self.content_type = ContentType::ApplicationJson;
        self.expires = now_realtime_s() + 1;
        self.cacheable = false;
        code
    }
}

/// A call's payload (`FUNCTION_PAYLOAD`'s body) and its content type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub body: Vec<u8>,
    pub content_type: ContentType,
}

/// `now_realtime_sec()`.
pub(crate) fn now_realtime_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C's error body: minified, escaped, JSON, a second to live, not cacheable.
    #[test]
    fn an_error_is_cs_body() {
        let mut r = Reply::new(ContentType::TextPlain);
        r.body = b"partial".to_vec();
        r.cacheable = true;
        let before = now_realtime_s();
        assert_eq!(r.error("a \"quoted\" text", 503), 503);
        assert_eq!(r.body, br#"{"status":503,"errorMessage":"a \"quoted\" text"}"#);
        assert_eq!((r.content_type, r.cacheable), (ContentType::ApplicationJson, false));
        assert!(r.expires == before + 1 || r.expires == before + 2, "{}", r.expires);
    }
}
