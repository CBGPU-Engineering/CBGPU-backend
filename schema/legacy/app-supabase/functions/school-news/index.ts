// Ported verbatim from src/app/api/school-news/route.ts.
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const FEEDS: Record<string, string> = {
  notice: 'https://school.cbe.go.kr/rss/cbs-h/M010401',
  letter: 'https://school.cbe.go.kr/rss/cbs-h/M010403',
  schedule: 'https://school.cbe.go.kr/rss/cbs-h/M010407',
};

function extract(tag: string, xml: string): string {
  return (
    xml.match(new RegExp(`<${tag}><\\!\\[CDATA\\[([\\s\\S]*?)\\]\\]><\\/${tag}>`))?.[1] ||
    xml.match(new RegExp(`<${tag}>([\\s\\S]*?)<\\/${tag}>`))?.[1] ||
    ''
  ).trim();
}

function parseRSS(xml: string) {
  const items: { title: string; link: string; pubDate: string; author: string }[] = [];
  const re = /<item>([\s\S]*?)<\/item>/g;
  let m;
  while ((m = re.exec(xml)) !== null) {
    const b = m[1];
    const title = extract('title', b);
    const link = extract('link', b) || extract('guid', b);
    const pubDate = extract('pubDate', b);
    const author = extract('author', b);
    if (title) items.push({ title, link, pubDate, author });
  }
  return items;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const url = new URL(req.url);
  const type = url.searchParams.get('type') || 'notice';
  const feedUrl = FEEDS[type] ?? FEEDS.notice;

  try {
    const res = await fetch(feedUrl, { headers: { 'User-Agent': 'Mozilla/5.0 (compatible; CBGPU/1.0)' } });
    if (!res.ok) throw new Error(`${res.status}`);
    const xml = await res.text();
    return Response.json({ items: parseRSS(xml) }, { headers: corsHeaders });
  } catch (e) {
    return Response.json({ items: [], error: String(e) }, { status: 500, headers: corsHeaders });
  }
});
