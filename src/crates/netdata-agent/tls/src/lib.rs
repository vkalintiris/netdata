//! TLS of the agent over the system's OpenSSL (D10), ported from `src/libnetdata/socket/security.c`: the library's
//! initialization and the web server's context as `netdata_ssl_create_server_ctx()` builds it, with C's records.
//! Decisions D96 in the status repository.

use netdata_agent_log::{netdata_log_error, netdata_log_info};
use openssl::error::{Error, ErrorStack};
use openssl::ssl::{SslContext, SslContextBuilder, SslFiletype, SslMethod, SslMode, SslVerifyMode, SslVersion};

/// `netdata_conf_ssl()`: OpenSSL initialized, its configuration file loaded (`OPENSSL_INIT_LOAD_CONFIG`, a default
/// since 1.1.1).
pub fn init() {
    openssl::init();
}

/// The web server's `[web]` TLS keys.
#[derive(Debug, Clone, Copy)]
pub struct ServerConfig<'a> {
    /// `ssl key`.
    pub key: &'a str,
    /// `ssl certificate`.
    pub certificate: &'a str,
    /// `tls version`.
    pub tls_version: &'a str,
    /// `tls ciphers`: `none` keeps OpenSSL's.
    pub ciphers: &'a str,
    /// `ssl skip certificate verification`.
    pub skip_verification: bool,
}

/// `netdata_ssl_initialize_ctx(NETDATA_SSL_WEB_SERVER_CTX)`: the context, or none (the server then speaks plain
/// HTTP only), with C's records.
pub fn web_server_context(config: &ServerConfig<'_>) -> Option<SslContext> {
    let exists = |path: &str| std::fs::metadata(path).is_ok();
    if !exists(config.key) || !exists(config.certificate) {
        netdata_log_info!(
            "To use encryption it is necessary to set \"ssl certificate\" and \"ssl key\" in [web] !\n"
        );
        return None;
    }
    match server_context(config) {
        Ok(context) => Some(context),
        Err(record) => {
            netdata_log_error!("{record}");
            None
        }
    }
}

/// `netdata_ssl_create_server_ctx()` in C's order: the chain, the protocol versions, the cipher list (a failure is
/// only reported), the key, then the check that fails the context. The error record of a failure, else the context.
fn server_context(config: &ServerConfig<'_>) -> Result<SslContext, String> {
    let mut b = SslContextBuilder::new(SslMethod::tls_server())
        .map_err(|_| "Cannot create a new SSL context, netdata won't encrypt communication".to_string())?;
    // OpenSSL's error queue: the failures C ignores stay queued, and the key check's record prints the oldest
    let mut queue: Vec<Error> = Vec::new();
    let mut keep = |r: Result<(), ErrorStack>| {
        if let Err(stack) = r {
            queue.extend(stack.errors().iter().cloned());
        }
    };
    keep(b.set_certificate_chain_file(config.certificate));
    keep(b.set_min_proto_version(Some(SslVersion::TLS1)));
    keep(b.set_max_proto_version(Some(tls_version(config.tls_version))));
    if config.ciphers != "none" {
        let r = b.set_cipher_list(config.ciphers);
        if r.is_err() {
            netdata_log_error!("SSL error. cannot set the cipher list");
        }
        keep(r);
    }
    keep(b.set_private_key_file(config.key, SslFiletype::PEM));
    if let Err(stack) = b.check_private_key() {
        queue.extend(stack.errors().iter().cloned());
        return Err(format!(
            "SSL cannot check the private key: {}",
            queue.first().map(error_string).unwrap_or_default()
        ));
    }
    // netdata_id_context: the bytes of an `int` 1
    keep(b.set_session_id_context(&1i32.to_ne_bytes()));
    // the info callback feeds only debug records, compiled out of production builds
    b.set_mode(SslMode::ENABLE_PARTIAL_WRITE | SslMode::ACCEPT_MOVING_WRITE_BUFFER);
    if config.skip_verification {
        b.set_verify(SslVerifyMode::NONE);
    }
    Ok(b.build())
}

