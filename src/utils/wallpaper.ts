/**
 * 壁纸处理：压缩到本地存储可承受的尺寸，并用 Canvas 提取主色。
 *
 * 主色不是简单平均值（会被大片白墙/夜空拉灰），而是对 48×48 缩略图做
 * 4bit 量化分桶，再按「出现次数 × 饱和度 × 中间明度」评分取最高桶。
 * 得到的 HEX 交给 `stores/theme.ts`，由 material-color-utilities 用 HCT
 * 生成整套 MD3 色板。
 */

const MAX_EDGE = 1600;
const MAX_FILE_BYTES = 24 * 1024 * 1024;
const SAMPLE_EDGE = 48;
const FALLBACK_SEED = "#2BD4B4";

export interface WallpaperPreparation {
  /** 压缩后的 JPEG data URL，直接进 localStorage */
  dataUrl: string;
  /** 从图片提取的主色（#RRGGBB） */
  seed: string;
}

interface Bucket {
  r: number;
  g: number;
  b: number;
  count: number;
  saturation: number;
  lightness: number;
}

export async function prepareWallpaper(file: File): Promise<WallpaperPreparation> {
  if (!file.type.startsWith("image/")) {
    throw new Error("请选择图片文件");
  }
  if (file.size > MAX_FILE_BYTES) {
    throw new Error("图片过大，请选择 24MB 以内的图片");
  }

  const source = await readAsDataUrl(file);
  const image = await loadImage(source);
  if (!image.naturalWidth || !image.naturalHeight) {
    throw new Error("图片尺寸无效");
  }

  const canvas = renderToCanvas(image, MAX_EDGE, true);
  const seed = extractSeed(canvas);
  const dataUrl = canvas.toDataURL("image/jpeg", 0.82);

  return { dataUrl, seed };
}

/** 对已保存的壁纸重新取色（取色面板里的“从当前壁纸取色”） */
export async function extractSeedFromDataUrl(dataUrl: string): Promise<string> {
  if (!/^data:image\//i.test(dataUrl)) {
    throw new Error("壁纸数据无效");
  }

  const image = await loadImage(dataUrl);
  if (!image.naturalWidth || !image.naturalHeight) {
    throw new Error("壁纸尺寸无效");
  }

  return extractSeed(renderToCanvas(image, 256, false));
}

function renderToCanvas(
  image: HTMLImageElement,
  maxEdge: number,
  fillSurface: boolean,
): HTMLCanvasElement {
  const longest = Math.max(image.naturalWidth, image.naturalHeight, 1);
  const scale = Math.min(1, maxEdge / longest);
  const width = Math.max(1, Math.round(image.naturalWidth * scale));
  const height = Math.max(1, Math.round(image.naturalHeight * scale));

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;

  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("当前 WebView 不支持 Canvas");

  if (fillSurface) {
    // 透明 PNG 转 JPEG 会变黑底；先铺当前主题的 surface，视觉上不突兀
    const surface = getComputedStyle(document.documentElement)
      .getPropertyValue("--md-surface")
      .trim();
    ctx.fillStyle = surface || "#0e1514";
    ctx.fillRect(0, 0, width, height);
  }

  ctx.drawImage(image, 0, 0, width, height);
  return canvas;
}

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      if (typeof reader.result === "string") resolve(reader.result);
      else reject(new Error("读取图片失败"));
    };
    reader.onerror = () => reject(new Error("读取图片失败"));
    reader.readAsDataURL(file);
  });
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("图片解码失败"));
    image.src = src;
  });
}

function extractSeed(source: HTMLCanvasElement): string {
  const sample = document.createElement("canvas");
  sample.width = SAMPLE_EDGE;
  sample.height = SAMPLE_EDGE;

  const ctx = sample.getContext("2d", { willReadFrequently: true });
  if (!ctx) return FALLBACK_SEED;

  ctx.drawImage(source, 0, 0, SAMPLE_EDGE, SAMPLE_EDGE);
  const { data } = ctx.getImageData(0, 0, SAMPLE_EDGE, SAMPLE_EDGE);

  const buckets = new Map<number, Bucket>();

  for (let i = 0; i < data.length; i += 4) {
    if (data[i + 3] < 160) continue;

    const r = data[i];
    const g = data[i + 1];
    const b = data[i + 2];
    const hsl = rgbToHsl(r, g, b);

    // 近黑 / 近白当主色会得到一片灰，直接过滤
    if (hsl.l < 0.06 || hsl.l > 0.96) continue;

    const key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4);
    const bucket = buckets.get(key) ?? {
      r: 0,
      g: 0,
      b: 0,
      count: 0,
      saturation: 0,
      lightness: 0,
    };

    bucket.r += r;
    bucket.g += g;
    bucket.b += b;
    bucket.count += 1;
    bucket.saturation += hsl.s;
    bucket.lightness += hsl.l;
    buckets.set(key, bucket);
  }

  let best: Bucket | null = null;
  let bestScore = -1;

  for (const bucket of buckets.values()) {
    const saturation = bucket.saturation / bucket.count;
    const lightness = bucket.lightness / bucket.count;
    // 出现得多、够鲜艳、明度居中：更像是图片的“主题色”
    const chroma = Math.min(1, saturation * 1.15);
    const midLight = 1 - Math.min(1, Math.abs(lightness - 0.56) * 1.7);
    const score = bucket.count * (0.25 + chroma) * (0.35 + midLight);

    if (score > bestScore) {
      bestScore = score;
      best = bucket;
    }
  }

  if (!best || best.count === 0) return FALLBACK_SEED;

  const rgb = {
    r: Math.round(best.r / best.count),
    g: Math.round(best.g / best.count),
    b: Math.round(best.b / best.count),
  };

  const hex = `#${[rgb.r, rgb.g, rgb.b]
    .map((value) => value.toString(16).padStart(2, "0"))
    .join("")}`.toUpperCase();

  return /^#[0-9A-F]{6}$/.test(hex) ? hex : FALLBACK_SEED;
}

function rgbToHsl(
  r: number,
  g: number,
  b: number,
): { h: number; s: number; l: number } {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;

  if (max === min) return { h: 0, s: 0, l };

  const delta = max - min;
  const s = l > 0.5 ? delta / (2 - max - min) : delta / (max + min);

  let h: number;
  if (max === rn) h = (gn - bn) / delta + (gn < bn ? 6 : 0);
  else if (max === gn) h = (bn - rn) / delta + 2;
  else h = (rn - gn) / delta + 4;

  return { h: h / 6, s, l };
}
