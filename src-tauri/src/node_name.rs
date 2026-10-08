//! Source-scoped logical names shared by repository and runtime selectors.
fn logical_name(name: &str) -> &str {
    let Some((prefix, suffix)) = name.rsplit_once('-') else {
        return name;
    };
    let digest = suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit());
    let legacy = !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit());
    if prefix.contains(" · ") && (digest || legacy) {
        prefix
    } else {
        name
    }
}
pub fn remap_name<'a>(selected: &str, available: &[&'a str]) -> Option<&'a str> {
    if let Some(exact) = available.iter().copied().find(|n| *n == selected) {
        return Some(exact);
    }
    let mut matches = available
        .iter()
        .copied()
        .filter(|n| logical_name(n) == logical_name(selected));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_rotation_suffix_and_ambiguity_contract() {
        assert_eq!(logical_name("server-12"), "server-12");
        assert_eq!(logical_name("server · source-12"), "server · source");
        assert_eq!(
            logical_name("server · source-AbCd0123456789Ef"),
            "server · source"
        );
        assert_eq!(
            logical_name("server · source-nothex"),
            "server · source-nothex"
        );
        let old = "server · source-1111111111111111";
        let rotated = "server · source-2222222222222222";
        assert_eq!(remap_name(old, &[old, rotated]), Some(old));
        assert_eq!(remap_name(old, &[rotated]), Some(rotated));
        assert_eq!(remap_name(old, &[rotated, "server · source-3"]), None);
        assert_eq!(remap_name(old, &["server · other-2222222222222222"]), None);
    }
}
