//! The query id (`query_target_generate_name()`, `src/database/contexts/query_target.c`): visible as `id` and `name`
//! in v1 jsonwrap and as `id` in v2 with `debug`. Spec §3.9.

use netdata_agent_text::sanitize::{RRD_STRING_ALLOWED_CHARS, text_sanitize};

use crate::request::DataRequest;
use crate::tables::{options, options_to_id_string};

/// `qt->id` holds at most 254 bytes before sanitizing.
const ID_MAX: usize = 254;

fn or_star(v: &Option<Vec<u8>>) -> String {
    v.as_ref().map_or_else(
        || "*".to_string(),
        |v| String::from_utf8_lossy(v).into_owned(),
    )
}

/// What the id names: a chart (v1 `chart=` found), a context query on a host (v1), or a v2/v3 query.
pub enum IdKind<'a> {
    Chart {
        hostname: &'a str,
        chart_name: &'a str,
    },
    Context {
        hostname: Option<&'a str>,
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
        &text_sanitize(&bytes, ID_MAX + 1, &RRD_STRING_ALLOWED_CHARS, true, b"").text,
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{parse_v1, parse_v2};

    #[test]
    fn ids_match_c() {
        let p = parse_v1(b"chart=system.cpu", 1);
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
        let p = parse_v1(
            b"context=a.b&dims=x&points=-1&options=abs&gtime=5&group=countif&group_options=>0",
            1,
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
        let r = parse_v2(b"scope_nodes=n1&tier=0", 2, 1);
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
}
