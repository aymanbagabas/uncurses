import mermaid from "mermaid";
import { currentTheme } from "./theme.js";

const diagrams = [...document.querySelectorAll(".mermaid")].map((node) => ({ node, source: node.textContent }));
let renderQueue = Promise.resolve();

async function renderDiagrams() {
  const dark = currentTheme() === "dark";
  const background = dark ? "#262a33" : "#f6f7fb";
  const ink = dark ? "#e8eaed" : "#23344b";
  const blue = dark ? "#293f59" : "#ddeafb";
  const green = dark ? "#233f38" : "#e0f3eb";
  const gold = dark ? "#493b27" : "#fff1d2";
  const blueBorder = dark ? "#89b4e2" : "#4f77aa";
  const goldBorder = dark ? "#d4ad71" : "#aa7c38";
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: "base",
    themeVariables: {
      darkMode: dark,
      background,
      primaryColor: blue,
      primaryTextColor: ink,
      primaryBorderColor: blueBorder,
      lineColor: dark ? "#b9c9dd" : "#5f738c",
      secondaryColor: green,
      secondaryTextColor: ink,
      secondaryBorderColor: dark ? "#73bfa3" : "#3f7b67",
      tertiaryColor: gold,
      tertiaryTextColor: ink,
      tertiaryBorderColor: goldBorder,
      clusterBkg: gold,
      clusterBorder: goldBorder,
      edgeLabelBackground: background,
      actorBkg: blue,
      actorBorder: blueBorder,
      actorTextColor: ink,
      noteBkgColor: gold,
      noteTextColor: ink,
      noteBorderColor: goldBorder,
      fontFamily: "system-ui, sans-serif",
    },
  });
  for (const { node, source } of diagrams) {
    const message = node.closest(".diagram").querySelector(".diagram-error");
    try {
      node.textContent = source;
      node.removeAttribute("data-processed");
      message.hidden = true;
      await mermaid.run({ nodes: [node], suppressErrors: false });
      const svg = node.querySelector("svg");
      if (svg && !svg.querySelector("title")) {
        const title = document.createElementNS("http://www.w3.org/2000/svg", "title");
        title.textContent = "Documentation diagram";
        svg.prepend(title);
        svg.setAttribute("role", "img");
      }
    } catch (error) {
      message.hidden = false;
      message.textContent = "This diagram could not render. Reload the page to try again.";
      console.error("Could not render a documentation diagram.", error);
    }
  }
}

function queueRender() {
  renderQueue = renderQueue.then(() => document.fonts.ready).then(renderDiagrams);
}
document.addEventListener("themechange", queueRender);
queueRender();
