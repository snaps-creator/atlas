export type ProtectionStatus = {
  secure: boolean | null;
  detail: string;
  checkedAt: number;
  lastConfirmedAt?: number;
};

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
