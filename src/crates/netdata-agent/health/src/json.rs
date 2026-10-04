//! A prototype as JSON (`health_dyncfg.c` `health_prototype_to_json()`): what DynCfg shows and, without three
//! members, the bytes a rule's hash is made of.

use netdata_agent_query::tables::options_to_json_array;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::prototype::Rule;
use crate::tables::{ACTION_OPTION_NO_CLEAR_NOTIFICATION, ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME, options_remove_overlapping};

/// `string2str()` of a text that may be unset: the empty text.
fn text(value: &Option<Vec<u8>>) -> &[u8] {
    value.as_deref().unwrap_or(b"")
}

/// `health_prototype_rule_to_json_array_member()`.
fn rule_to_json(w: &mut JsonWriter, rule: &Rule, for_hashing: bool) {
    let (am, ac) = (&rule.r#match, &rule.config);

    w.add_array_item_object();
    w.member_add_boolean("enabled", am.enabled);
    w.member_add_string("type", if am.is_template { "template" } else { "instance" });

    w.member_add_object("config");
    if !for_hashing {
        w.member_add_uuid("hash", &ac.hash_id);
        w.member_add_string("source_type", ac.source_type.name());
        w.member_add_string("source", text(&ac.source));
    }

    w.member_add_object("match");
    w.member_add_string("on", text(&am.on));
    w.member_add_string("host_labels", am.host_labels.as_deref().unwrap_or(b"*"));
    w.member_add_string("instance_labels", am.chart_labels.as_deref().unwrap_or(b"*"));
    w.object_close();

    w.member_add_string("summary", text(&ac.summary));
    w.member_add_string("info", text(&ac.info));
    w.member_add_string("type", text(&ac.r#type));
    w.member_add_string("component", text(&ac.component));
    w.member_add_string("classification", text(&ac.classification));

    w.member_add_object("value");
    w.member_add_object("database_lookup");
    w.member_add_int64("after", i64::from(ac.after));
    w.member_add_int64("before", i64::from(ac.before));
    w.member_add_string("time_group", ac.time_group_name());
    w.member_add_string("time_group_condition", ac.time_group_condition.name());
    w.member_add_double("time_group_value", ac.time_group_value);
    w.member_add_string("dims_group", ac.dims_group.name());
    w.member_add_string("data_source", ac.data_source.name());
    options_to_json_array(w, b"options", options_remove_overlapping(ac.options));
    w.member_add_string("dimensions", text(&ac.dimensions));
    w.object_close();
    w.member_add_string("calculation", ac.calculation.as_ref().map_or(&b""[..], |e| e.source()));
    w.member_add_string("units", text(&ac.units));
    // C passes the int as an unsigned 64-bit value
    w.member_add_uint64("update_every", i64::from(ac.update_every) as u64);
    w.object_close();

    w.member_add_object("conditions");
    w.member_add_string("warning_condition", ac.warning.as_ref().map_or(&b""[..], |e| e.source()));
    w.member_add_string("critical_condition", ac.critical.as_ref().map_or(&b""[..], |e| e.source()));
    w.object_close();

    w.member_add_object("action");
    w.member_add_array(Some(b"options"));
    if ac.alert_action_options & ACTION_OPTION_NO_CLEAR_NOTIFICATION != 0 {
        w.add_array_item_string(ACTION_OPTION_NO_CLEAR_NOTIFICATION_NAME);
    }
    w.array_close();
    w.member_add_string("execute", text(&ac.exec));
    w.member_add_string("recipient", text(&ac.recipient));
    w.member_add_object("delay");
    w.member_add_int64("up", i64::from(ac.delay_up_duration));
    w.member_add_int64("down", i64::from(ac.delay_down_duration));
    w.member_add_int64("max", i64::from(ac.delay_max_duration));
    w.member_add_double("multiplier", f64::from(ac.delay_multiplier));
    w.object_close();
    w.member_add_object("repeat");
    w.member_add_boolean("enabled", ac.has_custom_repeat_config);
    w.member_add_uint64("warning", if ac.has_custom_repeat_config { u64::from(ac.warn_repeat_every) } else { 0 });
    w.member_add_uint64("critical", if ac.has_custom_repeat_config { u64::from(ac.crit_repeat_every) } else { 0 });
    w.object_close();
    w.object_close();

    w.object_close();
    w.object_close();
}

/// `health_prototype_to_json()`: the prototype named `name` with its `rules`, minified. With `for_hashing` the
/// hash, the source type and the source are left out, so two rules that differ only in where they come from hash
/// alike.
pub fn prototype_to_json(name: &[u8], rules: &[&Rule], for_hashing: bool) -> Vec<u8> {
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    w.member_add_uint64("format_version", 1);
    w.member_add_string("name", name);
    w.member_add_array(Some(b"rules"));
    for rule in rules {
        rule_to_json(&mut w, rule, for_hashing);
    }
    w.array_close();
    w.finalize();
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use netdata_agent_dyncfg::model::SourceType;

    use super::*;

    fn rule() -> Rule {
        let mut rule = Rule::default();
        rule.r#match.on = Some(b"system.cpu".to_vec());
        rule.config.name = Some(b"a".to_vec());
        rule.config.hash_id = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        rule.config.source_type = SourceType::User;
        rule.config.source = Some(b"line=1,file=/etc/netdata/health.d/a.conf".to_vec());
        rule.config.update_every = 10;
        rule
    }

    /// Outside the hash a rule's `config` starts with its hash, source type and source; nothing else differs.
    #[test]
    fn the_json_shown_has_three_members_the_hashed_one_leaves_out() {
        let rule = rule();
        let hashed = String::from_utf8(prototype_to_json(b"a", &[&rule], true)).unwrap();
        let shown = String::from_utf8(prototype_to_json(b"a", &[&rule], false)).unwrap();
        let extra = "\"hash\":\"12345678-9abc-def0-0123-456789abcdef\",\"source_type\":\"user\",\
                     \"source\":\"line=1,file=/etc/netdata/health.d/a.conf\",";
        let config = "\"config\":{";
        assert_eq!(hashed.matches(config).count(), 1);
        assert_eq!(shown, hashed.replacen(config, &format!("{config}{extra}"), 1));
        assert!(!hashed.contains("\"hash\"") && !hashed.contains("\"source"));
    }

    #[test]
    fn every_rule_of_the_chain_is_an_item_of_rules() {
        let (first, mut second) = (rule(), rule());
        second.r#match.on = Some(b"system.ram".to_vec());
        let one = String::from_utf8(prototype_to_json(b"a", &[&first], true)).unwrap();
        let two = String::from_utf8(prototype_to_json(b"a", &[&first, &second], true)).unwrap();
        let head = "{\"format_version\":1,\"name\":\"a\",\"rules\":[";
        assert!(one.starts_with(head) && one.ends_with("]}"), "{one}");
        let item = &one[head.len()..one.len() - 2];
        assert_eq!(two, format!("{head}{item},{}]}}", item.replace("system.cpu", "system.ram")));
    }
}
