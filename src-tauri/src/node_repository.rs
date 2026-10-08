//! Source-scoped node identity. Provider payload stays intact; metadata never
//! enters Mihomo/Xray configuration or a transport fingerprint.
use super::Settings;
use serde_json::{json, Value};
use sha2::{Digest,Sha256};

pub fn runtime_node(node: &Value) -> Value {
    let mut node=node.clone();
    if let Some(fields)=node.as_object_mut() {fields.remove("atlas");fields.remove("extraParams");}
    node
}
pub fn identity(node: &Value) -> String {
    let mut node=runtime_node(node);
    if let Some(fields)=node.as_object_mut() {fields.remove("name");}
    format!("{:x}",Sha256::digest(node.to_string().as_bytes()))
}
pub fn node_id(node: &Value) -> Option<&str> {node["atlas"]["nodeId"].as_str()}

fn logical_name(name: &str) -> &str {
    let Some((prefix,suffix))=name.rsplit_once('-') else {return name};
    let digest=suffix.len()==16 && suffix.bytes().all(|b|b.is_ascii_hexdigit());
    let legacy=!suffix.is_empty() && suffix.bytes().all(|b|b.is_ascii_digit());
    if prefix.contains(" · ") && (digest||legacy) {prefix} else {name}
}
pub fn remap_name<'a>(selected:&str,available:&[&'a str])->Option<&'a str> {
    if let Some(exact)=available.iter().copied().find(|n|*n==selected) {return Some(exact);}
    let mut matches=available.iter().copied().filter(|n|logical_name(n)==logical_name(selected));
    let first=matches.next()?;matches.next().is_none().then_some(first)
}

pub fn normalize(settings:&mut Settings) {
    let mut used=std::collections::HashSet::new();
    for source in &mut settings.subscriptions {
        // The old VLESS mode also accepted HTTPS subscriptions. Their masked
        // origin identifies them without reading or moving the Keyring secret.
        if source.masked_url.starts_with("https://") {source.source=super::SubscriptionSource::Url;}
        let mut seen=std::collections::HashSet::new();
        source.servers.retain_mut(|node| {
            let identity=identity(node);
            if !seen.insert(identity.clone()) {return false;}
            let id=format!("{}:{identity}",source.id);
            let name=node["name"].as_str().unwrap_or("Сервер").to_owned();
            let display=node["atlas"]["displayName"].as_str().unwrap_or(&name).to_owned();
            if !used.insert(name.clone()) {
                let unique=format!("{name} · {id}");used.insert(unique.clone());node["name"]=json!(unique);
            }
            let protocol=node["xray"]["outbounds"][0]["protocol"].as_str().or_else(||node["type"].as_str()).unwrap_or("unknown").to_owned();
            let transport=node["xray"]["outbounds"][0]["streamSettings"]["network"].as_str()
                .or_else(||node["network"].as_str()).unwrap_or("tcp").to_owned();
            node["atlas"]=json!({"sourceId":source.id,"sourceName":source.name,"sourceType":source.source,
                "nodeId":id,"stableIdentity":identity,"protocol":protocol,"transport":transport,"displayName":display});
            true
        });
    }
}

