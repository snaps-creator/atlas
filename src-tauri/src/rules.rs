use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub fn yaml_error(e: serde_yaml::Error) -> String {
    if let Some(p) = e.location() {
        format!(
            "Некорректный YAML: строка {}, столбец {}. Проверьте отступы, скобки и поля правил.",
            p.line(),
            p.column()
        )
    } else {
        "Некорректный YAML: проверьте поля и типы значений".into()
    }
}

pub fn normalize(rule: &Rule) -> Result<Rule, String> {
    let mut r = rule.clone();
    r.kind = r.kind.trim().to_uppercase();
    r.value = r.value.trim().to_owned();
    match r.kind.as_str() {
        "DOMAIN" | "DOMAIN-SUFFIX" => {
            let input = if r.value.contains("://") {
                r.value.clone()
            } else {
                format!("https://{}", r.value.trim_start_matches("*."))
            };
            let url = url::Url::parse(&input).map_err(|_| "Некорректный адрес сайта")?;
            if !["http", "https"].contains(&url.scheme())
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("Укажите адрес сайта без учётных данных".into());
            }
            r.value = url
                .host_str()
                .ok_or("Нет домена")?
                .trim_end_matches('.')
                .to_lowercase();
        }
        "PROCESS-NAME" => {
            r.value = r
                .value
                .trim_matches('"')
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_lowercase();
        }
        "PROCESS-DOMAIN" => {
            let (process, domain) = r.value.split_once('|').ok_or("Укажите приложение|домен")?;
            let p = normalize(&Rule {
                kind: "PROCESS-NAME".into(),
                value: process.into(),
                no_resolve: false,
            })?;
            let d = normalize(&Rule {
                kind: "DOMAIN".into(),
                value: domain.into(),
                no_resolve: false,
            })?;
            r.value = format!("{}|{}", p.value, d.value);
        }
        "DOMAIN-KEYWORD" => r.value = r.value.to_lowercase(),
        "IP-CIDR" | "IP-CIDR6" => {
            r.value = r
                .value
                .parse::<ipnet::IpNet>()
                .map_err(|_| "Некорректная IP-сеть")?
                .trunc()
                .to_string();
        }
        _ => {}
    }
    validate(&r)?;
    Ok(r)
}
pub fn key(r: &Rule) -> Result<String, String> {
    let r = normalize(r)?;
    Ok(format!("{}:{}", r.kind, r.value))
}
fn specificity(r: &Rule) -> (u8, std::cmp::Reverse<usize>, &str) {
    let rank = match r.kind.as_str() {
        "PROCESS-DOMAIN" => 0,
        "PROCESS-NAME" => 1,
        "DOMAIN" => 2,
        "DOMAIN-SUFFIX" => 3,
        "DOMAIN-KEYWORD" => 4,
        _ => 5,
    };
    let detail = if r.kind.starts_with("IP-CIDR") {
        r.value
            .parse::<ipnet::IpNet>()
            .map(|n| n.prefix_len() as usize)
            .unwrap_or(0)
    } else {
        r.value.len()
    };
    (rank, std::cmp::Reverse(detail), &r.value)
}

