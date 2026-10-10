# Миграция и сохранность данных

Наследована транзакционная миграция SQLite к schema 3: старые source поля преобразуются в общий список, исходный DPAPI-encrypted payload сохраняется, ссылки на выбранный узел и избранное согласуются. Неизвестная более новая schema отклоняется без перезаписи. Повторное открытие не выполняет повторную деструктивную миграцию.

2.5.1 добавляет таблицу credential_cleanup к schema 3. Это аддитивный журнал UUID источников, без URL/паролей. Перед записью нового WinCred секрета pending marker записывается durable; commit источника удаляет marker в той же SQLite transaction. Удаление источника ставит marker в transaction удаления. На старте reconciliation удаляет лишь секреты отсутствующих источников; ошибки сохраняют marker и сообщаются пользователю. Существующий credential при add не перезаписывается.

WinCred и SQLite не являются одной атомарной транзакцией: применяется durable compensation. Тесты покрывают interrupted add, failed deletion, committed add, source deletion и повторную reconciliation. Реальная авария питания между API Windows и SQLite не воспроизводилась.

Runtime intent, видимость окна и source error сохраняются без создания пользовательского checkpoint. save_visibility меняет только lastWindowHidden и не сбрасывает restore intent. 100 последовательных refresh failures сохраняют прежний configuration backup. Старые backups не очищаются миграцией; rollback проверяет доступность нужных credential и отказывает без изменения состояния при их отсутствии.

Данные и secrets рабочей установки не читались и не менялись для acceptance: использованы временные SQLite/DPAPI и уникальные WinCred fixtures.
