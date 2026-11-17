import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { readFile } from "node:fs/promises";

test("the landing page centers its logo, description, and links, and search works", async ({ page }) => {
  const errors = [];
  const fontRequests = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (request.resourceType() === "font") fontRequests.push(request.url());
  });
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  const heading = page.getByRole("heading", { level: 1 });
  await expect(heading).toHaveAccessibleName("uncurses");
  const logo = page.getByRole("img", { name: "uncurses", exact: true });
  await expect(page.locator('img[alt="uncurses"]')).toHaveCount(1);
  await expect(logo).toBeVisible();
  await expect.poll(() => logo.evaluate((image) => image.naturalWidth)).toBeGreaterThan(0);
  await page.evaluate(() => document.fonts.ready);
  expect(await page.evaluate(() => getComputedStyle(document.body).fontFamily)).toMatch(/^system-ui,/);
  expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe("rgb(255, 255, 255)");
  await expect(page.getByRole("complementary", { name: "Documentation navigation" })).toHaveCount(0);
  await expect(page.locator("main img")).toHaveCount(1);
  const contentBox = await page.locator(".page-shell").boundingBox();
  const logoBox = await logo.boundingBox();
  expect(contentBox.width).toBeLessThanOrEqual(640);
  expect(contentBox.x + contentBox.width / 2).toBeCloseTo(page.viewportSize().width / 2);
  expect(Math.abs(contentBox.y + contentBox.height / 2 - page.viewportSize().height / 2)).toBeLessThan(1);
  expect(logoBox.x + logoBox.width / 2).toBeCloseTo(page.viewportSize().width / 2);
  const navigation = page.getByRole("navigation", { name: "Main navigation" });
  await expect(navigation.getByRole("button", { name: /^Color theme:/ })).toBeVisible();
  await expect(page.getByRole("banner")).toHaveCount(0);
  const footerBox = await page.getByRole("contentinfo").boundingBox();
  expect(footerBox.x + footerBox.width / 2).toBeCloseTo(page.viewportSize().width / 2);
  expect(footerBox.y + footerBox.height).toBeCloseTo(page.viewportSize().height);
  await expect(page.getByRole("contentinfo")).toHaveCSS("text-align", "center");
  await expect(navigation.getByRole("link", { name: "Get started", exact: true })).toHaveAttribute("href", "/docs/getting-started/");
  await expect(navigation.getByRole("link", { name: "Manual", exact: true })).toHaveAttribute("href", "/docs/");
  await expect(navigation.getByRole("link", { name: "API", exact: true })).toHaveAttribute("href", "/api/");
  await expect(navigation.getByRole("link", { name: "Examples", exact: true })).toHaveAttribute("href", /\/examples\/examples$/);
  await page.getByRole("button", { name: "Search documentation" }).click();
  await page.getByRole("searchbox", { name: "Search documentation" }).fill("keyboard");
  await expect(page.locator(".search-results a").first()).toContainText("Keyboard input");
  await page.locator(".search-results a").first().click();
  await expect(page).toHaveURL(/\/docs\/guides\/keyboard-input\//);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Keyboard input");
  expect(fontRequests).toEqual([]);
  expect(errors).toEqual([]);
});

test("the home footer fits a short viewport without extra scrolling", async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 568 });
  await page.goto("/");
  expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBe(page.viewportSize().height);
  const footerBox = await page.getByRole("contentinfo").boundingBox();
  expect(footerBox.y + footerBox.height).toBeCloseTo(page.viewportSize().height);
});

test("search supports the shortcut, empty results, and Escape focus restoration", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Search documentation" }).focus();
  await page.keyboard.press("/");
  await expect(page.getByRole("dialog")).toBeHidden();
  await page.keyboard.press("Control+k");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.getByRole("searchbox").fill("no-such-terminal-term");
  await expect(page.locator(".search-status")).toContainText("No results");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();
  await expect(page.getByRole("button", { name: "Search documentation" })).toBeFocused();
  await page.keyboard.press("Meta+k");
  await expect(page.getByRole("dialog")).toBeVisible();
});

