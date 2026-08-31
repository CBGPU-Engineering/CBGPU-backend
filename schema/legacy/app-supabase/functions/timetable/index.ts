// Ported from the Next.js app's src/lib/comcigan.ts + src/app/api/timetable/route.ts.
// Same 컴시간알리미 decoding logic (자료481/자료147 diffing, no eval of their
// page JS) — only the runtime plumbing (Buffer, iconv-lite import, fetch)
// changed for Deno. Module-level caches persist per warm Edge Function
// instance, same tradeoff the Next.js version accepted.
import iconv from 'npm:iconv-lite@0.6.3';
import { Buffer } from 'node:buffer';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const ENTRY_URL = 'http://컴시간학생.kr';
const SCHOOL_KEYWORD = '충북과학고';
const SCHOOL_DISPLAY_NAME = '충북과학고등학교';
const MAX_PERIOD = 8;
const UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36';

const META_TTL = 6 * 60 * 60 * 1000;
const DATA_TTL = 5 * 60 * 1000;

interface SiteMeta {
  baseUrl: string;
  extractCode: string;
  scPrefix: string;
  scSuffix: string;
}

type Grid = Record<number, Record<number, { subject: string; teacher: string; changed: boolean; group: string }>>;

// deno-lint-ignore no-explicit-any
type RawData = Record<string, any>;

let metaCache: { meta: SiteMeta; at: number } | null = null;
let schoolCodeCache: number | null = null;
let dataCache: { raw: RawData; at: number } | null = null;

async function fetchBuffer(url: string): Promise<Uint8Array> {
  const res = await fetch(url, { headers: { 'User-Agent': UA }, signal: AbortSignal.timeout(10_000) });
  if (!res.ok) throw new Error(`HTTP ${res.status} (${url})`);
  return new Uint8Array(await res.arrayBuffer());
}

async function fetchEucKr(url: string): Promise<string> {
  return iconv.decode(Buffer.from(await fetchBuffer(url)), 'euc-kr');
}

async function fetchUtf8(url: string): Promise<string> {
  return Buffer.from(await fetchBuffer(url)).toString('utf-8');
}

function hexEncodeEucKr(text: string): string {
  let hex = '';
  for (const byte of iconv.encode(text, 'euc-kr')) hex += '%' + byte.toString(16);
  return hex;
}

