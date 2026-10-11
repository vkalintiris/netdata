//! The query id (`query_target_generate_name()`, `src/database/contexts/query_target.c`): visible as `id` and `name`
//! in v1 jsonwrap and as `id` in v2 with `debug`. Spec §3.9.

use netdata_agent_text::sanitize::{RRD_STRING_ALLOWED_CHARS, text_sanitize};

use crate::request::DataRequest;
use crate::tables::{options, options_to_id_string};

/// `qt->id` holds at most 254 bytes before sanitizing.
const ID_MAX: usize = 254;
/// The sanitizer's buffer, `MAX_QUERY_TARGET_ID_LENGTH + 1`: one more than the cut id, so the hex of a character the
/// cut split still fits.
const SANITIZED_SIZE: usize = 256;

fn or_star(v: &Option<Vec<u8>>) -> String {
    v.as_ref().map_or_else(
        || "*".to_string(),
        |v| String::from_utf8_lossy(v).into_owned(),
    )
}

/// What the id names: a chart (v1 `chart=` found), a context query on a host (v1), one metric (a weights query),
/// or a v2/v3 query.
pub enum IdKind<'a> {
    Chart {
        hostname: &'a str,
        chart_name: &'a str,
    },
    Context {
        hostname: Option<&'a str>,
    },
    Metric {
        hostname: &'a str,
        context: &'a str,
        instance: &'a str,
        dimension: &'a str,
    },
    DataV2,
}

/// `query_target_generate_name()` over the request after the scope promotion.
pub fn generate(req: &DataRequest, kind: IdKind) -> String {
    let group = format!(
        "{}{}",
        req.time_group.name(),
        req.time_group_options
            .as_ref()
            .map(|o| String::from_utf8_lossy(o).into_owned())
            .unwrap_or_default()
    );
    let mut tail = options_to_id_string(req.options);
    if req.resampling_time > 1 {
        tail.push_str(&format!("/resampling:{}", req.resampling_time));
    }
    if req.options & options::SELECTED_TIER != 0 {
        tail.push_str(&format!("/tier:{}", req.tier));
    }
    let window = format!(
        "after:{}/before:{}/points:{}",
        req.after, req.before, req.points
    );
    let id = match kind {
        IdKind::Chart {
            hostname,
            chart_name,
        } => format!(
            "chart://hosts:{hostname}/instance:{chart_name}/dimensions:{}/{window}/group:{group}/options:{tail}",
            or_star(&req.dimensions)
        ),
        IdKind::Context { hostname } => format!(
            "context://hosts:{}/contexts:{}/instances:{}/dimensions:{}/{window}/group:{group}/options:{tail}",
            hostname.map_or_else(|| or_star(&req.nodes), str::to_string),
            or_star(&req.contexts),
            or_star(&req.instances),
            or_star(&req.dimensions)
        ),
        IdKind::Metric { hostname, context, instance, dimension } => format!(
            "metric://hosts:{hostname}/context:{context}/instance:{instance}/dimension:{dimension}/{window}\
             /group:{group}/options:{tail}"
        ),
        IdKind::DataV2 => format!(
            "data_v2://scope_nodes:{}/scope_contexts:{}/scope_instances:{}/scope_labels:{}/scope_dimensions:{}\
             /nodes:{}/contexts:{}/instances:{}/labels:{}/dimensions:{}/{window}/time_group:{group}/options:{tail}",
            or_star(&req.scope_nodes),
            or_star(&req.scope_contexts),
            or_star(&req.scope_instances),
            or_star(&req.scope_labels),
            or_star(&req.scope_dimensions),
            or_star(&req.nodes),
            or_star(&req.contexts),
            or_star(&req.instances),
            or_star(&req.labels),
            or_star(&req.dimensions)
        ),
    };
    let mut bytes = id.into_bytes();
    bytes.truncate(ID_MAX);
    String::from_utf8_lossy(
        &text_sanitize(&bytes, SANITIZED_SIZE, &RRD_STRING_ALLOWED_CHARS, true, b"").text,
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Profile, parse_v1, parse_v2};

    #[test]
    fn ids_match_c() {
        let p = parse_v1(b"chart=system.cpu", &Profile::default());
        assert_eq!(
            generate(
                &p.request,
                IdKind::Chart {
                    hostname: "c1",
                    chart_name: "system.cpu"
                }
            ),
            "chart://hosts:c1/instance:system.cpu/dimensions:*/after:-600/before:0/points:0/group:average/options:"
        );
        let metric = IdKind::Metric { hostname: "c1", context: "system.cpu", instance: "t.cpu", dimension: "user" };
        assert_eq!(
            generate(&p.request, metric),
            "metric://hosts:c1/context:system.cpu/instance:t.cpu/dimension:user/after:-600/before:0/points:0\
             /group:average/options:"
        );
        let p = parse_v1(
            b"context=a.b&dims=x&points=-1&options=abs&gtime=5&group=countif&group_options=>0",
            &Profile::default(),
        );
        assert_eq!(
            generate(
                &p.request,
                IdKind::Context {
                    hostname: Some("h")
                }
            ),
            "context://hosts:h/contexts:a.b/instances:*/dimensions:|x/after:-600/before:0/points:18446744073709551615\
             /group:countif>0/options:absolute/resampling:5"
        );
        let r = parse_v2(b"scope_nodes=n1&tier=0", 2, &Profile::default());
        assert!(
            generate(&r, IdKind::DataV2).starts_with("data_v2://scope_nodes:n1/scope_contexts:*/")
        );
        // A v2 id with every scope unset already passes C's 254 bytes: it is cut mid-word.
        let id = generate(&r, IdKind::DataV2);
        assert_eq!(id.len(), 254, "{id}");
        assert!(
            id.ends_with("options:jsonwrap,selected-tier,jw-anomaly-rates,virtual-poi"),
            "{id}"
        );
    }

    /// The cut at 254 bytes splits a two-byte character: C's sanitizer (`query_target.c:1225-1227`, a buffer of
    /// `MAX_QUERY_TARGET_ID_LENGTH + 1`) still has room for the hex of its first byte, so the id ends in 255 bytes.
    #[test]
    fn a_character_split_by_the_cut_ends_the_id_as_its_hex() {
        let p = parse_v1(b"chart=x", &Profile::default());
        let prefix = "chart://hosts:h/instance:";
        let name = format!("{}\u{e9}b", "a".repeat(ID_MAX - 1 - prefix.len()));
        let id = generate(&p.request, IdKind::Chart { hostname: "h", chart_name: &name });
        assert_eq!(id, format!("{prefix}{}c3", "a".repeat(ID_MAX - 1 - prefix.len())));
        assert_eq!(id.len(), ID_MAX + 1);
    }
}
