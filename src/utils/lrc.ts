export interface LyricWord {
  time: number;
  text: string;
}

export interface LyricLine {
  time: number;
  text: string;
  /** 内联逐字/增强 LRC 才会填充 */
  words?: LyricWord[];
}

/** 标准 LRC 标签：[mm:ss.xx] / [mm:ss:xx] / [mm:ss] */
const TAG = /\[(\d{1,2}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g;

function toSeconds(min: string, sec: string, frac?: string): number {
  const fraction = frac ? Number(frac.padEnd(3, "0")) / 1000 : 0;
  return Number(min) * 60 + Number(sec) + fraction;
}

/**
 * 支持三种歌词：
 * 1. 标准 LRC：[00:01.00]整行歌词
 * 2. 前缀多时间戳：[00:01][00:02]副歌复用
 * 3. 内联逐字 LRC：[00:00.000]五[00:00.603]月[00:01.206]天
 */
export function parseLrc(raw: string): LyricLine[] {
  if (!raw) return [];

  const lines: LyricLine[] = [];

  for (const row of raw.split(/\r?\n/)) {
    TAG.lastIndex = 0;
    const tags = [...row.matchAll(TAG)];
    if (tags.length === 0) continue;

    const entries: LyricWord[] = tags.map((match, index) => {
      const start = (match.index ?? 0) + match[0].length;
      const end = index + 1 < tags.length ? (tags[index + 1].index ?? row.length) : row.length;
      return {
        time: toSeconds(match[1], match[2], match[3]),
        text: row.slice(start, end),
      };
    });

    const nonEmpty = entries.filter((entry) => entry.text.trim().length > 0);
    if (nonEmpty.length === 0) continue;

    const firstTextIndex = entries.findIndex((entry) => entry.text.trim().length > 0);
    const beforeTextIsEmpty = entries
      .slice(0, firstTextIndex)
      .every((entry) => entry.text.trim().length === 0);

    // 标准 LRC / 前缀多个时间戳：所有时间戳共用最后一段文本
    if (beforeTextIsEmpty && nonEmpty.length === 1) {
      const text = nonEmpty[0].text.trim();
      for (const entry of entries) {
        lines.push({ time: entry.time, text });
      }
      continue;
    }

    // 内联逐字：每个时间戳后跟随一段文本
    const words = nonEmpty.map((entry) => ({
      time: entry.time,
      text: entry.text,
    }));
    const text = words.map((word) => word.text).join("").trim();
    if (!text) continue;

    lines.push({
      time: words[0].time,
      text,
      ...(words.length > 1 ? { words } : {}),
    });
  }

  if (lines.length === 0) {
    // 没有可识别时间戳的纯文本歌词也展示
    return raw
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((text, index) => ({ time: index * 8, text }));
  }

  return lines.sort((a, b) => a.time - b.time);
}

/** 秒 → mm:ss */
export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "00:00";
  const total = Math.floor(seconds);
  const mm = Math.floor(total / 60);
  const ss = total % 60;
  return `${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
}
