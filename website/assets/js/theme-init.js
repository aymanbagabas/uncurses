try {
  const theme = localStorage.getItem("uncurses-theme");
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
} catch (error) {
  console.warn("Could not read the saved color theme.", error);
}