pub fn validate(rule: &Rule) -> Result<(), String> {
    if rule.value.is_empty() || rule.value.contains([',', '\n', '\r']) {
        return Err("Правило содержит пустое значение или недопустимый разделитель".into());
    }
    match rule.kind.as_str() {
        "DOMAIN" | "DOMAIN-SUFFIX" => {
            if rule.value.contains([' ', '/', ':', '(', ')', '|']) {
                return Err("Укажите домен без протокола и пути".into());
            }
        }
        "DOMAIN-KEYWORD" => {}
        "PROCESS-NAME" => {
            if rule.value.contains(['/', '\\', ':', '(', ')', '|']) {
                return Err("Укажите имя EXE без пути".into());
            }
        }
        "PROCESS-DOMAIN" => {
            let (p, d) = rule
                .value
                .split_once('|')
                .ok_or("Укажите приложение|домен")?;
            validate(&Rule {
                kind: "PROCESS-NAME".into(),
                value: p.into(),
                no_resolve: false,
            })?;
            validate(&Rule {
                kind: "DOMAIN".into(),
                value: d.into(),
                no_resolve: false,
            })?;
        }
        "IP-CIDR" | "IP-CIDR6" => {
            let network: ipnet::IpNet = rule.value.parse().map_err(|_| "Некорректная IP-сеть")?;
            if network.addr().is_ipv6() != (rule.kind == "IP-CIDR6") {
                return Err("Тип IP-сети не совпадает с правилом".into());
            }
        }
        _ => return Err(format!("Неподдерживаемый тип правила: {}", rule.kind)),
    }
    if rule.no_resolve && !rule.kind.starts_with("IP-CIDR") {
        return Err("no-resolve разрешён только для IP-сетей".into());
    }
    Ok(())
}
pub fn compile(settings: &Settings) -> Result<Vec<String>, String> {
    let mut result = vec![];
    let mut unique: BTreeMap<String, (Rule, Route)> = BTreeMap::new();
    for g in settings.groups.iter().filter(|g| g.enabled) {
        for r in &g.rules {
            let r = normalize(r)?;
            let k = key(&r)?;
            if let Some((old, route)) = unique.get(&k) {
                if *route != g.route || old.no_resolve != r.no_resolve {
                    return Err(format!(
                        "Конфликт правила {}: выберите один маршрут и параметры",
                        r.value
                    ));
                }
            } else {
                unique.insert(k, (r, g.route.clone()));
            }
        }
    }
    let mut entries: Vec<_> = unique.into_values().collect();
    entries.sort_by(|a, b| specificity(&a.0).cmp(&specificity(&b.0)));
    for (r, route) in entries {
        if r.kind == "PROCESS-DOMAIN" {
            let (process, domain) = r.value.split_once('|').unwrap();
            result.push(format!(
                "AND,((PROCESS-NAME,{process}),(DOMAIN,{domain})),{}",
                route.target()
            ));
        } else {
            result.push(format!(
                "{},{},{}{}",
                r.kind,
                r.value,
                route.target(),
                if r.no_resolve { ",no-resolve" } else { "" }
            ));
        }
    }
    result.push(format!("MATCH,{}", settings.default_route.target()));
    Ok(result)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Import {
    pub groups: Vec<RuleGroup>,
    pub default_route: Option<Route>,
    pub warnings: Vec<String>,
}
pub fn import(text: &str) -> Result<Import, String> {
    if text.len() > 4 * 1024 * 1024 {
        return Err("Файл правил превышает 4 МБ".into());
    }
    let doc: serde_yaml::Value = serde_yaml::from_str(text).map_err(yaml_error)?;
    let mut result = Import {
        groups: vec![],
        default_route: None,
        warnings: vec![],
    };
    if doc.get("delete").is_some() {
        result.warnings.push(
            "Секция delete не импортируется: она относится к исходной конфигурации Clash.".into(),
        )
    }
    let mut lines = vec![];
    if let Some(seq) = doc.as_sequence() {
        lines.extend(seq.iter())
    } else {
        for key in ["prepend", "rules", "append"] {
            if let Some(seq) = doc.get(key).and_then(|v| v.as_sequence()) {
                lines.extend(seq.iter())
            }
        }
    }
    if lines.is_empty() {
        return Err("В YAML нет списка rules, prepend или append".into());
    }
    for line in lines {
        let raw = line.as_str().ok_or("Правило должно быть строкой")?;
        let p: Vec<&str> = raw.split(',').map(str::trim).collect();
        let is_match = p.first() == Some(&"MATCH");
        let count = if is_match { 2 } else { 3 };
        if p.len() < count || p.len() > count + 1 {
            return Err("Некорректное число полей правила".into());
        }
        let route = match p[count - 1] {
            "DIRECT" => Route::Direct,
            "REJECT" => Route::Block,
            _ => Route::Proxy,
        };
        if is_match {
            if result
                .default_route
                .as_ref()
                .is_some_and(|old| *old != route)
            {
                return Err("Конфликт маршрута по умолчанию".into());
            }
            result.default_route = Some(route);
            continue;
        }
        let rule = Rule {
            kind: p[0].into(),
            value: p[1].into(),
            no_resolve: p.get(3) == Some(&"no-resolve"),
        };
        validate(&rule)?;
        if p.len() == 4 && !rule.no_resolve {
            return Err("Неизвестный параметр правила".into());
        }
        // Storage groups preserve labels; the compiler determines routing priority.
        if let Some(last) = result.groups.last_mut().filter(|g| g.route == route) {
            last.rules.push(rule)
        } else {
            result.groups.push(RuleGroup {
                id: uuid::Uuid::new_v4().to_string(),
                name: format!("Импорт · {}", result.groups.len() + 1),
                description: String::new(),
                enabled: true,
                route,
                rules: vec![rule],
            })
        }
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn order_independent_specificity_and_duplicates() {
        let mut s = Settings::default();
        s.groups=import("rules: ['DOMAIN-SUFFIX,example.com,DIRECT','PROCESS-NAME,Telegram.exe,VPN','DOMAIN,chat.example.com,VPN','DOMAIN,chat.example.com,VPN']").unwrap().groups;
        let compiled = compile(&s).unwrap();
        s.groups.reverse();
        for g in &mut s.groups {
            g.rules.reverse();
        }
        assert_eq!(compiled, compile(&s).unwrap());
        assert_eq!(compiled.len(), 4);
        assert!(compiled[0].starts_with("PROCESS-NAME"));
        assert!(compiled[1].starts_with("DOMAIN,"));
    }
    #[test]
    fn normalized_conflict_rejected() {
        let mut s = Settings::default();
        s.groups = import("rules: ['DOMAIN,Example.com,VPN','DOMAIN,example.com.,DIRECT']")
            .unwrap()
            .groups;
        assert!(compile(&s).unwrap_err().contains("Конфликт"));
    }
    #[test]
    fn normalizes_sites_and_processes() {
        assert_eq!(
            normalize(&Rule {
                kind: "DOMAIN-SUFFIX".into(),
                value: "https://CHATGPT.com/c/123?test=1".into(),
                no_resolve: false
            })
            .unwrap()
            .value,
            "chatgpt.com"
        );
        assert_eq!(
            normalize(&Rule {
                kind: "PROCESS-NAME".into(),
                value: "C:\\Apps\\Telegram.exe".into(),
                no_resolve: false
            })
            .unwrap()
            .value,
            "telegram.exe"
        );
    }
    #[test]
    fn order_is_preserved() {
        let a=import("rules:\n - DOMAIN,a.test,GLOBAL\n - DOMAIN,b.test,DIRECT\n - DOMAIN,c.test,GLOBAL\n - MATCH,REJECT").unwrap();
        assert_eq!(a.groups.len(), 3);
        let mut s = Settings::default();
        s.groups = a.groups;
        s.default_route = a.default_route.unwrap();
        assert_eq!(
            compile(&s).unwrap(),
            vec![
                "DOMAIN,a.test,ATLAS",
                "DOMAIN,b.test,DIRECT",
                "DOMAIN,c.test,ATLAS",
                "MATCH,REJECT"
            ]
        );
    }
    #[test]
    fn reject_injection() {
        assert!(validate(&Rule {
            kind: "DOMAIN".into(),
            value: "x,DIRECT".into(),
            no_resolve: false
        })
        .is_err())
    }
    #[test]
    fn ipv6() {
        assert!(validate(&Rule {
            kind: "IP-CIDR6".into(),
            value: "2001:db8::/32".into(),
            no_resolve: true
        })
        .is_ok())
    }
    #[test]
    fn match_position_is_irrelevant() {
        let a = import("rules: ['MATCH,DIRECT','DOMAIN,x,GLOBAL']").unwrap();
        let b = import("rules: ['DOMAIN,x,GLOBAL','MATCH,DIRECT']").unwrap();
        let mut sa = Settings::default();
        sa.groups = a.groups;
        let mut sb = Settings::default();
        sb.groups = b.groups;
        assert_eq!(compile(&sa).unwrap(), compile(&sb).unwrap());
    }
}