test("the terminal demo types, pauses, and repeats its recorded application output", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-01-01T00:00:00Z") });
  await page.clock.pauseAt(new Date("2026-01-01T00:00:01Z"));
  await page.goto("/");
  const demo = page.getByRole("figure", { name: "Hello, terminal demo" });
  const output = demo.locator("[data-terminal-text]");
  const { frames } = JSON.parse(await demo.locator("[data-terminal-recording]").textContent());
  const typingTime = frames.slice(0, -1).reduce((sum, frame) => sum + frame.duration, 0);
  const holdTime = frames.at(-1).duration;
  await expect(output).toHaveText("");
  await expect(demo.locator(".terminal-cursor")).toHaveCSS("animation-name", "terminal-blink");
  await page.clock.runFor(frames[0].duration + 1);
  await expect(output).toHaveText("H");
  await page.clock.runFor(typingTime - frames[0].duration);
  await expect(output).toHaveText("Hello, terminal!");
  await demo.hover();
  await page.clock.runFor(typingTime + holdTime);
  await expect(output).toHaveText("Hello, terminal!");
  await expect(demo.locator(".terminal-cursor")).toHaveCSS("animation-name", "none");
  await demo.focus();
  await page.mouse.move(0, 0);
  await page.clock.runFor(typingTime + holdTime);
  await expect(output).toHaveText("Hello, terminal!");
  await expect(demo.locator(".terminal-cursor")).toHaveCSS("animation-name", "none");
  await page.getByRole("link", { name: "Get started", exact: true }).focus();
  await expect(demo.locator(".terminal-cursor")).toHaveCSS("animation-name", "terminal-blink");
  await page.clock.runFor(holdTime + 1);
  await expect(output).toHaveText("");
  await page.clock.runFor(typingTime);
  await expect(output).toHaveText("Hello, terminal!");
});

test("the terminal demo stays still when reduced motion is enabled", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.clock.install({ time: new Date("2026-01-01T00:00:00Z") });
  await page.clock.pauseAt(new Date("2026-01-01T00:00:01Z"));
  await page.goto("/");
  const demo = page.getByRole("figure", { name: "Hello, terminal demo" });
  await expect(demo).toHaveAttribute("data-playing", "false");
  await page.clock.runFor(10000);
  await expect(demo.locator("[data-terminal-text]")).toHaveText("Hello, terminal!");
  await expect(demo.locator(".terminal-cursor")).toHaveCSS("animation-name", "none");
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await expect(demo).toHaveAttribute("data-playing", "true");
  await expect(demo.locator("[data-terminal-text]")).toHaveText("");
});

test("search reports an index failure and can retry", async ({ page }) => {
  await page.route("**/search.json", (route) => route.fulfill({ status: 503, body: "Unavailable" }));
  await page.goto("/");
  await page.getByRole("button", { name: "Search documentation" }).click();
  await page.getByRole("searchbox").fill("cells");
  await expect(page.locator(".search-status")).toContainText("Search could not load.");
  await page.unroute("**/search.json");
  await page.getByRole("searchbox").fill("cell");
  await expect(page.locator(".search-results a").first()).toContainText("Cells");
});

