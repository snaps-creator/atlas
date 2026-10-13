# Regression review

Совместно проверены source identity/reconciliation, schema3 migration, encrypted backups, missing credential rollback refusal, runtime intent, cancellation/coordinator, fair reserve queue, default no optimization, latency local fixtures, broker admission/path selection и bounded history. Frontend production bundle и native compilation проходят.

Существующие архитектурные owners сохранены: desktop intent/startup coordinator, SYSTEM service HealthState/Recovery, durable updater transaction. Дополнительного Windows network service или параллельного reconnect loop не вводилось. CEF остаётся runtime приложения, вспомогательный maintenance/updater независим от него.

Installation acceptance расширен тремя baseline jobs: historical legacy release, published2.4.2 и freshly signed actual2.4.3 source49dfcde. Старый Git-bundled payload больше не используется. Candidate signatures/receipts проверяются до реальной установки. Fresh/reinstall/rollback/uninstall используют существующий guarded script на disposable runner.

Без installed CI нельзя объявить отсутствие всех регрессий. В частности реальная смена сети, загрузка offline hives разных Windows users, hidden CEF performance и тесты после reboot не выполнялись локально. Точные CI результаты и conflicts проверяются после отправки ветки.
