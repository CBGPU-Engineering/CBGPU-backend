// Ported verbatim from src/app/api/news/route.ts — Google News RSS proxy,
// no secrets involved.
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

interface NewsItem {
  title: string;
  link: string;
  pubDate: string;
  source: string;
}

function extractCDATA(tag: string, xml: string): string {
  return (
    xml.match(new RegExp(`<${tag}><\\!\\[CDATA\\[([\\s\\S]*?)\\]\\]><\\/${tag}>`))?.[1] ||
    xml.match(new RegExp(`<${tag}>([\\s\\S]*?)<\\/${tag}>`))?.[1] ||
    ''
  ).trim();
}

function parseRSS(xml: string): NewsItem[] {
  const items: NewsItem[] = [];
  const re = /<item>([\s\S]*?)<\/item>/g;
  let m;
  while ((m = re.exec(xml)) !== null) {
    const block = m[1];
    const rawTitle = extractCDATA('title', block);
    const title = rawTitle.replace(/\s+[-–]\s+[^-–]{1,60}$/, '').trim() || rawTitle;
    const link = extractCDATA('link', block) || block.match(/<link\/>(.*?)</)?.[1]?.trim() || '';
    const pubDate = extractCDATA('pubDate', block);
    const source = extractCDATA('source', block) || rawTitle.match(/\s+[-–]\s+([^-–]{1,60})$/)?.[1]?.trim() || '';
    if (title && link) items.push({ title, link, pubDate, source });
  }
  return items;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const url = new URL(req.url);
  const q = url.searchParams.get('q') || 'artificial intelligence';
  const hl = url.searchParams.get('hl') || 'en-US';

  try {
    const rssUrl = `https://news.google.com/rss/search?q=${encodeURIComponent(q)}&hl=${hl}&gl=US&ceid=US:en&num=50`;
    const res = await fetch(rssUrl, {
      headers: { 'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36', Accept: 'application/rss+xml, application/xml, text/xml, */*' },
    });
    if (!res.ok) throw new Error(`RSS ${res.status}`);
    const xml = await res.text();
    return Response.json({ items: parseRSS(xml) }, { headers: corsHeaders });
  } catch (err) {
    return Response.json({ items: [], error: String(err) }, { status: 500, headers: corsHeaders });
  }
});
