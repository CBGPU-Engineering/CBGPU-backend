// Ported verbatim from the Next.js app's src/app/api/meal/route.ts — pure
// HTML scrape of school.cbe.go.kr, no secrets involved, so the port is a
// straight copy with fetch/Response swapped for their Deno equivalents.
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const MEAL_LIST = 'https://school.cbe.go.kr/cbs-h/M01050705/list';
const HEADERS = {
  'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
  Accept: 'text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8',
  'Accept-Language': 'ko-KR,ko;q=0.9,en;q=0.8',
  Referer: 'https://school.cbe.go.kr/',
};

const MEAL_TYPES = ['조식', '중식', '석식'];

function stripTags(s: string) {
  return s
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/&#\d+;/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function removeAllergens(s: string) {
  return s.replace(/\s*\([0-9.]+\)\s*/g, '').trim();
}

interface MealItem {
  type: string;
  items: string[];
  calories: string;
}

function parseDayHtml(html: string): MealItem[] {
  const meals: MealItem[] = [];
  const dlRegex = /<dl[^>]*>([\s\S]*?)<\/dl>/gi;
  let dlMatch;
  while ((dlMatch = dlRegex.exec(html)) !== null) {
    const dlHtml = dlMatch[1];
    const dtMatch = dlHtml.match(/<dt[^>]*>([\s\S]*?)<\/dt>/i);
    if (!dtMatch) continue;
    const typeText = stripTags(dtMatch[1]).trim();
    if (!MEAL_TYPES.includes(typeText)) continue;
    const ddMatch = dlHtml.match(/<dd[^>]*>([\s\S]*?)<\/dd>/i);
    if (!ddMatch) continue;
    const items: string[] = [];
    const liRegex = /<li[^>]*>([\s\S]*?)<\/li>/gi;
    let liMatch;
    while ((liMatch = liRegex.exec(ddMatch[1])) !== null) {
      const item = removeAllergens(stripTags(liMatch[1]));
      if (item) items.push(item);
    }
    if (items.length > 0) meals.push({ type: typeText, items, calories: '' });
  }
  return meals;
}

function parseMealPage(html: string, dateYmd: string): MealItem[] {
  const linkIdx = html.indexOf(`?ymd=${dateYmd}`);
  if (linkIdx === -1) return [];
  const before = html.slice(0, linkIdx);
  const aStart = before.lastIndexOf('<a ');
  if (aStart === -1) return [];
  const fromA = html.slice(aStart);
  const aEnd = fromA.indexOf('</a>');
  if (aEnd === -1) return [];
  return parseDayHtml(fromA.slice(0, aEnd));
}

function fmtDate(d: Date) {
  return `${d.getFullYear()}${String(d.getMonth() + 1).padStart(2, '0')}${String(d.getDate()).padStart(2, '0')}`;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const url = new URL(req.url);
  const today = fmtDate(new Date());
  const dateParam = url.searchParams.get('date') ?? today;

  try {
    const listUrl = `${MEAL_LIST}?ymd=${dateParam}`;
    const res = await fetch(listUrl, { headers: HEADERS, signal: AbortSignal.timeout(10000) });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const html = await res.text();
    const meals = parseMealPage(html, dateParam);
    return Response.json({ meals, date: dateParam, today, source: listUrl }, { headers: corsHeaders });
  } catch (e) {
    return Response.json({ meals: [], error: (e as Error).message, date: dateParam, today }, { status: 500, headers: corsHeaders });
  }
});