test("documentation cards resolve and tutorial pages follow their stated order", async ({ page }) => {
  await page.goto("/docs/getting-started/");
  await page.getByRole("link", { name: "Installation", exact: false }).filter({ has: page.locator(".doc-card-title") }).click();
  await expect(page).toHaveURL(/\/docs\/getting-started\/installation\//);
  await expect(page.locator(".page-pagination")).toContainText("Hello, terminal");
  await page.locator(".page-pagination a").filter({ hasText: "Hello, terminal" }).click();
  await expect(page).toHaveURL(/\/docs\/getting-started\/hello-world\//);
  await expect(page.locator(".page-pagination a").filter({ hasText: "Installation" })).toContainText("PREVIOUS");
  await expect(page.locator(".page-pagination a").filter({ hasText: "The layers" })).toContainText("NEXT");
});

test("documentation overviews link README features to usage documentation", async ({ page }) => {
  const readme = await readFile(new URL("../../../README.md", import.meta.url), "utf8");
  const featureSection = readme.split("\n## Features\n")[1].split("\n## ")[0];
  const featureCount = featureSection.match(/^- /gm).length;
  for (const path of ["/docs/", "/docs/getting-started/"]) {
    await page.goto(path);
    await page.locator(".document-body").getByRole("link", { name: "feature overview", exact: true }).click();
    await expect(page).toHaveURL(/\/docs\/features\/$/);
  }
  await expect(page.getByRole("heading", { name: "Features", exact: true })).toBeVisible();
  await expect(page.locator(".page-pagination")).toHaveCount(0);
  const rows = page.locator(".document-body table tbody tr");
  await expect(rows).toHaveCount(featureCount);
  for (const row of await rows.all()) {
    await expect(row.locator('a[href^="/docs/"]').first()).toBeVisible();
  }
  await rows.filter({ hasText: "Clickable hyperlinks" }).getByRole("link", { name: "Hyperlinks", exact: true }).click();
  await expect(page).toHaveURL(/\/docs\/guides\/styling-text\/#hyperlinks$/);
  await expect(page.getByRole("heading", { name: "Hyperlinks", exact: true })).toBeInViewport();
});

test("prose links stay underlined without changing section cards", async ({ page }) => {
  for (const colorScheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme });
    for (const [path, selector] of [
      ["/docs/features/", ".prose td a"],
      ["/docs/getting-started/first-app/", ".prose li > a"],
    ]) {
      await page.goto(path);
      await expect(page.locator(selector).first()).toHaveCSS("text-decoration-line", "underline");
    }
    await page.goto("/docs/getting-started/");
    await expect(page.locator(".doc-card").first()).toHaveCSS("text-decoration-line", "none");
  }
});

test("sidebar sections collapse and expand with the pointer and keyboard", async ({ page }) => {
  await page.goto("/docs/concepts/cells/");
  const sidebar = page.getByRole("complementary", { name: "Documentation navigation" });
  const concepts = sidebar.locator(".nav-group").filter({ has: page.getByText("Concepts", { exact: true }) });
  const details = concepts.locator("details");
  const summary = concepts.locator("summary");
  const title = concepts.getByRole("link", { name: "Concepts", exact: true });
  const cells = concepts.getByRole("link", { name: "Cells", exact: true });
  const groupBox = await concepts.boundingBox();
  const summaryBox = await summary.boundingBox();
  expect(summaryBox.x).toBeGreaterThan(groupBox.x + groupBox.width / 2);
  expect(summaryBox.x + summaryBox.width).toBeCloseTo(groupBox.x + groupBox.width);
  await expect(details).toHaveJSProperty("open", true);
  await summary.focus();
  await page.keyboard.press("Space");
  await expect(cells).toBeHidden();
  await expect(details).toHaveJSProperty("open", false);
  await expect(title).toBeVisible();
  await summary.click({ position: { x: 4, y: 10 } });
  await expect(cells).toBeVisible();
  await expect(cells).toHaveAttribute("aria-current", "page");
  await title.click();
  await expect(page).toHaveURL(/\/docs\/concepts\/$/);
  await expect(title).toHaveAttribute("aria-current", "page");
});

test("documentation keeps a sidebar, a reading column, and a footer across both", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  for (const path of ["/docs/", "/docs/getting-started/"]) {
    await page.goto(path);
    await expect(page.getByRole("contentinfo")).toBeInViewport();
    expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBe(page.viewportSize().height);
  }
  await page.goto("/docs/concepts/cells/");
  const sidebar = page.getByRole("complementary", { name: "Documentation navigation" });
  const outline = page.getByRole("complementary", { name: "On this page" });
  await expect(sidebar).toBeVisible();
  await expect(sidebar.getByRole("link", { name: "Cells", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.locator('img[alt="uncurses"]')).toHaveCount(1);
  const sidebarBox = await sidebar.boundingBox();
  const articleBox = await page.locator(".document-body").boundingBox();
  const outlineBox = await outline.boundingBox();
  const shellBox = await page.locator(".docs-shell").boundingBox();
  const footerBox = await page.getByRole("contentinfo").boundingBox();
  expect(sidebarBox.x + sidebarBox.width).toBeLessThan(articleBox.x);
  expect(articleBox.width).toBeLessThanOrEqual(720);
  expect(footerBox.x).toBe(0);
  expect(footerBox.width).toBe(page.viewportSize().width);
  await expect(page.getByRole("contentinfo")).toHaveCSS("border-top-color", await page.getByRole("banner").evaluate((node) => getComputedStyle(node).borderBottomColor));
  expect(footerBox.y).toBeGreaterThanOrEqual(shellBox.y + shellBox.height - 1);
  expect(outlineBox.x).toBeCloseTo(articleBox.x);
  await expect(outline.locator("details")).toHaveJSProperty("open", false);
  await outline.locator("summary").click();
  const anchor = outline.getByRole("link").first();
  const fragment = await anchor.getAttribute("href");
  await anchor.click();
  expect(new URL(page.url()).hash).toBe(fragment);
  expect((await page.locator(fragment).boundingBox()).y).toBeGreaterThanOrEqual(76);
  await page.evaluate(() => scrollTo(0, 900));
  expect((await page.locator(".site-header").boundingBox()).y).toBe(0);
  expect((await sidebar.boundingBox()).y).toBe(76);
  await sidebar.getByRole("link", { name: "Keyboard input", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Keyboard input");
  await expect(sidebar.getByRole("link", { name: "Keyboard input", exact: true })).toHaveAttribute("aria-current", "page");
});

test("diagrams render locally and code blocks copy their source", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/docs/concepts/cells/");
  await expect(page.locator(".mermaid svg")).toBeVisible();
  await page.getByRole("button", { name: "Copy", exact: true }).first().click();
  await expect(page.getByRole("button", { name: "Copied!" })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('Cell::new("a", 1)');
  expect(errors).toEqual([]);
});

for (const width of [320, 390, 768, 1440]) {
  test(`layouts fit the viewport at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const path of ["/", "/docs/", "/docs/features/", "/docs/getting-started/hello-world/", "/docs/concepts/screen/", "/404.html"]) {
      await page.goto(path);
      await page.evaluate(() => document.fonts.ready);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      if (path === "/") {
        const content = await page.locator(".page-shell").boundingBox();
        expect(Math.abs(content.y + content.height / 2 - page.viewportSize().height / 2)).toBeLessThan(1);
      }
    }
  });
}

test("mobile navigation reaches all documentation sections", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "Manual", exact: true }).click();
  const menu = page.locator(".docs-menu");
  const sidebar = page.getByRole("complementary", { name: "Documentation navigation" });
  await expect(menu).toHaveJSProperty("open", false);
  await menu.locator(":scope > summary").click();
  await sidebar.getByRole("link", { name: "Concepts", exact: true }).click();
  await menu.locator(":scope > summary").click();
  await sidebar.getByRole("link", { name: "Cells", exact: true }).click();
  await expect(page).toHaveURL(/\/docs\/concepts\/cells\//);
  await expect(menu).toHaveJSProperty("open", false);
  await expect(page.locator(".page-toc details")).toHaveJSProperty("open", false);
  await page.locator(".page-toc summary").click();
  await expect(page.locator(".page-toc a").first()).toBeVisible();
  await page.setViewportSize({ width: 1280, height: 900 });
  await expect(menu).toHaveJSProperty("open", true);
  await expect(sidebar.getByRole("link", { name: "Cells", exact: true })).toBeVisible();
  await expect(page.locator(".page-toc details")).toHaveJSProperty("open", true);
  await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(menu).toHaveJSProperty("open", false);
  await expect(page.locator(".page-toc details")).toHaveJSProperty("open", true);
  await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
});

test("the website remains readable without JavaScript", async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false, colorScheme: "light" });
  const page = await context.newPage();
  await page.goto("http://127.0.0.1:13989/");
  await expect(page.getByRole("link", { name: "Get started", exact: true })).toBeVisible();
  await expect(page.locator("main")).toContainText("A library for building terminal user interfaces.");
  await expect(page.locator('img[alt="uncurses"]')).toHaveCount(1);
  await expect.poll(() => page.locator(".brand img").evaluate((image) => image.currentSrc)).toMatch(/uncurses-logo\.png$/);
  await page.emulateMedia({ colorScheme: "dark" });
  await expect.poll(() => page.locator(".brand img").evaluate((image) => image.currentSrc)).toMatch(/uncurses-logo-dark\.png$/);
  await expect(page.locator("[data-theme-toggle]")).toBeHidden();
  await expect(page.locator("[data-terminal-text]")).toHaveText("Hello, terminal!");
  await page.getByRole("link", { name: "Get started", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Getting Started");
  await page.getByRole("complementary", { name: "Documentation navigation" }).getByRole("link", { name: "Cells", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Cells");
  await context.close();
});

test("every documentation page and internal link resolves", async ({ page, request }) => {
  const index = await (await request.get("/search.json")).json();
  const links = new Set();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  for (const entry of index) {
    const response = await page.goto(entry.url);
    expect(response.status(), entry.url).toBe(200);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(entry.title);
    const diagrams = page.locator(".mermaid");
    for (const diagram of await diagrams.all()) {
      const svg = diagram.locator("svg");
      await expect(svg).toBeVisible();
      await expect(svg).toHaveAccessibleName(/\S/);
      await expect(svg).not.toHaveAccessibleName("Documentation diagram");
      await expect(svg).toHaveAccessibleDescription(/\S/);
    }
    for (const link of await page.locator('a[href^="/"]').evaluateAll((nodes) => nodes.map((node) => node.getAttribute("href")))) {
      if (!link.startsWith("/api/")) links.add(link.split("#")[0]);
    }
  }
  for (const link of links) expect((await request.get(link)).status(), link).toBe(200);
  expect(errors).toEqual([]);
});

for (const theme of ["light", "dark"]) {
  for (const path of ["/", "/docs/", "/docs/concepts/cells/"]) {
    test(`the ${theme} page meets WCAG accessibility checks: ${path}`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: theme });
      await page.goto(path);
      await page.evaluate(() => document.fonts.ready);
      if (path.includes("cells")) await expect(page.locator(".mermaid svg")).toBeVisible();
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze();
      expect(results.violations.map(({ id, nodes }) => ({ id, nodes: nodes.map(({ target, failureSummary }) => ({ target, failureSummary })) }))).toEqual([]);
    });
  }
}

test("themes follow the system, persist across pages, and return to automatic mode", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  const background = () => page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  const logoSource = () => page.locator(".brand img").evaluate((image) => image.currentSrc);
  const light = await background();
  await expect.poll(logoSource).toMatch(/uncurses-logo\.png$/);
  await page.emulateMedia({ colorScheme: "dark" });
  expect(await background()).not.toBe(light);
  await expect.poll(logoSource).toMatch(/uncurses-logo-dark\.png$/);
  const theme = page.getByRole("button", { name: /^Color theme:/ });
  await expect(theme).toHaveAccessibleName("Color theme: Auto. Switch to Light.");
  await expect(theme.locator('[data-theme-icon="system"]')).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Main navigation" }).locator("[data-theme-toggle]")).toBeVisible();
  await theme.focus();
  await page.keyboard.press("Enter");
  await expect(theme).toHaveAccessibleName("Color theme: Light. Switch to Dark.");
  await expect(theme.locator('[data-theme-icon="light"]')).toBeVisible();
  expect(await background()).toBe(light);
  await expect.poll(logoSource).toMatch(/uncurses-logo\.png$/);
  await page.goto("/docs/concepts/cells/");
  await expect(theme).toHaveAccessibleName("Color theme: Light. Switch to Dark.");
  await expect(page.getByRole("banner").locator("[data-theme-toggle]")).toBeVisible();
  await expect(page.locator('img[alt="uncurses"]')).toHaveCount(1);
  await expect.poll(logoSource).toMatch(/uncurses-logo\.png$/);
  await expect(page.locator(".mermaid svg")).toBeVisible();
  const diagramBefore = await page.locator(".mermaid").innerHTML();
  await theme.click();
  await expect(theme).toHaveAccessibleName("Color theme: Dark. Switch to Auto.");
  await expect(theme.locator('[data-theme-icon="dark"]')).toBeVisible();
  await expect.poll(() => page.locator(".mermaid").innerHTML()).not.toBe(diagramBefore);
  await expect(page.locator(".mermaid svg")).toBeVisible();
  await expect(page.locator(".mermaid svg")).toHaveAccessibleName("Cell content, style, and width");
  await expect(page.locator(".mermaid svg")).toHaveAccessibleDescription(/A cell stores a grapheme/);
  await page.reload();
  await expect(theme).toHaveAccessibleName("Color theme: Dark. Switch to Auto.");
  await expect.poll(logoSource).toMatch(/uncurses-logo-dark\.png$/);
  await theme.click();
  await expect(theme).toHaveAccessibleName("Color theme: Auto. Switch to Light.");
  expect(await page.evaluate(() => localStorage.getItem("uncurses-theme"))).toBeNull();
  await page.emulateMedia({ colorScheme: "light" });
  expect(await background()).toBe(light);
  await expect.poll(logoSource).toMatch(/uncurses-logo\.png$/);
});
