//! Construction and parsing of remote object-storage keys.
//!
//! ## Bucket layout (versioned at the root)
//!
//! ```text
//! v1/catalog/{YYYY-MM-DD}/{tenant_id}/{machine}-{boot}-{max_seq}-{min_ts}-{max_ts}.catalog
//! v1/tenants/{tenant_id}/sfst/{YYYY-MM-DD}/{file_id}.sfst
//! ```
//!
//! Top-level prefixes are artifact-first so a console browse / LIST `v1/`
//! immediately tells an operator what lives in the bucket.
//!
//! - **`v1/catalog/{date}/{tenant}/...`** — date-first under the catalog
//!   umbrella. Catalogs are LIST-enumerated per-(date, tenant) for query
//!   discovery, and bucket-level lifecycle rules attach naturally to a
//!   single date prefix (`v1/catalog/2025-01-01/`). The tenant segment is
//!   redundant with the body's `tenant_id` field but scopes per-tenant
//!   LISTs and IAM policies.
//!
//! - **`v1/tenants/{tenant}/sfst/{date}/...`** — tenant-first under the
//!   tenants umbrella. SFSTs are fetched by known key (drawn from a
//!   catalog entry's `remote_key`), never LIST-enumerated by date, so the
//!   prefix shape doesn't affect query discovery — only IAM policies
//!   (per-tenant scope) and lifecycle rules (date-bucketed under the
//!   tenant).
//!
//! - **WAL is absent.** WAL files are deleted post-index; they're
//!   ephemeral by design and never reach the remote.
//!
//! All layout decisions live in this module — constructors and the
//! inverse `parse_*` functions sit together so they stay in sync.

use chrono::NaiveDate;
use file_registry::{FileId, TenantId};
use uuid::Uuid;

/// Schema version prefix. Bumping this enables side-by-side migrations
/// (write `v2/...` while readers still handle `v1/...`).
const SCHEMA_VERSION: &str = "v1";

/// Remote key for an uploaded SFST file.
pub fn sfst(tenant_id: &TenantId, date: NaiveDate, id: FileId) -> String {
    format!(
        "{SCHEMA_VERSION}/tenants/{}/sfst/{}/{}",
        tenant_id,
        date.format("%Y-%m-%d"),
        id.to_filename("sfst"),
    )
}

/// LIST prefix for every SFST uploaded for `tenant_id` on `date`.
pub fn sfst_prefix(tenant_id: &TenantId, date: NaiveDate) -> String {
    format!(
        "{SCHEMA_VERSION}/tenants/{}/sfst/{}/",
        tenant_id,
        date.format("%Y-%m-%d"),
    )
}

/// Remote key for a rotated catalog file.
pub fn catalog(
    date: NaiveDate,
    tenant_id: &TenantId,
    machine_id: Uuid,
    boot_id: Uuid,
    max_seq: u64,
    min_timestamp_s: u32,
    max_timestamp_s: u32,
) -> String {
    format!(
        "{SCHEMA_VERSION}/catalog/{}/{}/{}",
        date.format("%Y-%m-%d"),
        tenant_id,
        otel_catalog::filename(
            machine_id,
            boot_id,
            max_seq,
            min_timestamp_s,
            max_timestamp_s,
        ),
    )
}

/// Extract the date from an SFST remote key.
///
/// Expected shape: `v1/tenants/{tenant_id}/sfst/{YYYY-MM-DD}/{file_id}.sfst`.
/// Returns `None` if the key doesn't match this shape.
pub fn parse_sfst_date(key: &str) -> Option<NaiveDate> {
    let mut parts = key.split('/');
    if parts.next()? != SCHEMA_VERSION {
        return None;
    }
    if parts.next()? != "tenants" {
        return None;
    }
    let _tenant = parts.next()?;
    if parts.next()? != "sfst" {
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

    fn tenant() -> TenantId {
        TenantId::from("tenant1")
    }

    #[test]
    fn sfst_key_and_date_roundtrip() {
        let id = FileId::new(machine(), boot(), 42, 0);
        let key = sfst(&tenant(), sample_date(), id);
        assert!(key.starts_with("v1/tenants/tenant1/sfst/2026-04-17/"));
        assert!(key.ends_with(".sfst"));
        assert_eq!(parse_sfst_date(&key), Some(sample_date()));
    }

    #[test]
    fn sfst_prefix_has_trailing_slash() {
        assert_eq!(
            sfst_prefix(&tenant(), sample_date()),
            "v1/tenants/tenant1/sfst/2026-04-17/",
        );
    }

    #[test]
    fn catalog_key_is_versioned_catalog_date_tenant() {
        let key = catalog(
            sample_date(),
            &tenant(),
            machine(),
            boot(),
            100,
            1_700_000_000,
            1_700_003_600,
        );
        assert!(key.starts_with("v1/catalog/2026-04-17/tenant1/"));
        assert!(key.ends_with(".catalog"));
    }

    #[test]
    fn parse_sfst_date_happy_path() {
        let key = "v1/tenants/tenant1/sfst/2026-04-17/abc123.sfst";
        assert_eq!(parse_sfst_date(key), Some(sample_date()));
    }

    #[test]
    fn parse_sfst_date_rejects_unknown_shapes() {
        assert!(parse_sfst_date("").is_none());
        // Missing v1/ root.
        assert!(parse_sfst_date("tenants/tenant1/sfst/2026-04-17/x").is_none());
        // Wrong version.
        assert!(parse_sfst_date("v2/tenants/tenant1/sfst/2026-04-17/x").is_none());
        // Missing tenants umbrella.
        assert!(parse_sfst_date("v1/tenant1/sfst/2026-04-17/x").is_none());
        // Catalog key shape (not an SFST key).
        assert!(parse_sfst_date("v1/catalog/2026-04-17/tenant1/x").is_none());
        // Truncated.
        assert!(parse_sfst_date("v1/tenants/tenant1/sfst").is_none());
        // Date doesn't parse.
        assert!(parse_sfst_date("v1/tenants/tenant1/sfst/not-a-date/x").is_none());
    }
}
