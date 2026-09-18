use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub groups: Vec<RuleGroup>,
    pub subscriptions: Vec<Subscription>,
    pub selected: String,
    pub default_route: Route,
    pub mode: String,
    pub dns: Dns,
    pub startup: Startup,
    pub theme: String,
    pub was_connected: bool,
    pub favorites: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
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
            theme: "system".into(),
            was_connected: false,
            favorites: vec![],
        }
    }
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
