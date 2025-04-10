import { test, chromium, expect } from "@playwright/test";

test("Camera", async () => {
  const browser1 = await chromium.launch({
    args: [
      "--use-fake-ui-for-media-stream",
      "--use-fake-device-for-media-stream",
    ],
  });

  const browser2 = await chromium.launch({
    args: [
      "--use-fake-ui-for-media-stream",
      "--use-fake-device-for-media-stream",
    ],
  });

  const context1 = await browser1.newContext();
  const page1 = await context1.newPage();

  const context2 = await browser2.newContext();
  const page2 = await context2.newPage();

  await page1.goto("http://localhost:3000/room?room=example");
  await page2.goto("http://localhost:3000/room?room=example");

  await page1.click("#connect");
  await page2.click("#connect");

  await page1.waitForFunction(() => {
    const button = document.querySelector("#capture") as HTMLButtonElement;
    return button && button.disabled === false;
  });

  await page1.click("#capture");

  await page1.waitForSelector("#sending-video");

  await page2.waitForSelector(".receiving-video");

  await page2.waitForFunction(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video && video.readyState > 2;
  });

  const videoState1 = await page1.evaluate(() => {
    const video = document.querySelector("#sending-video") as HTMLVideoElement;
    return video ? video.readyState : -1;
  });

  const videoState2 = await page2.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.readyState : -1;
  });

  console.log(`Browser 1 video state: ${videoState1}`);
  console.log(`Browser 2 video state: ${videoState2}`);

  expect(videoState1).toBeGreaterThan(2);
  expect(videoState2).toBeGreaterThan(2);

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

  await browser1.close();
  await browser2.close();
});
