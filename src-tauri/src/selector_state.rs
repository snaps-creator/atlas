use serde_json::Value;

pub(crate) use crate::node_name::remap_name;

pub(super) fn restore_plan(before: &Value, after: &Value) -> Vec<(String, String)> {
    ["AUTO", "FAILOVER"].into_iter().filter_map(|group| {
        let selected = before["proxies"][group]["now"].as_str()?;
        let available = after["proxies"][group]["all"].as_array()?;
        let names=available.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        let target=remap_name(selected,&names)?;
        Some((group.into(), target.into()))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn preserve_nested_selection_after_reload_and_credential_rotation() {
        let before = json!({"proxies":{"AUTO":{"now":"UAE · source-1111111111111111"},"FAILOVER":{"now":"backup"}}});
        let after = json!({"proxies":{"AUTO":{"all":["Amsterdam","UAE · source-2222222222222222"]},"FAILOVER":{"all":["backup"]}}});
        assert_eq!(restore_plan(&before,&after),vec![("AUTO".into(),"UAE · source-2222222222222222".into()),("FAILOVER".into(),"backup".into())]);
    }
    #[test]
    fn never_guess_between_sources_or_ambiguous_nodes() {
        let before=json!({"proxies":{"AUTO":{"now":"UAE · source-1111111111111111"}}});
        for nodes in [json!(["UAE · other-2222222222222222"]),json!(["UAE · source-2222222222222222","UAE · source-3333333333333333"])] {
            assert!(restore_plan(&before,&json!({"proxies":{"AUTO":{"all":nodes}}})).is_empty());
        }
    }
}
