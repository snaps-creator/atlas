//! UI snapshots are optimistic writes, never an authority for repository references.
use crate::model::{repository, Settings};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub revision: u64,
    pub settings: Settings,
}

impl Write {
    pub fn validate_revision(&self, revision: u64) -> Result<(), String> {
        if self.revision != revision {
            return Err("Настройки уже изменились. Обновите страницу и повторите действие.".into());
        }
        Ok(())
    }
    pub fn apply(self, current: &Settings, revision: u64) -> Result<Settings, String> {
        self.validate_revision(revision)?;
        let mut next = self.settings;
        next.subscriptions = current.subscriptions.clone();
        next.selected = current.selected.clone();
        next.selected_node_id = current.selected_node_id.clone();
        next.favorites = current.favorites.clone();
        next.was_connected = current.was_connected;
        next.user_disconnected = current.user_disconnected;
        next.last_window_hidden = current.last_window_hidden;
        Ok(next)
    }
}

pub fn favorite(current: &Settings, id: &str, enabled: bool) -> Result<Settings, String> {
    if !current.servers().iter().any(|node| repository::node_id(node) == Some(id)) {
        return Err("Сервер уже изменился. Обновите список и повторите действие.".into());
    }
    let mut next = current.clone();
    next.favorites.retain(|reference| reference != id);
    if enabled { next.favorites.push(id.into()); }
    Ok(next)
}

/// Secrets are deliberately not kept in configuration backups. Fail before any
/// configuration write instead of presenting an unusable restored subscription.
pub fn require_credentials(settings: &Settings, mut exists: impl FnMut(&str) -> bool) -> Result<(), String> {
    if settings.subscriptions.iter().any(|source| !exists(&source.id)) {
        return Err("Восстановление отменено: секрет одной из подписок удалён из хранилища Windows. Добавьте эту подписку заново с URL или VLESS-ключом. Текущие настройки не изменены.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Subscription, SubscriptionSource};
    use serde_json::json;
    fn fixture() -> Settings {
        let mut s = Settings::default();
        s.subscriptions.push(Subscription { id:"source".into(), name:"fixture".into(), source:SubscriptionSource::Url,
            options:Default::default(),masked_url:String::new(),updated_at:0,error:None,
            servers:vec![json!({"name":"node · source-1111111111111111","type":"vless","server":"example.test","port":443,"sni":"old.example"})] });
        repository::normalize(&mut s);
        s.select_node(repository::node_id(&s.servers()[0]).unwrap()).unwrap();
        s.favorites=vec![s.selected_node_id.clone()]; s
    }
    #[test]
    fn rotated_references_survive_stale_theme_and_startup_snapshots() {
        let old=fixture(); let mut current=old.clone();
        current.subscriptions[0].servers[0]["sni"]=json!("new.example");
        current.subscriptions[0].servers[0]["name"]=json!("node · source-2222222222222222");
        repository::reconcile_references(&old,&mut current);
        assert_ne!(old.selected_node_id,current.selected_node_id);
        for startup in [false,true] {
            let mut stale=old.clone();
            if startup {stale.startup.auto_connect=true;} else {stale.theme="dark".into();}
            assert!(Write { revision:1,settings:stale.clone() }.apply(&current,2).is_err());
            // Even an accepted settings write cannot carry repository references.
            let applied=Write { revision:2,settings:stale }.apply(&current,2).unwrap();
            assert_eq!(applied.selected_node_id,current.selected_node_id);
            assert_eq!(applied.favorites,current.favorites);
        }
        assert!(favorite(&current,&old.selected_node_id,false).is_err());
        assert!(current.select_node(&old.selected_node_id).is_err());
        let next=favorite(&current,&current.selected_node_id,false).unwrap();
        assert!(next.favorites.is_empty()); assert_eq!(next.selected_node_id,current.selected_node_id);
    }
    #[test]
    fn missing_credential_blocks_restore_before_success_can_be_reported() {
        let s=fixture();
        assert!(require_credentials(&s,|_|false).is_err());
        assert!(require_credentials(&s,|id|id=="source").is_ok());
    }
}
