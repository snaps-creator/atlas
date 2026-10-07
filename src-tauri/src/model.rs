use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_AUTO_TEST_INTERVAL_SECONDS: u64 = 300;
pub const MIN_AUTO_TEST_INTERVAL_SECONDS: u64 = 30;
pub const MAX_AUTO_TEST_INTERVAL_SECONDS: u64 = 3600;

fn default_auto_test_interval_seconds() -> u64 {
    DEFAULT_AUTO_TEST_INTERVAL_SECONDS
}
fn legacy_rules_semantics_version() -> u8 { 1 }
fn default_auto_search_ping_ms() -> u64 { 150 }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Route {
    Proxy,
    Direct,
    Block,
}
impl Route {
    pub fn target(&self) -> &'static str {
        match self {
            Self::Proxy => "ATLAS",
            Self::Direct => "DIRECT",
            Self::Block => "REJECT",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub kind: String,
    pub value: String,
    #[serde(default)]
    pub no_resolve: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleGroup {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub route: Route,
    pub rules: Vec<Rule>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    #[serde(default)]
    pub source: SubscriptionSource,
    #[serde(default)]
    pub options: crate::subscription_options::Options,
    pub id: String,
    pub name: String,
    pub masked_url: String,
    pub updated_at: u64,
    pub error: Option<String>,
    pub servers: Vec<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dns {
    pub servers: Vec<String>,
    pub ipv6: bool,
    pub fake_ip: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Startup {
    pub launch_with_windows: bool,
    pub auto_connect: bool,
    pub start_in_tray: bool,
    pub delay_seconds: u64,
    pub restore_connection: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub active_source: SubscriptionSource,
    #[serde(default)]
    pub source_selections: std::collections::BTreeMap<SubscriptionSource,String>,
    #[serde(default)]
    pub tun_stack: TunStack,
    #[serde(default)]
    pub routing_mode: RoutingMode,
    pub groups: Vec<RuleGroup>,
    pub subscriptions: Vec<Subscription>,
    pub selected: String,
    pub default_route: Route,
    #[serde(default = "legacy_rules_semantics_version")]
    pub rules_semantics_version: u8,
    pub mode: String,
    pub dns: Dns,
    pub startup: Startup,
    #[serde(default = "default_auto_test_interval_seconds")]
    pub auto_test_interval_seconds: u64,
    #[serde(default = "default_auto_search_ping_ms")]
    pub auto_search_ping_ms: u64,
    pub theme: String,
    pub was_connected: bool,
    pub favorites: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            active_source: SubscriptionSource::Url,
            source_selections: Default::default(),
            tun_stack: TunStack::Gvisor,
            routing_mode: RoutingMode::Rule,
            groups: vec![],
            subscriptions: vec![],
            selected: "AUTO".into(),
            default_route: Route::Direct,
            rules_semantics_version: 2,
            mode: "tun".into(),
            dns: Dns {
                servers: vec![
                    "https://1.1.1.1/dns-query".into(),
                    "https://dns.google/dns-query".into(),
                ],
                ipv6: false,
                fake_ip: true,
            },
            startup: Startup {
                launch_with_windows: false,
                auto_connect: false,
                start_in_tray: false,
                delay_seconds: 3,
                restore_connection: false,
            },
            auto_test_interval_seconds: DEFAULT_AUTO_TEST_INTERVAL_SECONDS,
            auto_search_ping_ms: default_auto_search_ping_ms(),
            theme: "system".into(),
            was_connected: false,
            favorites: vec![],
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    #[default]
    Rule,
    Global,
    Direct,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunStack {
    #[default]
    Gvisor,
    Mixed,
}
impl Settings {
    pub fn active_subscriptions(&self) -> impl Iterator<Item=&Subscription> {
        self.subscriptions.iter().filter(|s|s.source==self.active_source)
    }
    pub fn active_subscriptions_mut(&mut self) -> impl Iterator<Item=&mut Subscription> {
        let source=self.active_source;
        self.subscriptions.iter_mut().filter(move |s|s.source==source)
    }
    pub fn switch_source(&mut self, source: SubscriptionSource) {
        self.source_selections.insert(self.active_source,self.selected.clone());
        self.active_source=source;
        self.selected=self.source_selections.get(&source).cloned().unwrap_or_else(||"AUTO".into());
        self.reconcile_selection();
        self.was_connected=false;
    }
    pub fn reconcile_selection(&mut self) {
        if !["AUTO","FAILOVER"].contains(&self.selected.as_str()) && !self.servers().iter().any(|n|n["name"]==self.selected) {
            self.selected="AUTO".into();
        }
        self.source_selections.insert(self.active_source,self.selected.clone());
    }
    pub fn servers(&self) -> Vec<Value> {
        self.active_subscriptions()
            .flat_map(|s| s.servers.clone())
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all="UPPERCASE")]
pub enum SubscriptionSource {
    #[default]
    Url,
    Vless,
}
impl SubscriptionSource {
    pub fn accepts(self, node: &Value) -> bool {
        match self {
            Self::Url=>true,
            Self::Vless=>node["type"]=="vless" || (node["type"]=="xray" && node["xray"]["outbounds"].as_array().is_some_and(|items|items.iter().any(|o|o["protocol"]=="vless"))),
        }
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_groups_preserve_selection_and_migrate_old_settings() {
        let sub=|source,name:&str| Subscription {source,id:name.into(),name:name.into(),masked_url:String::new(),updated_at:1,error:None,options:Default::default(),
            servers:vec![serde_json::json!({"name":name,"type":"vless","server":"localhost","port":443,"uuid":"id"})]};
        let mut settings=Settings::default();
        settings.subscriptions=vec![sub(SubscriptionSource::Url,"A"),sub(SubscriptionSource::Vless,"B")];
        settings.selected="A".into();
        assert_eq!(settings.servers().len(),1);
        settings.switch_source(SubscriptionSource::Vless);settings.selected="B".into();
        settings.switch_source(SubscriptionSource::Url);assert_eq!(settings.selected,"A");
        settings.switch_source(SubscriptionSource::Vless);assert_eq!(settings.selected,"B");
        let persisted=serde_json::to_vec(&settings).unwrap();
        let mut restored:Settings=serde_json::from_slice(&persisted).unwrap();
        assert_eq!(restored.active_source,SubscriptionSource::Vless);assert_eq!(restored.selected,"B");
        restored.subscriptions.retain(|s|s.source!=SubscriptionSource::Url);
        restored.switch_source(SubscriptionSource::Url);assert!(restored.servers().is_empty());assert_eq!(restored.selected,"AUTO");
        restored.switch_source(SubscriptionSource::Vless);assert_eq!(restored.selected,"B");
        let mut legacy=serde_json::to_value(&settings).unwrap();
        legacy.as_object_mut().unwrap().remove("activeSource");legacy.as_object_mut().unwrap().remove("sourceSelections");
        for sub in legacy["subscriptions"].as_array_mut().unwrap() {sub.as_object_mut().unwrap().remove("source");}
        let legacy:Settings=serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.active_source,SubscriptionSource::Url);assert_eq!(legacy.servers().len(),2);
    }
    #[test]
    fn inactive_subscription_errors_and_changes_do_not_change_network_config() {
        let mut settings=Settings::default();
        let node=serde_json::json!({"name":"active","type":"ss","server":"localhost","port":443,"cipher":"aes-128-gcm","password":"test"});
        settings.subscriptions.push(Subscription {source:SubscriptionSource::Url,id:"url".into(),name:"URL".into(),masked_url:String::new(),updated_at:1,error:None,options:Default::default(),servers:vec![node]});
        let mut next=settings.clone();
        let mut inactive=next.subscriptions[0].clone();inactive.source=SubscriptionSource::Vless;inactive.id="other".into();
        inactive.error=Some("Download failed".into());inactive.servers=vec![serde_json::json!({"type":"xray","name":"invalid inactive"})];
        next.subscriptions.push(inactive);
        assert!(crate::config::same_network_config(&settings,&next));
        next.switch_source(SubscriptionSource::Vless);
        assert!(!crate::config::same_network_config(&settings,&next));
        assert_eq!(next.servers()[0]["name"],"invalid inactive");
    }

    #[test]
    fn stack_settings_migrate_and_validate() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("tunStack");
        assert_eq!(serde_json::from_value::<Settings>(value.clone()).unwrap().tun_stack, TunStack::Gvisor);
        value["tunStack"] = serde_json::json!("mixed");
        let mixed = serde_json::from_value::<Settings>(value.clone()).unwrap();
        assert_eq!(mixed.tun_stack, TunStack::Mixed);
        assert_eq!(serde_json::to_value(mixed).unwrap()["tunStack"], "mixed");
        value["tunStack"] = serde_json::json!("unknown");
        assert!(serde_json::from_value::<Settings>(value).is_err());
    }

    #[test]
    fn old_settings_receive_default_auto_test_interval() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("autoTestIntervalSeconds");
        value.as_object_mut().unwrap().remove("autoSearchPingMs");
        let restored: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(restored.auto_search_ping_ms,150);
        let mut changed=restored.clone(); changed.auto_search_ping_ms=220;
        let saved=serde_json::to_value(changed).unwrap();
        assert_eq!(serde_json::from_value::<Settings>(saved).unwrap().auto_search_ping_ms,220);
        assert_eq!(
            restored.auto_test_interval_seconds,
            DEFAULT_AUTO_TEST_INTERVAL_SECONDS
        );
    }
}