/// Preserve references only inside their original source, with unambiguous
/// reconciliation after parameter rotation. Never merge equal display names.
pub fn reconcile_references(previous:&Settings,next:&mut Settings) {
    normalize(next);
    let old=previous.servers();let nodes=next.servers();
    let resolve=|reference:&str| -> Option<String> {
        if ["AUTO","FAILOVER"].contains(&reference) {return Some(reference.into());}
        let previous=old.iter().find(|n|node_id(n)==Some(reference)||n["name"]==reference)?;
        let source=previous["atlas"]["sourceId"].as_str()?;
        let candidates=nodes.iter().filter(|n|n["atlas"]["sourceId"]==source).collect::<Vec<_>>();
        if let Some(same)=candidates.iter().find(|n|node_id(n)==node_id(previous)) {return node_id(same).map(str::to_owned);}
        let names=candidates.iter().filter_map(|n|n["name"].as_str()).collect::<Vec<_>>();
        let name=remap_name(previous["name"].as_str()?,&names)?;
        candidates.iter().find(|n|n["name"]==name).and_then(|n|node_id(n)).map(str::to_owned)
    };
    if let Some(selected)=resolve(&previous.selected_node_id).or_else(||resolve(&previous.selected)) {
        next.selected_node_id=selected;
    }
    next.favorites=previous.favorites.iter().filter_map(|f|resolve(f)).collect();
    next.reconcile_selection();
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Subscription,SubscriptionSource};
    fn source(id:&str,kind:SubscriptionSource,count:usize)->Subscription {
        Subscription{id:id.into(),source:kind,name:id.into(),masked_url:String::new(),updated_at:17,error:None,options:Default::default(),
            servers:(0..count).map(|i|json!({"name":format!("node-{i}"),"type":"vless","server":"192.0.2.1","port":443+i,"uuid":"fixture"})).collect()}
    }
    #[test]
    fn all_sources_share_one_pool_with_scoped_identity() {
        let mut s=Settings::default();s.subscriptions=vec![source("office",SubscriptionSource::Url,62),source("office2",SubscriptionSource::Url,62),source("paper",SubscriptionSource::Vless,1),source("red",SubscriptionSource::Vless,1)];
        normalize(&mut s);let nodes=s.servers();assert_eq!(nodes.len(),126);
        assert_eq!(nodes.iter().filter_map(node_id).collect::<std::collections::HashSet<_>>().len(),126);
        assert_eq!(nodes[0]["atlas"]["stableIdentity"],nodes[62]["atlas"]["stableIdentity"]);
        assert_ne!(node_id(&nodes[0]),node_id(&nodes[62]));
        let before=serde_json::to_value(&s).unwrap();normalize(&mut s);assert_eq!(serde_json::to_value(&s).unwrap(),before);
    }
    #[test]
    fn removing_one_source_preserves_other_selection_and_favorites() {
        let mut s=Settings::default();s.subscriptions=vec![source("url",SubscriptionSource::Url,2),source("key",SubscriptionSource::Vless,1)];normalize(&mut s);
        s.select_node(node_id(&s.servers()[2]).unwrap()).unwrap();s.favorites=vec![s.selected_node_id.clone()];
        let mut next=s.clone();next.subscriptions.remove(0);reconcile_references(&s,&mut next);
        assert_eq!(next.selected_node_id,s.selected_node_id);assert_eq!(next.favorites,s.favorites);assert_eq!(next.servers().len(),1);
        let mut removed=next.clone();removed.subscriptions.clear();reconcile_references(&next,&mut removed);assert_eq!(removed.selected_node_id,"AUTO");assert!(removed.favorites.is_empty());
    }
    #[test]
    fn update_is_scoped_and_equal_display_names_do_not_merge_different_nodes() {
        let mut s=Settings::default();
        s.subscriptions=vec![source("url",SubscriptionSource::Url,2),source("key",SubscriptionSource::Vless,1)];
        s.subscriptions[0].servers[1]["name"]=json!("node-0");
        normalize(&mut s);
        assert_eq!(s.servers().len(),3);
        assert_ne!(node_id(&s.servers()[0]),node_id(&s.servers()[1]));
        let id=node_id(&s.servers()[2]).unwrap().to_owned();s.select_node(&id).unwrap();s.favorites=vec![id.clone()];
        let untouched=s.subscriptions[1].servers.clone();let mut next=s.clone();
        next.subscriptions[0].servers.reverse();reconcile_references(&s,&mut next);
        assert_eq!(next.subscriptions[1].servers,untouched);assert_eq!(next.selected_node_id,id);assert_eq!(next.favorites,s.favorites);
        let serialized=serde_json::to_vec(&next).unwrap();let mut restarted:Settings=serde_json::from_slice(&serialized).unwrap();
        normalize(&mut restarted);restarted.reconcile_selection();assert_eq!(restarted.selected_node_id,id);
    }
    #[test]
    fn source_metadata_never_enters_generated_config_or_changes_transport() {
        let mut s=Settings::default();s.subscriptions=vec![source("url",SubscriptionSource::Url,1),source("key",SubscriptionSource::Vless,1)];
        normalize(&mut s);let mut renamed=s.clone();renamed.subscriptions[0].name="Renamed source".into();normalize(&mut renamed);
        assert!(crate::config::same_network_config(&s,&renamed));
        let config=crate::config::generate(&s,"fixture").unwrap();
        assert!(!config.contains("stableIdentity"));assert!(!config.contains("sourceId:"));
        let config:serde_yaml::Value=serde_yaml::from_str(&config).unwrap();
        assert_eq!(config["proxies"].as_sequence().unwrap().len(),2);
        for group in config["proxy-groups"].as_sequence().unwrap().iter().filter(|g|g["name"]=="AUTO"||g["name"]=="FAILOVER") {
            assert_eq!(group["proxies"].as_sequence().unwrap().len(),2);
        }
    }
}
