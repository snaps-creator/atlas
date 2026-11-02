export type CaptureReport = {
  deviceId: string; at: number; captureId?: string; captureRunning?: boolean;
  captureCompletedAt?: number; captureError?: string; schema?: number;
};
// Capture is sent once. Polling cannot accidentally start a second collection.
export async function collectAndDownload<T extends CaptureReport>(
  fetch: (capture: boolean, statusOnly: boolean) => Promise<T>,
  save: (report: T) => Promise<unknown>,
  active: () => boolean,
  pause: () => Promise<void> = () => new Promise(resolve => setTimeout(resolve, 1500)),
  now: () => number = Date.now,
): Promise<T> {
  const started = now();
  let report = await fetch(true, true);
  const id = report.captureId;
  if (!id) throw new Error("Обновите Atlas на этом ПК: ожидание готового сбора не поддерживается.");
  const device = report.deviceId;
  for (;;) {
    if (!active()) throw new Error("Просмотр закрыт. Автосохранение отменено.");
    if (report.deviceId !== device || report.captureId !== id) throw new Error("Сбор на удалённом ПК изменился. Повторите действие.");
    if (report.captureError) throw new Error(report.captureError);
    if (!report.captureRunning) {
      if (!report.captureCompletedAt) throw new Error("Сбор не завершился; отчёт не сохранён.");
      report = await fetch(false, false);
      if (!active()) throw new Error("Просмотр закрыт. Автосохранение отменено.");
      if (report.deviceId !== device || report.captureId !== id || report.captureRunning || !report.captureCompletedAt || report.captureError)
        throw new Error("Готовый отчёт изменился. Повторите сбор.");
      await save(report);
      return report;
    }
    if (now() - started >= 150000) throw new Error("Сбор не завершился за 150 секунд. Проверьте связь и повторите позже.");
    await pause();
    if (!active()) throw new Error("Просмотр закрыт. Автосохранение отменено.");
    report = await fetch(false, true);
  }
}
