use crate::{model::*, rules};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Document {
    version: u8,
    default_route: String,
    rules: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Entry {
    #[serde(skip_serializing_if = "Option::is_none")]
    process: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain_suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain_keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip_cidr: Option<String>,
    route: String,
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    enabled: bool,
    #[serde(default, skip_serializing_if = "is_no")]
    no_resolve: bool,
}
fn yes() -> bool {
    true
}
fn is_yes(v: &bool) -> bool {
    *v
}
fn is_no(v: &bool) -> bool {
    !*v
}
fn route(s: &str) -> Result<Route, String> {
    match s.to_lowercase().as_str() {
        "proxy" | "vpn" => Ok(Route::Proxy),
        "direct" => Ok(Route::Direct),
        "block" => Ok(Route::Block),
        _ => Err(format!("Неизвестный маршрут: {s}")),
    }
}
fn route_text(r: &Route) -> String {
    match r {
        Route::Proxy => "proxy",
        Route::Direct => "direct",
        Route::Block => "block",
    }
    .into()
}
pub fn parse(text: &str) -> Result<rules::Import, String> {
    if text.len() > 4 * 1024 * 1024 {
        return Err("Файл правил превышает 4 МБ".into());
    }
    let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(rules::yaml_error)?;
    if value.get("version").is_none() {
        return rules::import(text);
    }
    let doc: Document = serde_yaml::from_str(text).map_err(rules::yaml_error)?;
    if doc.version != 1 {
        return Err("Неподдерживаемая версия правил".into());
    }
    let default_route = Some(route(&doc.default_route)?);
    let mut groups = vec![];
    for e in doc.rules {
        let fields = [
            e.process.is_some(),
            e.domain.is_some(),
            e.domain_suffix.is_some(),
            e.domain_keyword.is_some(),
            e.ip_cidr.is_some(),
        ]
        .into_iter()
        .filter(|v| *v)
        .count();
        let (kind, value) = if e.process.is_some() && e.domain.is_some() && fields == 2 {
            (
                "PROCESS-DOMAIN",
                format!("{}|{}", e.process.unwrap(), e.domain.unwrap()),
            )
        } else if fields != 1 {
            return Err("Укажите один объект правила или пару process + domain".into());
        } else if let Some(v) = e.process {
            ("PROCESS-NAME", v)
        } else if let Some(v) = e.domain {
            ("DOMAIN", v)
        } else if let Some(v) = e.domain_suffix {
            ("DOMAIN-SUFFIX", v)
        } else if let Some(v) = e.domain_keyword {
            ("DOMAIN-KEYWORD", v)
        } else {
            let v = e.ip_cidr.unwrap();
            (
                if v.contains(':') {
                    "IP-CIDR6"
                } else {
                    "IP-CIDR"
                },
                v,
            )
        };
        let r = rules::normalize(&Rule {
            kind: kind.into(),
            value,
            no_resolve: e.no_resolve,
        })?;
        groups.push(RuleGroup {
            id: uuid::Uuid::new_v4().to_string(),
            name: r.value.clone(),
            description: String::new(),
            enabled: e.enabled,
            route: route(&e.route)?,
            rules: vec![r],
        });
    }
    Ok(rules::Import {
        groups,
        default_route,
        warnings: vec![],
    })
}
pub fn export(settings: &Settings) -> Result<String, String> {
    let mut entries = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for g in &settings.groups {
        for raw in &g.rules {
            let r = rules::normalize(raw)?;
            let identity = format!(
                "{}:{}:{}:{}",
                rules::key(&r)?,
                route_text(&g.route),
                g.enabled,
                r.no_resolve
            );
            if !seen.insert(identity) {
                continue;
            }
            let mut e = Entry {
                process: None,
                domain: None,
                domain_suffix: None,
                domain_keyword: None,
                ip_cidr: None,
                route: route_text(&g.route),
                enabled: g.enabled,
                no_resolve: r.no_resolve,
            };
            match r.kind.as_str() {
                "PROCESS-NAME" => e.process = Some(r.value),
                "DOMAIN" => e.domain = Some(r.value),
                "DOMAIN-SUFFIX" => e.domain_suffix = Some(r.value),
                "DOMAIN-KEYWORD" => e.domain_keyword = Some(r.value),
                "PROCESS-DOMAIN" => {
                    let (p, d) = r.value.split_once('|').unwrap();
                    e.process = Some(p.into());
                    e.domain = Some(d.into());
                }
                _ => e.ip_cidr = Some(r.value),
            }
            entries.push(e);
        }
    }
    serde_yaml::to_string(&Document {
        version: 1,
        default_route: route_text(&settings.default_route),
        rules: entries,
    })
    .map_err(|e| e.to_string())
}
#[derive(Serialize)]
pub struct Conflict {
    key: String,
    value: String,
    routes: Vec<Route>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub groups: Vec<RuleGroup>,
    pub default_route: Option<Route>,
    pub added: usize,
    pub updated: usize,
    pub duplicates: usize,
    pub conflicts: Vec<Conflict>,
    pub warnings: Vec<String>,
}
pub fn preview(
    existing: &[RuleGroup],
    incoming: rules::Import,
    choices: &BTreeMap<String, Route>,
) -> Result<Preview, String> {
    let mut map: BTreeMap<String, RuleGroup> = BTreeMap::new();
    let mut conflicts: BTreeMap<String, Conflict> = BTreeMap::new();
    let mut added = 0;
    let mut updated = 0;
    let mut duplicates = 0;
    for (is_new, groups) in [(false, existing), (true, incoming.groups.as_slice())] {
        for g in groups {
            for raw in &g.rules {
                let r = rules::normalize(raw)?;
                let key = rules::key(&r)?;
                if let Some(old) = map.get_mut(&key) {
                    if old.route == g.route
                        && old.enabled == g.enabled
                        && old.rules[0].no_resolve == r.no_resolve
                    {
                        if is_new {
                            duplicates += 1;
                        }
                    } else if old.route != g.route || old.rules[0].no_resolve != r.no_resolve {
                        let c = conflicts.entry(key.clone()).or_insert(Conflict {
                            key: key.clone(),
                            value: r.value.clone(),
                            routes: vec![old.route.clone()],
                        });
                        if !c.routes.contains(&g.route) {
                            c.routes.push(g.route.clone());
                        }
                        if old.rules[0].no_resolve != r.no_resolve {
                            return Err(format!(
                                "Разные параметры no-resolve для {}: исправьте YAML",
                                r.value
                            ));
                        }
                    } else {
                        old.enabled = g.enabled;
                        if is_new {
                            updated += 1;
                        }
                    }
                } else {
                    let mut next = g.clone();
                    next.id = format!("{}:{}", g.id, map.len());
                    next.rules = vec![r];
                    map.insert(key, next);
                    if is_new {
                        added += 1;
                    }
                }
            }
        }
    }
    for (key, choice) in choices {
        if conflicts.remove(key).is_some() {
            if let Some(g) = map.get_mut(key) {
                g.route = choice.clone();
                updated += 1;
            }
        }
    }
    Ok(Preview {
        groups: map.into_values().collect(),
        default_route: incoming.default_route,
        added,
        updated,
        duplicates,
        conflicts: conflicts.into_values().collect(),
        warnings: incoming.warnings,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_round_trip() {
        let mut s = Settings::default();
        s.groups=parse("version: 1\ndefault-route: direct\nrules:\n - process: 'C:\\Apps\\ChatGPT.exe'\n   route: proxy\n - domain-suffix: https://openai.com/path?q=1\n   route: block").unwrap().groups;
        let yaml = export(&s).unwrap();
        assert!(!yaml.contains("C:"));
        assert!(!yaml.contains("subscription"));
        let mut back = s.clone();
        back.groups = parse(&yaml).unwrap().groups;
        assert_eq!(rules::compile(&s).unwrap(), rules::compile(&back).unwrap());
    }
    #[test]
    fn preview_conflict_requires_choice() {
        let a =
            parse("version: 1\ndefault-route: direct\nrules: [{domain: openai.com, route: proxy}]")
                .unwrap();
        let b = parse(
            "version: 1\ndefault-route: direct\nrules: [{domain: openai.com, route: direct}]",
        )
        .unwrap();
        assert_eq!(
            preview(&a.groups, b, &BTreeMap::new())
                .unwrap()
                .conflicts
                .len(),
            1
        );
    }
    #[test]
    fn duplicate_skipped() {
        let a =
            parse("version: 1\ndefault-route: direct\nrules: [{domain: openai.com, route: proxy}]")
                .unwrap();
        let b = a.clone();
        let p = preview(&a.groups, b, &BTreeMap::new()).unwrap();
        assert_eq!(p.duplicates, 1);
        assert_eq!(p.groups.len(), 1);
    }
}
