export function searchPages(pages, query) {
  const terms = query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
  if (!terms.length) return [];

  return pages.map((page) => {
    const title = page.title.toLocaleLowerCase();
    const section = page.section.toLocaleLowerCase();
    const text = page.text.toLocaleLowerCase();
    if (!terms.every((term) => `${title} ${section} ${text}`.includes(term))) return null;
    const score = terms.reduce((sum, term) =>
      sum + (title.includes(term) ? 20 : 0) + (section.includes(term) ? 5 : 0) + (text.includes(term) ? 1 : 0), 0);
    const position = text.indexOf(terms[0]);
    const start = Math.max(0, position - 50);
    const snippet = page.text.slice(start, start + 160).replace(/\s+/g, " ").trim();
    return { ...page, score, snippet: `${start ? "…" : ""}${snippet}${start + 160 < page.text.length ? "…" : ""}` };
  }).filter(Boolean).sort((a, b) => b.score - a.score || a.title.localeCompare(b.title)).slice(0, 20);
}
