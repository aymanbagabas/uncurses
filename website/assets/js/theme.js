export function currentTheme() {
  return document.documentElement.dataset.theme ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
}

export function initTheme() {
  const control = document.querySelector("[data-theme-toggle]");
  const themes = ["system", "light", "dark"];
  const labels = { system: "Auto", light: "Light", dark: "Dark" };
  const notice = document.querySelector("[data-theme-notice]");
  const root = document.documentElement;
  const media = matchMedia("(prefers-color-scheme: dark)");
  control.hidden = false;
  function nextTheme() {
    return themes[(themes.indexOf(root.dataset.theme ?? "system") + 1) % themes.length];
  }
  function update() {
    const theme = root.dataset.theme ?? "system";
    const label = labels[theme];
    for (const icon of control.querySelectorAll("[data-theme-icon]")) {
      icon.toggleAttribute("hidden", icon.dataset.themeIcon !== theme);
    }
    control.setAttribute("aria-label", `Color theme: ${label}. Switch to ${labels[nextTheme()]}.`);
    control.title = control.getAttribute("aria-label");
  }
  function notify() {
    document.querySelector("[data-logo-dark]").media = currentTheme() === "dark" ? "all" : "not all";
    document.dispatchEvent(new CustomEvent("themechange", { detail: currentTheme() }));
  }
  control.addEventListener("click", () => {
    const theme = nextTheme();
    if (theme === "system") delete root.dataset.theme;
    else root.dataset.theme = theme;
    try {
      if (theme === "system") localStorage.removeItem("uncurses-theme");
      else localStorage.setItem("uncurses-theme", theme);
      notice.textContent = "";
    } catch (error) {
      notice.textContent = "The theme changed, but this browser could not save your preference.";
      console.warn("Could not save the color theme.", error);
    }
    update();
    notify();
  });
  update();
  notify();
  media.addEventListener("change", () => {
    if (!root.dataset.theme) notify();
  });
}
