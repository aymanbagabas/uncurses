import test from "node:test";
import assert from "node:assert/strict";
import { searchPages } from "../assets/js/search.js";

const pages = [
  { title: "Keyboard input", section: "Guides", url: "/docs/guides/keyboard-input/", text: "Read a key and handle keyboard events." },
  { title: "Events", section: "Concepts", url: "/docs/concepts/events/", text: "Keyboard input, mouse events, and resize events become typed data." },
  { title: "Cells", section: "Concepts", url: "/docs/concepts/cells/", text: "A cell holds a grapheme, style, and width." },
];

test("title matches rank ahead of body matches, regardless of case", () => {
  assert.deepEqual(searchPages(pages, "KEYBOARD").map((page) => page.title), ["Keyboard input", "Events"]);
});

test("all query terms must match, across title, section, and body", () => {
  assert.deepEqual(searchPages(pages, "concepts mouse").map((page) => page.title), ["Events"]);
  assert.deepEqual(searchPages(pages, "keyboard grapheme"), []);
});

test("empty, whitespace, and missing terms return no results", () => {
  for (const query of ["", "  \n ", "missing"]) assert.deepEqual(searchPages(pages, query), []);
});

test("snippets include the matching body text and results stay bounded", () => {
  const text = `${"prefix ".repeat(30)}grapheme ${"suffix ".repeat(30)}`;
  const many = Array.from({ length: 30 }, (_, index) => ({ title: `Page ${index}`, section: "Concepts", url: `/page-${index}/`, text }));
  const results = searchPages(many, "grapheme");
  assert.equal(results.length, 20);
  assert.match(results[0].snippet, /grapheme/);
  assert.ok(results[0].snippet.startsWith("…"));
  assert.ok(results[0].snippet.endsWith("…"));
});
