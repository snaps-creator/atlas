use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_AUTO_TEST_INTERVAL_SECONDS: u64 = 300;
pub const MIN_AUTO_TEST_INTERVAL_SECONDS: u64 = 30;
pub const MAX_AUTO_TEST_INTERVAL_SECONDS: u64 = 3600;

fn default_auto_test_interval_seconds() -> u64 {
    DEFAULT_AUTO_TEST_INTERVAL_SECONDS
}

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
    pub tun_stack: TunStack,
    #[serde(default)]
    pub routing_mode: RoutingMode,
    pub groups: Vec<RuleGroup>,
    pub subscriptions: Vec<Subscription>,
    pub selected: String,
    pub default_route: Route,
    pub mode: String,
    pub dns: Dns,
    pub startup: Startup,
    #[serde(default = "default_auto_test_interval_seconds")]
    pub auto_test_interval_seconds: u64,
    pub theme: String,
    pub was_connected: bool,
    pub favorites: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            tun_stack: TunStack::Gvisor,
            routing_mode: RoutingMode::Rule,
            groups: vec![],
            subscriptions: vec![],
            selected: "AUTO".into(),
            default_route: Route::Direct,
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
    pub fn servers(&self) -> Vec<Value> {
        self.subscriptions
            .iter()
            .flat_map(|s| s.servers.clone())
            .collect()
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
        let restored: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(
            restored.auto_test_interval_seconds,
            DEFAULT_AUTO_TEST_INTERVAL_SECONDS
        );
    }
}
