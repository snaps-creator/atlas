# Atlas 2.5.1 — release readiness

Статус: кандидат, НЕ ОПУБЛИКОВАН. Merge/tag/publish не разрешены текущей задачей. Единый asset после отдельного разрешения: Atlas-Setup-2.5.1.exe; tag v2.5.1; release name Atlas 2.5.1. Официальная загрузка: https://github.com/snaps-creator/atlas/releases/latest (сейчас опубликована2.4.2).

Обязательные gates: frontend,verificationContracts,backend,restrictedProcess,cleanSignedBuild,packageAndSignatures,connectedUpgrade,installedRecovery,publicationPreparation. updater-readiness.json — policy, не кеш успешных тестов. Старые Actions links в policy относятся к2.4.2 и не доказывают2.5.1.

PR должен пройти main branch protection, отсутствие conflicts и все applicable checks на финальном head. Build ID и authenticated manifest привязаны к этому SHA. Релиз после merge строится снова на merged main SHA, проходит ту же reusable матрицу acceptance (legacy/2.4.2/actual2.4.3), полный backend и получает tag именно этого SHA. Publish скачивает тот же подписанный artifact текущего run и не пересобирает EXE после тестов. Автоматическая публикация при pushmain удалена; workflow_dispatch требует отдельного подтверждения.

Release assets: один полный EXE, его .sig,latest.json,update-manifest.json + .sig,SHA256SUMS.txt. Внутренний NSIS payload не распространяется. GitHub Actions artifacts временные; ни EXE,ни части,ни Git LFS не являются публичным каналом загрузки.

Состав: UI,existing SYSTEM service,Mihomo,Xray+geo assets,CEF locales/runtime,maintenance,receipt-bound updater/stable launcher. Проверяется x64 PE и ProductVersion каждого Atlas component, pinned resource hashes,full file inventory,missing/mismatched/corrupted payload rejection.

До фактической signed сборки размер/SHA256 не заявляются. Итоговые evidence и GitHub состояние добавляются после CI; отсутствие результатов означает gate остаётся открытым.

Physical reboot/power loss остаётся NOT_TESTED, с существующим ранее документированным risk acceptance policy. Это не обещание автоматического восстановления после любого повреждения диска.
