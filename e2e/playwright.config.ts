// playwright.config.js
import { PlaywrightTestConfig } from "@playwright/test";

const config: PlaywrightTestConfig = {
  timeout: 60 * 1000,
  workers: 1,
  use: {
    locale: "en-US",
    headless: true,
  },
  projects: [
    {
      name: "chromium",
      use: {
        browserName: "chromium",
        permissions: ["microphone", "camera"],
        launchOptions: {
          args: [
            "--use-fake-ui-for-media-stream",
            "--use-fake-device-for-media-stream",
          ],
        },
      },
    },
    {
      name: "firefox",
      use: {
        browserName: "firefox",
        launchOptions: {
          // Refs: https://webrtc.org/getting-started/testing
          // Refs: https://github.com/microsoft/playwright/issues/4532#issuecomment-735692428
          firefoxUserPrefs: {
            "media.navigator.streams.fake": true,
            "media.navigator.permission.disabled": true,
            "media.peerconnection.ice.obfuscate_host_addresses": false,
          },
        },
      },
    },
  ],
};
export default config;
