//! Construction and parsing of remote object-storage keys.
//!
//! ## Bucket layout
//!
//! - **SFST**: `{tenant_id}/sfst/{YYYY-MM-DD}/{file_id}.sfst`
//!
//!   Tenant-first. SFSTs are fetched by known key (from a catalog entry's
//!   `remote_key`), never LIST-enumerated by date, so per-tenant IAM
//!   policies map naturally onto the prefix.
//!
//! - **Catalog**: `{YYYY-MM-DD}/{tenant_id}/catalog/{machine}-{boot}-{max_seq}.catalog`
//!
//!   Date-first. Catalogs are enumerated per-date for query discovery,
//!   and bucket-level lifecycle rules match date prefixes.
//!
//! All layout decisions live in this module — constructors and the
//! inverse `parse_*` functions sit together so they stay in sync.

use chrono::NaiveDate;
use file_registry::FileId;
use uuid::Uuid;

/// Remote key for an uploaded SFST file.
pub fn sfst(tenant_id: &str, date: NaiveDate, id: FileId) -> String {
    format!(
        "{}/sfst/{}/{}",
        tenant_id,
        date.format("%Y-%m-%d"),
        id.to_filename("sfst"),
    )
}

/// LIST prefix for every SFST uploaded for `tenant_id` on `date`.
pub fn sfst_prefix(tenant_id: &str, date: NaiveDate) -> String {
    format!("{}/sfst/{}/", tenant_id, date.format("%Y-%m-%d"))
}

/// Remote key for a rotated catalog file.
pub fn catalog(
    date: NaiveDate,
    tenant_id: &str,
    machine_id: Uuid,
    boot_id: Uuid,
    max_seq: u64,
) -> String {
    format!(
        "{}/{}/catalog/{}",
        date.format("%Y-%m-%d"),
        tenant_id,
        otel_catalog::registry::filename(machine_id, boot_id, max_seq),
    )
}

/// Extract the date from an SFST remote key.
///
/// Expected shape: `{tenant_id}/sfst/{YYYY-MM-DD}/{file_id}.sfst`.
/// Returns `None` if the key doesn't match this shape.
pub fn parse_sfst_date(key: &str) -> Option<NaiveDate> {
    let mut parts = key.split('/');
    let _tenant = parts.next()?;
    let prefix = parts.next()?;
    if prefix != "sfst" {
        return None;
    }
    let date_str = parts.next()?;
    NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Uuid {
        Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }

    fn boot() -> Uuid {
        Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111)
    }

    fn sample_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()
    }

    #[test]
    fn sfst_key_and_date_roundtrip() {
        let id = FileId::new(machine(), boot(), 42, 0);
        let key = sfst("tenant1", sample_date(), id);
        assert!(key.starts_with("tenant1/sfst/2026-04-17/"));
        assert!(key.ends_with(".sfst"));
        assert_eq!(parse_sfst_date(&key), Some(sample_date()));
    }

    #[test]
    fn sfst_prefix_has_trailing_slash() {
        assert_eq!(
            sfst_prefix("tenant1", sample_date()),
            "tenant1/sfst/2026-04-17/",
        );
    }

    #[test]
    fn catalog_key_is_date_first() {
        let key = catalog(sample_date(), "tenant1", machine(), boot(), 100);
        assert!(key.starts_with("2026-04-17/tenant1/catalog/"));
        assert!(key.ends_with(".catalog"));
    }

    #[test]
    fn parse_sfst_date_happy_path() {
        let key = "tenant1/sfst/2026-04-17/abc123.sfst";
        assert_eq!(parse_sfst_date(key), Some(sample_date()));
    }

    #[test]
    fn parse_sfst_date_rejects_unknown_shapes() {
        assert!(parse_sfst_date("").is_none());
        assert!(parse_sfst_date("tenant1").is_none());
        assert!(parse_sfst_date("tenant1/catalog/2026-04-17/x").is_none());
        assert!(parse_sfst_date("tenant1/sfst/not-a-date/x").is_none());
        assert!(parse_sfst_date("tenant1/sfst").is_none());
    }
}