async function loadMeta(): Promise<SiteMeta> {
  const entryHtml = await fetchUtf8(ENTRY_URL);
  const frameTag = entryHtml.toLowerCase().replace(/'/g, '"').match(/<frame [^>]*src="[^"]*"[^>]*>/);
  if (!frameTag) throw new Error('frame을 찾을 수 없습니다');
  const hrefMatch = frameTag[0].match(/"(.*)"/);
  if (!hrefMatch) throw new Error('접근 주소를 찾을 수 없습니다');
  const frameUrl = hrefMatch[1];
  const baseUrl = new URL(frameUrl).origin;

  const pageSource = await fetchEucKr(frameUrl);
  const idx = pageSource.toLowerCase().indexOf('school_ra(sc)');
  const idx2 = pageSource.indexOf("sc_data('");
  if (idx === -1 || idx2 === -1) throw new Error('소스에서 식별 코드를 찾을 수 없습니다');

  const chunk1 = pageSource.slice(idx, idx + 50).replace(' ', '');
  const schoolRaMatch = chunk1.match(/url:'.(.*?)'/);
  if (!schoolRaMatch) throw new Error('school_ra 값을 찾을 수 없습니다');

  const chunk2 = pageSource.slice(idx2, idx2 + 30).replace(' ', '');
  const scDataMatch = chunk2.match(/\((.*?)\)/);
  if (!scDataMatch) throw new Error('sc_data 값을 찾을 수 없습니다');
  const scParts = scDataMatch[1].replace(/'/g, '').split(',');

  return { baseUrl, extractCode: schoolRaMatch[1], scPrefix: scParts[0], scSuffix: scParts[2] };
}

async function getMeta(forceRefresh = false): Promise<SiteMeta> {
  if (!forceRefresh && metaCache && Date.now() - metaCache.at < META_TTL) return metaCache.meta;
  const meta = await loadMeta();
  metaCache = { meta, at: Date.now() };
  return meta;
}

async function searchSchool(meta: SiteMeta, keyword: string) {
  const url = meta.baseUrl + meta.extractCode + hexEncodeEucKr(keyword);
  const body = await fetchUtf8(url);
  const jsonStr = body.slice(0, body.lastIndexOf('}') + 1);
  const list = JSON.parse(jsonStr)?.['학교검색'] as [number, string, string, number][] | undefined;
  if (!list?.length) throw new Error('검색된 학교가 없습니다');
  return list.map(([, region, name, code]) => ({ region, name, code }));
}

async function resolveSchoolCode(meta: SiteMeta): Promise<number> {
  if (schoolCodeCache != null) return schoolCodeCache;
  const schools = await searchSchool(meta, SCHOOL_KEYWORD);
  schoolCodeCache = schools[0].code;
  return schoolCodeCache;
}

async function fetchRawData(meta: SiteMeta, schoolCode: number): Promise<RawData> {
  const path = meta.extractCode.split('?')[0];
  const payload = `${meta.scPrefix}${schoolCode}_0_${meta.scSuffix}`;
  const b64 = Buffer.from(payload).toString('base64');
  const url = meta.baseUrl + path + '?' + b64;
  const body = await fetchUtf8(url);
  const jsonStr = body.slice(0, body.lastIndexOf('}') + 1);
  return JSON.parse(jsonStr);
}

async function loadRawData(forceRefresh = false): Promise<RawData> {
  if (!forceRefresh && dataCache && Date.now() - dataCache.at < DATA_TTL) return dataCache.raw;
  try {
    const meta = await getMeta();
    const raw = await fetchRawData(meta, await resolveSchoolCode(meta));
    dataCache = { raw, at: Date.now() };
    return raw;
  } catch {
    metaCache = null;
    schoolCodeCache = null;
    const meta = await getMeta(true);
    const raw = await fetchRawData(meta, await resolveSchoolCode(meta));
    dataCache = { raw, at: Date.now() };
    return raw;
  }
}

function splitDigits(value: number | string): [number, number] {
  const matches = String(value).match(/\d{1,3}(?=(?:\d{3})+(?!\d))|\d{1,3}$/g);
  if (!matches) return [0, 0];
  const nums = matches.reverse().map(Number);
  return [nums[0] ?? 0, nums[1] ?? 0];
}

function splitDaily(dailyRaw: number | string, defaultRaw: number | string, changeNotify: number) {
  const dailyStr = String(dailyRaw);
  if (changeNotify === 1) {
    if (dailyStr.startsWith('>')) {
      const [teacherIdx, subjectRaw] = splitDigits(dailyStr.slice(1));
      return { teacherIdx, subjectRaw, changed: true };
    }
    const [teacherIdx, subjectRaw] = splitDigits(dailyRaw);
    return { teacherIdx, subjectRaw, changed: false };
  }
  const [teacherIdx, subjectRaw] = splitDigits(dailyRaw);
  return { teacherIdx, subjectRaw, changed: dailyStr !== String(defaultRaw) };
}

function groupLetter(subjectRaw: number, separator: number): string {
  if (separator === 100) return '';
  const group = Math.floor(subjectRaw / separator);
  return group >= 1 && group <= 26 ? String.fromCharCode(group + 64) : '';
}

function cleanTeacherName(name?: string): string {
  if (!name) return '';
  if ((name.includes('*') && !name.startsWith('*') && !name.endsWith('*')) || (name.length === 2 && name[1] === '*')) {
    return name;
  }
  return name.replace('*', '');
}

function decodeGrid(raw: RawData, grade: number, cls: number): Grid {
  const separator: number = raw?.['분리'] ?? 100;
  const changeNotify: number = raw?.['변경알림'] ?? 0;
  const defaultData = raw?.['자료481']?.[grade]?.[cls];
  const dailyData = raw?.['자료147']?.[grade]?.[cls];
  const teacherNames: string[] = raw?.['자료446'] ?? [];
  const subjectNames: string[] = raw?.['자료492'] ?? [];
  const grid: Grid = {};
  if (!dailyData) return grid;

  for (let weekday = 1; weekday <= 5; weekday++) {
    const dailyDay = dailyData[weekday];
    if (!dailyDay) continue;
    const defaultDay = defaultData?.[weekday];
    for (let period = 1; period <= MAX_PERIOD; period++) {
      const dailyRaw = dailyDay[period] ?? 0;
      const defaultRaw = defaultDay?.[period] ?? 0;
      const { teacherIdx, subjectRaw, changed } = splitDaily(dailyRaw, defaultRaw, changeNotify);
      if (teacherIdx <= 0) continue;
      const subjectIdx = subjectRaw % separator;
      const subject = subjectNames[subjectIdx];
      if (!subject || typeof subject !== 'string') continue;
      const wd = weekday - 1;
      if (!grid[wd]) grid[wd] = {};
      grid[wd][period] = { subject, teacher: cleanTeacherName(teacherNames[teacherIdx]), changed, group: groupLetter(subjectRaw, separator) };
    }
  }
  return grid;
}

async function getTimetable(grade: number, cls: number, forceRefresh: boolean) {
  const raw = await loadRawData(forceRefresh);
  const classCounts: number[] = raw?.['학급수'] ?? [];
  const maxGrade = Math.max(0, classCounts.length - 1);
  const grades = Array.from({ length: maxGrade }, (_, i) => i + 1);
  const classCount = classCounts[grade] ?? 0;
  const classes = Array.from({ length: classCount }, (_, i) => i + 1);
  return { grid: decodeGrid(raw, grade, cls), classTimes: raw?.['일과시간'] ?? [], grades, classes, schoolName: SCHOOL_DISPLAY_NAME };
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const url = new URL(req.url);
  const grade = parseInt(url.searchParams.get('grade') || '1');
  const cls = parseInt(url.searchParams.get('class') || '1');
  const forceRefresh = url.searchParams.get('refresh') === '1';

  try {
    const result = await getTimetable(grade, cls, forceRefresh);
    return Response.json({ grid: result.grid, times: result.classTimes, grades: result.grades, classes: result.classes, school: result.schoolName }, { headers: corsHeaders });
  } catch (e) {
    return Response.json({ error: (e as Error).message ?? String(e) }, { status: 500, headers: corsHeaders });
  }
});
