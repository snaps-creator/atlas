# Security review

Сохранены DPAPI, Windows Credential Manager, редактирование секретов диагностики, signed manifest/minisign checks, byte/hash/size checks, path/receipt checks, SYSTEM service ACL/image validation, независимый cleanup observer и original-user restricted token handoff. Ключи подписей не выводились и не добавлялись в Git.

Новое: credential journal хранит только source IDs; новый source не перезаписывает чужой credential. Неавторизованный IPC не снимает guard и не заканчивает live session; retry rejects не превращаются в бесконечный admission window. Source errors не вытесняют backup. Offline uninstall использует registered ProfileList paths, не USERPROFILE elevated пользователя; reparse points отвергаются, hive unload выполняется RAII, ownership команды проверяется точно. Locked/unreadable hive даёт incomplete error до удаления payload.

Установщик в Git/LFS запрещён distribution regression contract. CI signing разрешён только same-repository PR; fork secrets не раскрываются. Public package дополнительно сопровождается SHA256SUMS. Publish требует main, точного build SHA, workflow_dispatch и отдельного подтверждения; draft остаётся private при сбое uploads. Подпись обновления не выдаётся за Authenticode.

Оставшиеся проверки: real offline hive/logon race, malformed-client stress на installed service, ограничение очередей/CEF workload, полный final-SHA isolated acceptance. UUID/session/build timing не является обещанием полной анонимности: отчёт всё ещё содержит локальные адреса/имена адаптеров.
