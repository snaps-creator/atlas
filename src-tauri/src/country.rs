use serde_json::Value;
use std::sync::OnceLock;

fn code(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() == 2
        && s.bytes().all(|b| b.is_ascii_alphabetic())
        && include_str!("../resources/country-codes.txt")
            .split_whitespace()
            .any(|v| v.eq_ignore_ascii_case(s)))
    .then(|| s.to_ascii_lowercase())
}
pub fn detect(node: &Value) -> Option<String> {
    for parent in [node, &node["metadata"]] {
        for key in ["country-code", "country_code", "countryCode", "country"] {
            if let Some(c) = parent[key].as_str().and_then(code) {
                return Some(c);
            }
        }
    }
    let name = node["name"].as_str().unwrap_or_default();
    let chars: Vec<char> = name.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|c| ('\u{1f1e6}'..='\u{1f1ff}').contains(c)) {
            return Some(
                pair.iter()
                    .map(|c| char::from_u32(*c as u32 - 0x1f1e6 + 97).unwrap())
                    .collect(),
            );
        }
    }
    let name = name.to_lowercase();
    let known = [
        (
            "nl",
            &["amsterdam", "netherlands", "нидерланды", "амстердам"][..],
        ),
        (
            "de",
            &["frankfurt", "germany", "berlin", "германия", "франкфурт"],
        ),
        ("fi", &["helsinki", "finland", "финляндия", "хельсинки"]),
        (
            "us",
            &["usa", "united states", "america", "new york", "сша"],
        ),
        (
            "gb",
            &["uk", "london", "united kingdom", "британия", "лондон"],
        ),
        ("fr", &["france", "paris", "франция", "париж"]),
        ("se", &["sweden", "stockholm", "швеция"]),
        ("ch", &["switzerland", "zurich", "швейцария"]),
        ("jp", &["japan", "tokyo", "япония"]),
        ("sg", &["singapore", "сингапур"]),
        ("hk", &["hong kong", "hongkong", "гонконг"]),
        ("ru", &["russia", "moscow", "россия", "москва"]),
        ("tr", &["turkey", "istanbul", "турция"]),
        ("ca", &["canada", "toronto", "канада"]),
        ("pl", &["poland", "warsaw", "польша"]),
        ("ee", &["estonia", "tallinn", "эстония"]),
        ("lv", &["latvia", "riga", "латвия"]),
        ("au", &["australia", "sydney", "австралия"]),
    ];
    let tokens: Vec<&str> = name
        .split(|c: char| !c.is_alphabetic())
        .filter(|s| !s.is_empty())
        .collect();
    for (country, aliases) in known {
        if tokens.contains(&country)
            || aliases.iter().any(|alias| {
                if alias.contains(' ') {
                    name.contains(alias)
                } else {
                    tokens.contains(alias)
                }
            })
        {
            return Some(country.into());
        }
    }
    static DB: OnceLock<Option<maxminddb::Reader<&'static [u8]>>> = OnceLock::new();
    let reader = DB
        .get_or_init(|| {
            maxminddb::Reader::from_source(include_bytes!("../resources/country.mmdb").as_slice())
                .ok()
        })
        .as_ref()?;
    let ip = node["server"].as_str()?.parse().ok()?;
    let record = reader.lookup(ip).ok()?;
    let value: Value = record.decode().ok()??;
    value["country"]["iso_code"]
        .as_str()
        .or_else(|| value.as_str())
        .and_then(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn metadata_wins() {
        assert_eq!(
            detect(&json!({"country":"DE","name":"Amsterdam 01"})),
            Some("de".into())
        );
    }
    #[test]
    fn names_and_flags() {
        assert_eq!(detect(&json!({"name":"🇫🇮 Helsinki 01"})), Some("fi".into()));
        assert_eq!(detect(&json!({"name":"Frankfurt 02"})), Some("de".into()));
        assert_eq!(detect(&json!({"name":"unknown"})), None);
    }
    #[test]
    fn local_database() {
        let reader =
            maxminddb::Reader::from_source(include_bytes!("../resources/country.mmdb").as_slice())
                .unwrap();
        let value = reader
            .lookup("5.9.0.1".parse().unwrap())
            .unwrap()
            .decode::<Value>();
        assert!(detect(&json!({"server":"5.9.0.1"})).is_some(), "{value:?}");
    }
}
