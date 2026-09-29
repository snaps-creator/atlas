//! Provider metadata is retained separately from proxy credentials.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
// Subscription providers use this compatibility identifier to choose the Xray
// representation. A provider directive or explicit per-subscription override wins.
pub const DEFAULT_USER_AGENT: &str = "Happ/4.3.0";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all="camelCase")]
pub struct Options {
    pub user_agent: Option<String>,
    pub user_agent_override: Option<String>,
    pub fallback_url: Option<String>,
    pub effective_url: Option<String>,
    pub provider_id: Option<String>,
    pub update_interval_hours: Option<u64>,
    pub parameters: BTreeMap<String,String>,
}

impl Options {
    pub fn effective_user_agent(&self) -> &str {
        self.user_agent_override.as_deref().or(self.user_agent.as_deref())
            .unwrap_or(DEFAULT_USER_AGENT)
    }
    pub fn refresh_interval_seconds(&self) -> u64 {
        self.update_interval_hours
            .map(|hours| hours.clamp(1, 720) * 3600)
            .unwrap_or(1800)
    }
}

pub fn https_url(raw: &str) -> Result<url::Url,String> {
    let u=url::Url::parse(raw).map_err(|_|"Некорректный URL подписки")?;
    if u.scheme()!="https" || u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return Err("Подписка должна использовать HTTPS без userinfo и фрагмента".into());
    }
    Ok(u)
}

pub fn validate_agent(value: &str) -> Result<(),String> {
    if value.is_empty() || value.len()>512 || !value.bytes().all(|b|(32..=126).contains(&b)) {
        return Err("User-Agent должен содержать от 1 до 512 печатных ASCII-символов".into());
    }
    Ok(())
}

/// Body directives are stripped before proxy URI parsing; headers take precedence.
pub fn extract(body: &str, headers: &BTreeMap<String,String>, previous: &Options,
    source: &str) -> Result<(String,Options),String> {
    let mut metadata=BTreeMap::new();
    let mut lines=Vec::new();
    for line in body.lines() {
        if let Some(comment)=line.trim().strip_prefix('#') {
            let comment=comment.trim();
            if let Some(separator)=comment.find(|c: char| c == ':' || c.is_whitespace()) {
                let (key,value)=comment.split_at(separator);
                let value=value.trim_start().strip_prefix(':').unwrap_or(value).trim();
                metadata.insert(key.trim().to_ascii_lowercase(),value.trim().to_owned());
            }
        } else { lines.push(line); }
    }
    metadata.extend(headers.iter().map(|(k,v)|(k.to_ascii_lowercase(),v.clone())));
    let mut next=previous.clone();
    if let Some(id)=metadata.get("providerid").or_else(||metadata.get("provider-id")) {
        if id.trim().is_empty() || id.len()>256 { return Err("Некорректный Provider ID".into()); }
        next.provider_id=Some(id.clone());
    }
    if let Some(agent)=metadata.get("change-user-agent") {
        validate_agent(agent)?; next.user_agent=Some(agent.clone());
    }
    if let Some(hours)=metadata.get("profile-update-interval") {
        let hours=hours.parse::<u64>().map_err(|_|"Некорректный интервал обновления подписки")?;
        if !(1..=720).contains(&hours) { return Err("Интервал обновления должен быть от 1 до 720 часов".into()); }
        next.update_interval_hours=Some(hours);
    }
    if next.provider_id.is_some() {
        if let Some(fallback)=metadata.get("fallback-url") {
            https_url(fallback)?; next.fallback_url=Some(fallback.clone());
        }
        if let Some(new_url)=metadata.get("new-url") {
            https_url(new_url)?; next.effective_url=Some(new_url.clone());
        } else if let Some(domain)=metadata.get("new-domain") {
            let authority=https_url(&format!("https://{domain}"))?;
            if authority.path()!="/" || authority.query().is_some() || authority.port().is_some() {
                return Err("Некорректный новый домен подписки".into());
            }
            let mut u=https_url(source)?;
            u.set_host(authority.host_str()).map_err(|_|"Некорректный новый домен подписки")?;
            next.effective_url=Some(u.to_string());
        }
    }
    // Preserve only documented connection directives, not arbitrary HTTP headers.
    for (key,value) in metadata {
        if key.starts_with("fragmentation-") || key.starts_with("noises-")
            || ["ping-type","check-url-via-proxy","resolve-server-domains","resolve-server-domains-dns"].contains(&key.as_str()) {
            if value.len()>4096 { return Err("Слишком длинный параметр подписки".into()); }
            next.parameters.insert(key,value);
        }
    }
    Ok((lines.join("\n"),next))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_overrides_body_and_domain_change_preserves_secret_path() {
        let headers=BTreeMap::from([("change-user-agent".into(),"Happ/4".into())]);
        let (body,o)=extract("#providerid: vendor\n#change-user-agent: old\n#new-domain: backup.example\n#fallback-url: https://backup.example/key\nvless://node",&headers,&Options::default(),"https://old.example/private?k=x").unwrap();
        assert_eq!(body,"vless://node");
        assert_eq!(o.user_agent.as_deref(),Some("Happ/4"));
        assert_eq!(o.effective_url.as_deref(),Some("https://backup.example/private?k=x"));
        assert_eq!(o.fallback_url.as_deref(),Some("https://backup.example/key"));
    }
    #[test]
    fn invalid_directives_do_not_produce_partial_settings() {
        assert!(extract("#providerid: \n#new-url: https://b.example",&BTreeMap::new(),&Options::default(),"https://a.example").is_err());
        assert!(extract("#change-user-agent: bad\tvalue",&BTreeMap::new(),&Options::default(),"https://a.example").is_err());
        assert!(extract("#providerid: x\n#fallback-url: http://a.example",&BTreeMap::new(),&Options::default(),"https://a.example").is_err());
        assert!(extract("#profile-update-interval: 0",&BTreeMap::new(),&Options::default(),"https://a.example").is_err());
    }
    #[test]
    fn whitespace_directives_preserve_url_colons() {
        let (_, options) = extract(
            "# providerid vendor\n# fallback-url https://backup.example/key?q=a:b\n# new-url : https://new.example/key",
            &BTreeMap::new(), &Options::default(), "https://old.example/key",
        ).unwrap();
        assert_eq!(options.fallback_url.as_deref(), Some("https://backup.example/key?q=a:b"));
        assert_eq!(options.effective_url.as_deref(), Some("https://new.example/key"));
    }
    #[test]
    fn provider_refresh_interval_is_bounded() {
        let mut options = Options::default();
        assert_eq!(options.refresh_interval_seconds(), 1800);
        options.update_interval_hours = Some(2);
        assert_eq!(options.refresh_interval_seconds(), 7200);
        options.update_interval_hours = Some(u64::MAX);
        assert_eq!(options.refresh_interval_seconds(), 720 * 3600);
    }
    #[test]
    fn manual_agent_survives_provider_changes_and_can_be_reset() {
        let previous = Options { user_agent_override: Some("Custom/1".into()), ..Default::default() };
        let (_, mut next) = extract("#change-user-agent: Provider/2", &BTreeMap::new(), &previous,
            "https://a.example/key").unwrap();
        assert_eq!(next.effective_user_agent(), "Custom/1");
        next.user_agent_override = None;
        assert_eq!(next.effective_user_agent(), "Provider/2");
    }
}
