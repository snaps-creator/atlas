# Review scope

Main:365b91279f5b7c1a9fd49fd49ab42f1b5f986c69. Candidate base:49dfcde7666a58847c48c65a8a29570174316710. Branch:release/atlas-2.5.1. Один main PR включает изменения candidate2.4.3 и исправления2.5.1; не создаются отдельные PR с пересекающимися архитектурными owners.

Reviewer priorities: identity migration/selection, intent durability before connection, one recovery owner, bounded queue/admission, credential compensation, offline hive ownership, authenticated installer source SHA и ручная публикация. Сравнивать runtime behavior, а не только UI.

Старые installer binaries/parts удалены только из текущего tree. Git history и рабочая исходная ветка не переписываются. В PR не включаются local logs,build outputs,credential fixtures или установщик. Все версии продукта согласованы2.5.1.

PR URL,final SHA,CI conclusions,conflict/branch-protection status уточняются после создания и проверок. До проверки этих фактов merge readiness не объявляется.
