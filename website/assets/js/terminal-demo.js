const demo = document.querySelector(".terminal-demo");
const output = demo.querySelector("[data-terminal-text]");
const { frames } = JSON.parse(demo.querySelector("[data-terminal-recording]").textContent);
const motion = matchMedia("(prefers-reduced-motion: reduce)");
let frame = motion.matches ? frames.length - 1 : 0;
let timer;

function update() {
  clearTimeout(timer);
  const active = !motion.matches && !document.hidden && !demo.matches(":hover, :focus-within");
  demo.dataset.playing = String(active);
  output.textContent = frames[frame].text;
  if (active) {
    timer = setTimeout(() => {
      frame = (frame + 1) % frames.length;
      update();
    }, frames[frame].duration);
  }
}

demo.tabIndex = 0;
for (const event of ["mouseenter", "mouseleave", "focusin", "focusout"]) {
  demo.addEventListener(event, update);
}
motion.addEventListener("change", () => {
  frame = motion.matches ? frames.length - 1 : 0;
  update();
});
document.addEventListener("visibilitychange", update);
update();
