# Review scope

Main:365b91279f5b7c1a9fd49fd49ab42f1b5f986c69. Candidate base:49dfcde7666a58847c48c65a8a29570174316710. Branch:release/atlas-2.5.1. Один main PR включает изменения candidate2.4.3 и исправления2.5.1; не создаются отдельные PR с пересекающимися архитектурными owners.

Reviewer priorities: identity migration/selection, intent durability before connection, one recovery owner, bounded queue/admission, credential compensation, offline hive ownership, authenticated installer source SHA и ручная публикация. Сравнивать runtime behavior, а не только UI.

Старые installer binaries/parts удалены только из текущего tree. Git history и рабочая исходная ветка не переписываются. В PR не включаются local logs,build outputs,credential fixtures или установщик. Все версии продукта согласованы2.5.1.

PR: https://github.com/snaps-creator/atlas/pull/60 (draft до финальных checks). GitHub сообщает mergeable=true, конфликтов нет; ruleset main требует PR, запрет удаления/non-fast-forward и разрешение review threads. Набор именованных required checks пуст, но все applicable CI остаются обязательными для этой задачи. Финальный head/build ID фиксируется динамически в CI artifact build.json и authenticated manifest, а не самоссылкой внутри этого документа.

Первый полный CI: https://github.com/snaps-creator/atlas/actions/runs/38073396670 — PASS на6da9c05 (245 library,50 maintenance,65 updater,отдельный restricted-process). Это не финальный head acceptance. Текущие проверки: https://github.com/snaps-creator/atlas/pull/60/checks.
