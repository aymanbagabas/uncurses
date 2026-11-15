import { searchPages } from "./search.js";
import { initTheme } from "./theme.js";

initTheme();

document.querySelectorAll("details[data-desktop-open]").forEach((details) => {
  const desktop = matchMedia(details.dataset.desktopOpen);
  const update = () => { details.open = desktop.matches; };
  update();
  desktop.addEventListener("change", update);
});

const dialog = document.querySelector(".search-dialog");
const input = document.querySelector("#search-input");
const status = document.querySelector(".search-status");
const results = document.querySelector(".search-results");
let indexPromise;
let searchRequest = 0;

async function updateSearch() {
  const request = ++searchRequest;
  const query = input.value.trim();
  results.replaceChildren();
  if (!query) {
    status.textContent = "Type to search the documentation.";
    return;
  }
  status.textContent = "Searching the manual...";
  try {
    indexPromise ??= fetch(dialog.dataset.index).then((response) => {
      if (!response.ok) throw new Error(`Search index: HTTP ${response.status}`);
      return response.json();
    }).catch((error) => {
      indexPromise = undefined;
      throw error;
    });
    const pages = await indexPromise;
    if (request !== searchRequest) return;
    const matches = searchPages(pages, query);
    status.textContent = matches.length ? `${matches.length} result${matches.length === 1 ? "" : "s"} for "${query}".` : `No results for "${query}". Try another term.`;
    for (const match of matches) {
      const item = document.createElement("li");
      const link = document.createElement("a");
      link.href = match.url;
      const section = document.createElement("span");
      section.className = "result-section";
      section.textContent = match.section;
      const title = document.createElement("strong");
      title.textContent = match.title;
      const snippet = document.createElement("span");
      snippet.className = "result-snippet";
      snippet.textContent = match.snippet;
      link.append(section, title, snippet);
      item.append(link);
      results.append(item);
    }
  } catch (error) {
    if (request !== searchRequest) return;
    status.textContent = "Search could not load. Try again, or use the documentation navigation.";
    console.error(error);
  }
}

document.querySelectorAll("[data-search-open]").forEach((button) => {
  button.hidden = false;
  button.addEventListener("click", () => {
    dialog.showModal();
    input.focus();
  });
});
document.querySelector("[data-search-close]").addEventListener("click", () => dialog.close());
dialog.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    event.preventDefault();
    dialog.close();
  }
});
dialog.addEventListener("click", (event) => {
  if (event.target !== dialog) return;
  const rect = dialog.getBoundingClientRect();
  if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) dialog.close();
});
input.addEventListener("input", updateSearch);
document.addEventListener("keydown", (event) => {
  if (event.defaultPrevented || event.isComposing || event.altKey || event.shiftKey) return;
  const editing = event.target.closest("input, textarea, select, [contenteditable]");
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k" && !editing && !dialog.open) {
    event.preventDefault();
    dialog.showModal();
    input.focus();
  }
});

document.querySelectorAll("[data-copy], [data-copy-code]").forEach((button) => {
  if (!navigator.clipboard?.writeText) return;
  button.hidden = false;
  const label = button.textContent;
  button.addEventListener("click", async () => {
    const text = button.dataset.copy ?? button.closest(".code-block").querySelector("pre").textContent;
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = "Copied!";
    } catch (error) {
      button.textContent = "Copy failed";
      console.error("Could not copy text.", error);
    }
    setTimeout(() => { button.textContent = label; }, 2000);
  });
});