/// `netdata_ssl_select_tls_version()`: the highest version the server offers; another text is the library's
/// highest (`TLS_MAX_VERSION`, 1.3).
fn tls_version(text: &str) -> SslVersion {
    match text {
        "1" | "1.0" => SslVersion::TLS1,
        "1.1" => SslVersion::TLS1_1,
        "1.2" => SslVersion::TLS1_2,
        _ => SslVersion::TLS1_3,
    }
}

/// `ERR_error_string_n()` of OpenSSL 3: `error:<code>:<library>::<reason>`, the library and the reason by number when
/// OpenSSL has no text for them.
pub fn error_string(e: &Error) -> String {
    let code = e.code();
    let library = e.library().map_or_else(|| format!("lib({})", e.library_code()), str::to_string);
    let reason = e.reason().map_or_else(|| format!("reason({})", e.reason_code()), str::to_string);
    format!("error:{code:08X}:{library}::{reason}")
}

#[cfg(test)]
mod tests {
    use openssl::asn1::Asn1Time;
    use openssl::ec::{EcGroup, EcKey};
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::{PKey, Private};
    use openssl::x509::{X509, X509NameBuilder};

    use super::*;

    fn key() -> PKey<Private> {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
    }

    fn certificate(key: &PKey<Private>) -> X509 {
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut b = X509::builder().unwrap();
        b.set_version(2).unwrap();
        b.set_subject_name(&name).unwrap();
        b.set_issuer_name(&name).unwrap();
        b.set_pubkey(key).unwrap();
        b.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        b.set_not_after(&Asn1Time::days_from_now(1).unwrap()).unwrap();
        b.sign(key, MessageDigest::sha256()).unwrap();
        b.build()
    }

    fn files(dir: &tempfile::TempDir, key: &PKey<Private>, cert: &X509) -> (String, String) {
        let (k, c) = (dir.path().join("key.pem"), dir.path().join("cert.pem"));
        std::fs::write(&k, key.private_key_to_pem_pkcs8().unwrap()).unwrap();
        std::fs::write(&c, cert.to_pem().unwrap()).unwrap();
        (k.to_str().unwrap().into(), c.to_str().unwrap().into())
    }

    fn config<'a>(key: &'a str, certificate: &'a str, ciphers: &'a str) -> ServerConfig<'a> {
        ServerConfig { key, certificate, tls_version: "1.3", ciphers, skip_verification: false }
    }

    /// A matching pair builds; a key that is not the certificate's fails the check with the oldest queued error, a
    /// bad cipher list's when it failed first.
    #[test]
    fn contexts_as_c() {
        init();
        let dir = tempfile::tempdir().unwrap();
        let k = key();
        let (key_file, cert_file) = files(&dir, &k, &certificate(&k));
        assert!(server_context(&config(&key_file, &cert_file, "none")).is_ok());
        let other = tempfile::tempdir().unwrap();
        let (other_key, _) = files(&other, &key(), &certificate(&k));
        let mismatch = server_context(&config(&other_key, &cert_file, "none")).unwrap_err();
        assert!(mismatch.starts_with("SSL cannot check the private key: error:"), "{mismatch}");
        // the key's load queued the x509 error first
        assert!(mismatch.ends_with(":x509 certificate routines::key values mismatch"), "{mismatch}");
        let bad = server_context(&config(&other_key, &cert_file, "NOPE")).unwrap_err();
        assert!(bad.ends_with(":SSL routines::no cipher match"), "{bad}");
        assert!(server_context(&config(&key_file, &cert_file, "NOPE")).is_ok(), "only reported");
        std::fs::write(&key_file, "not a key").unwrap();
        let garbage = server_context(&config(&key_file, &cert_file, "none")).unwrap_err();
        assert!(garbage.starts_with("SSL cannot check the private key: error:"), "{garbage}");
    }

    #[test]
    fn versions_as_c() {
        let v = ["1", "1.0", "1.1", "1.2", "1.3", "bogus"].map(tls_version);
        assert_eq!(
            v,
            [
                SslVersion::TLS1,
                SslVersion::TLS1,
                SslVersion::TLS1_1,
                SslVersion::TLS1_2,
                SslVersion::TLS1_3,
                SslVersion::TLS1_3
            ]
        );
    }
}
