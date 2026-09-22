export type ProtectionStatus = {
  secure: boolean | null;
  detail: string;
  checkedAt: number;
  lastConfirmedAt?: number;
};

export function connectionProtection(running: boolean, status?: string): ProtectionStatus | null {
  if (running && status === "Connected") return null;
  const pending = !status || status === "Connecting" || status === "Disconnecting";
  return { secure: pending ? null : false,
    detail: pending ? "Ожидаем завершения подключения. Защита ещё не проверена." : "Atlas не подключён.",
    checkedAt: 0 };
}

export function afterConnectionReady(ready: boolean, inspect: () => void): () => void {
  // A local controller becomes ready before all initial network activity settles.
  const timer = ready ? setTimeout(inspect, 5000) : undefined;
  return () => clearTimeout(timer);
}

export function unavailableProtection(previous: ProtectionStatus, reason: unknown): ProtectionStatus {
  const busy = String(reason).includes("занят");
  return {
    secure: null,
    detail: busy ? "Проверка временно недоступна. Повторяем автоматически." : `Не удалось выполнить проверку: ${String(reason)}`,
    checkedAt: 0,
    lastConfirmedAt: previous.secure === true ? previous.checkedAt : previous.lastConfirmedAt,
  };
}

export function protectionLabel(status: ProtectionStatus): string {
  return status.secure === null ? "Защита не проверена" : status.secure ? "Данные защищены" : "Данные не защищены";
}
