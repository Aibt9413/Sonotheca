export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${value} ${units[unit]}`;
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

/** bps → "320 kbps" */
export function formatBitrate(bitsPerSecond: number | null | undefined): string {
  if (!bitsPerSecond || bitsPerSecond <= 0) return "—";
  const kbps = bitsPerSecond / 1000;
  return kbps >= 1000 ? `${(kbps / 1000).toFixed(1)} Mbps` : `${Math.round(kbps)} kbps`;
}

export function formatSpeed(bytesPerSecond: number): string {
  return bytesPerSecond > 0 ? `${formatBytes(bytesPerSecond)}/s` : "—";
}

export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || ms <= 0) return "—";
  const totalSeconds = Math.round(ms / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}
