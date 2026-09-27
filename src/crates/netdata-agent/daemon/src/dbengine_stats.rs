//! `/api/v1/dbengine_stats` (`api_v1_dbengine.c`): the size statistics of each tier in use, in C's hand-built JSON.

use std::fmt::Write;

use netdata_agent_rrd::storage::StorageLayout;
use netdata_agent_storage::dbengine::engine::stats::SizeStats;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::server::Reply;

/// `%0.2f` as glibc prints it: Rust's text, but for NaN, which glibc names `nan` or `-nan` by its sign.
fn f2(v: f64) -> String {
    match (v.is_nan(), v.is_sign_negative()) {
        (true, true) => "-nan".into(),
        (true, false) => "nan".into(),
        _ => format!("{v:.2}"),
    }
}

/// `web_client_api_v1_dbengine_stats_for_tier()`.
fn tier(out: &mut String, s: &SizeStats) {
    let members: [(&str, String); 28] = [
        (
            "default_granularity_secs",
            s.default_granularity_secs.to_string(),
        ),
        ("sizeof_datafile", s.sizeof_datafile.to_string()),
        ("sizeof_page_in_cache", s.sizeof_page_in_cache.to_string()),
        ("sizeof_point_data", s.sizeof_point_data.to_string()),
        ("sizeof_page_data", s.sizeof_page_data.to_string()),
        ("pages_per_extent", s.pages_per_extent.to_string()),
        ("datafiles", s.datafiles.to_string()),
        ("extents", s.extents.to_string()),
        ("extents_pages", s.extents_pages.to_string()),
        ("points", s.points.to_string()),
        ("metrics", s.metrics.to_string()),
        ("metrics_pages", s.metrics_pages.to_string()),
        (
            "extents_compressed_bytes",
            s.extents_compressed_bytes.to_string(),
        ),
        (
            "pages_uncompressed_bytes",
            s.pages_uncompressed_bytes.to_string(),
        ),
        ("pages_duration_secs", s.pages_duration_secs.to_string()),
        ("single_point_pages", s.single_point_pages.to_string()),
        ("first_t", s.first_time_s.to_string()),
        ("last_t", s.last_time_s.to_string()),
        (
            "database_retention_secs",
            s.database_retention_secs.to_string(),
        ),
        (
            "average_compression_savings",
            f2(s.average_compression_savings),
        ),
        (
            "average_point_duration_secs",
            f2(s.average_point_duration_secs),
        ),
        (
            "average_metric_retention_secs",
            f2(s.average_metric_retention_secs),
        ),
        (
            "ephemeral_metrics_per_day_percent",
            f2(s.ephemeral_metrics_per_day_percent),
        ),
        ("average_page_size_bytes", f2(s.average_page_size_bytes)),
        (
            "estimated_concurrently_collected_metrics",
            s.estimated_concurrently_collected_metrics.to_string(),
        ),
        (
            "currently_collected_metrics",
            s.currently_collected_metrics.to_string(),
        ),
        ("disk_space", s.disk_space.to_string()),
        ("max_disk_space", s.max_disk_space.to_string()),
    ];
    for (i, (name, value)) in members.iter().enumerate() {
        let _ = write!(
            out,
            "{}\n\t\t\"{name}\":{value}",
            if i == 0 { "" } else { "," }
        );
    }
}

/// `api_v1_dbengine_stats()` once startup completed: 404 without the dbengine (the reply's defaults otherwise kept),
/// else a `tierN` object per tier in use.
pub fn reply(storage: &StorageLayout) -> Reply {
    let Some(engine) = storage.dbengine() else {
        return Reply {
            code: status::NOT_FOUND,
            body: b"dbengine is not enabled".to_vec(),
            ..Reply::default()
        };
    };
    let mut out = String::from("{");
    for (t, td) in engine.tiers.iter().enumerate() {
        let granularity_s = storage.update_every() as u64 * storage.tier_grouping(t);
        let stats = td.size_statistics(granularity_s, engine.main.pages_per_extent());
        let _ = write!(out, "{}\n\t\"tier{t}\": {{", if t == 0 { "" } else { "," });
        tier(&mut out, &stats);
        out.push_str("\n\t}");
    }
    out.push_str("\n}");
    Reply {
        code: status::OK,
        content_type: ContentType::ApplicationJson,
        body: out.into_bytes(),
        ..Reply::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// glibc's `%0.2f`: rounding to even on exact ties, the sign of a negative zero, infinities, NaN by its sign.
    #[test]
    fn floats_print_as_glibc() {
        let zero = std::hint::black_box(0.0f64);
        for (v, want) in [
            (0.125, "0.12"),
            (0.375, "0.38"),
            (2.675, "2.67"),
            (-0.001, "-0.00"),
            (-0.0, "-0.00"),
            (1e20, "100000000000000000000.00"),
            (-837.5, "-837.50"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (zero / zero, "-nan"),
            (-(zero / zero), "nan"),
        ] {
            assert_eq!(f2(v), want, "{v}");
        }
    }

    /// A tier's members in C's order and layout.
    #[test]
    fn a_tier_prints_as_c() {
        let s = SizeStats {
            default_granularity_secs: 60,
            sizeof_datafile: 408,
            sizeof_point_data: 16,
            sizeof_page_data: 2048,
            pages_per_extent: 109,
            datafiles: 2,
            pages_duration_secs: -3,
            first_time_s: -1,
            ephemeral_metrics_per_day_percent: f64::INFINITY,
            average_page_size_bytes: 1.0 / 3.0,
            max_disk_space: 26214400,
            ..SizeStats::default()
        };
        let mut out = String::new();
        tier(&mut out, &s);
        let want = "\n\t\t\"default_granularity_secs\":60,\n\t\t\"sizeof_datafile\":408,\
                    \n\t\t\"sizeof_page_in_cache\":0,\n\t\t\"sizeof_point_data\":16,\
                    \n\t\t\"sizeof_page_data\":2048,\n\t\t\"pages_per_extent\":109,\n\t\t\"datafiles\":2,\
                    \n\t\t\"extents\":0,\n\t\t\"extents_pages\":0,\n\t\t\"points\":0,\n\t\t\"metrics\":0,\
                    \n\t\t\"metrics_pages\":0,\n\t\t\"extents_compressed_bytes\":0,\
                    \n\t\t\"pages_uncompressed_bytes\":0,\n\t\t\"pages_duration_secs\":-3,\
                    \n\t\t\"single_point_pages\":0,\n\t\t\"first_t\":-1,\n\t\t\"last_t\":0,\
                    \n\t\t\"database_retention_secs\":0,\n\t\t\"average_compression_savings\":0.00,\
                    \n\t\t\"average_point_duration_secs\":0.00,\n\t\t\"average_metric_retention_secs\":0.00,\
                    \n\t\t\"ephemeral_metrics_per_day_percent\":inf,\n\t\t\"average_page_size_bytes\":0.33,\
                    \n\t\t\"estimated_concurrently_collected_metrics\":0,\n\t\t\"currently_collected_metrics\":0,\
                    \n\t\t\"disk_space\":0,\n\t\t\"max_disk_space\":26214400";
        assert_eq!(out, want);
    }
}
