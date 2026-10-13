import {
  test as base,
  chromium,
  expect,
  BrowserType,
  firefox,
} from "@playwright/test";
import { expectPlaying } from "./helper";

const browserTypes: { [key: string]: BrowserType } = {
  chromium,
  firefox,
};

const test = base.extend<{ browserType: BrowserType }>({
  browserType: async ({ browserName }, use) => {
    await use(browserTypes[browserName]);
  },
});

test("Screen", async ({ browserType }) => {
  const browser1 = await browserType.launch({});

  const browser2 = await browserType.launch({});

  const context1 = await browser1.newContext();
  const page1 = await context1.newPage();

  const context2 = await browser2.newContext();
  const page2 = await context2.newPage();

  await page1.goto("http://localhost:3000/room?room=example-screen");
  await page2.goto("http://localhost:3000/room?room=example-screen");

  await page1.click("#connect");
  await page2.click("#connect");

  await page1.waitForFunction(() => {
    const button = document.querySelector("#capture") as HTMLButtonElement;
    return button && button.disabled === false;
  });

  await page1.click("#capture");

  await expectPlaying(page1.locator("#sending-video"), "Browser 1");

  await expectPlaying(page2.locator(".receiving-video"), "Browser 2");

  const videoPaused1 = await page1.evaluate(() => {
    const video = document.querySelector("#sending-video") as HTMLVideoElement;
    return video ? video.paused : true;
  });

  const videoPaused2 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.paused : true;
  });

  console.log(`Browser 1 video paused: ${videoPaused1}`);
  console.log(`Browser 2 video paused: ${videoPaused2}`);

  expect(videoPaused1).toBe(false);
  expect(videoPaused2).toBe(false);

  const video2Time1 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });

  console.log(`Browser 2 video current time: ${video2Time1}`);

  await page1.waitForTimeout(10000); // Wait for 10 seconds

  const video2Time2 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });

  console.log(`Browser 2 video current time: ${video2Time2}`);

  expect(video2Time2).toBeGreaterThan(video2Time1);

  // Add browser3
  const browser3 = await browserType.launch({});

  const context3 = await browser3.newContext();
  const page3 = await context3.newPage();
  await page3.goto("http://localhost:3000/room?room=example-screen");
  await page3.click("#connect");
  await page3.waitForFunction(() => {
    const button = document.querySelector("#capture") as HTMLButtonElement;
    return button && button.disabled === false;
  });
  // Confirm browser3 is subscribing the video
  await expectPlaying(page3.locator(".receiving-video"), "Browser 3");

  // Start publishing video from browser3
  await page3.click("#capture");
  await expectPlaying(page3.locator("#sending-video"), "Browser 3");

  // Browser1 can receive the video from browser3
  await expectPlaying(page1.locator(".receiving-video"), "Browser 1");

  // Browser2 can receive the video from browser3. It should have 2 video elements
  const videoLocator = page2.locator(".receiving-video");
  await expect(videoLocator).toHaveCount(2);

  for (let i = 0; i < 2; i++) {
    const videoElement = videoLocator.nth(i);
    await expectPlaying(videoElement, `Browser 2 video ${i}`);
  }

  await browser1.close();
  await browser2.close();
});

test("Restart ICE", async ({ browserType }) => {
  const browser1 = await browserType.launch({});
  const browser2 = await browserType.launch({});

  const context1 = await browser1.newContext();
  const page1 = await context1.newPage();

  const context2 = await browser2.newContext();
  const page2 = await context2.newPage();

  await page1.goto("http://localhost:3000/room?room=example-ice");
  await page2.goto("http://localhost:3000/room?room=example-ice");

  await page1.click("#connect");
  await page2.click("#connect");

  await page1.waitForFunction(() => {
    const button = document.querySelector("#capture") as HTMLButtonElement;
    return button && button.disabled === false;
  });

  await page1.click("#capture");

  await expectPlaying(page1.locator("#sending-video"), "Browser 1");

  await expectPlaying(page2.locator(".receiving-video"), "Browser 2");

  const video2Time1 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });

  console.log(`Browser 2 video current time: ${video2Time1}`);

  await page1.waitForTimeout(10000); // Wait for 10 seconds

  const video2Time2 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });

  console.log(`Browser 2 video current time: ${video2Time2}`);
  expect(video2Time2).toBeGreaterThan(video2Time1);

  // Restart ICE
  await page2.click("#restart");

  await page2.waitForTimeout(5000); // Wait for 5 seconds
  await expectPlaying(page2.locator(".receiving-video"), "Browser 2");

  const video2Time3 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });
  console.log(`Browser 2 video current time: ${video2Time3}`);
  expect(video2Time3).toBeGreaterThan(video2Time2);
});
