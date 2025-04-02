// playwright.config.js
import { PlaywrightTestConfig } from "@playwright/test";

const config: PlaywrightTestConfig = {
  use: {
    locale: "en-US",
    permissions: ["microphone", "camera"],
    headless: false,
    launchOptions: {
      args: [
        "--use-fake-ui-for-media-stream",
        "--use-fake-device-for-media-stream",
      ],
    },
  },
};
export default config;
