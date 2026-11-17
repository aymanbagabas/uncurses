import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/browser",
  fullyParallel: true,
  use: {
    baseURL: "http://127.0.0.1:13989",
    launchOptions: {
      executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
    },
  },
  webServer: {
    command: "hugo server --bind 127.0.0.1 --port 13989 --disableFastRender --destination ../target/website-preview",
    url: "http://127.0.0.1:13989",
    reuseExistingServer: !process.env.CI,
  },
});
