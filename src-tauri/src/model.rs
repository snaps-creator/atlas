use serde::{Deserialize, Serialize};
use serde_json::Value;
#[path="node_repository.rs"]
pub(crate) mod repository;

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
    pub selected_node_id: String,
    #[serde(default)]
    pub user_disconnected: bool,
    #[serde(default)]
    pub last_window_hidden: bool,
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
    #[serde(default)]
    pub auto_optimize: bool,
    pub theme: String,
    pub was_connected: bool,
    pub favorites: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            selected_node_id: "AUTO".into(),
            user_disconnected: false,
            last_window_hidden: false,
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
            auto_optimize: false,
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
    pub fn select_node(&mut self, reference: &str) -> Result<(), String> {
        if ["AUTO","FAILOVER"].contains(&reference) {
            self.selected_node_id=reference.into();self.selected=reference.into();return Ok(());
        }
        let nodes=self.servers();
        let node=nodes.iter().find(|n|repository::node_id(n)==Some(reference)||n["name"]==reference)
            .ok_or("Сервер больше не существует")?;
        self.selected=node["name"].as_str().unwrap_or_default().into();
        self.selected_node_id=repository::node_id(node).unwrap_or(&self.selected).into();
        Ok(())
    }
    pub fn reconcile_selection(&mut self) {
        let reference=if self.selected_node_id.is_empty() {self.selected.clone()} else {self.selected_node_id.clone()};
        if self.select_node(&reference).is_err() {self.selected="AUTO".into();self.selected_node_id="AUTO".into();}
        let nodes=self.servers();
        let mut seen=std::collections::HashSet::new();
        self.favorites=self.favorites.iter().filter_map(|f|nodes.iter().find(|n|repository::node_id(n)==Some(f.as_str())||n["name"]==*f)
            .and_then(repository::node_id).map(str::to_owned)).filter(|id|seen.insert(id.clone())).collect();
    }
    pub fn servers(&self) -> Vec<Value> {
        self.subscriptions.iter().flat_map(|s| s.servers.clone()).collect()
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
