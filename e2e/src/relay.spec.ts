import {
  test as base,
  chromium,
  expect,
  BrowserType,
  firefox,
} from "@playwright/test";

const browserTypes: { [key: string]: BrowserType } = {
  chromium,
  firefox,
};

const test = base.extend<{ browserType: BrowserType }>({
  browserType: async ({ browserName }, use) => {
    await use(browserTypes[browserName]);
  },
});

test("Relay", async ({ browserType }) => {
  const browser1 = await browserType.launch({});
  const browser2 = await browserType.launch({});
  const context1 = await browser1.newContext();
  const page1 = await context1.newPage();
  const context2 = await browser2.newContext();
  const page2 = await context2.newPage();

  await page1.goto("http://localhost:3001/room?room=example-relay");
  await page2.goto("http://localhost:3002/room?room=example-relay");

  await page1.waitForSelector("#connect");
  await page2.waitForSelector("#connect");

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

  // Reload browser 2 and check if the video is still playing
  await context2.close();
  const context3 = await browser2.newContext();
  const page3 = await context3.newPage();
  await page3.goto("http://localhost:3002/room?room=example-relay");
  await page3.waitForSelector("#connect");
  await page3.click("#connect");
  await page3.waitForSelector(".receiving-video");
  await page3.waitForFunction(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video && video.readyState > 2;
  });
  const video3State = await page3.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.readyState : -1;
  });
  console.log(`Browser 2 reloaded video state: ${video3State}`);

  expect(video3State).toBeGreaterThan(2);
  const video3Paused = await page3.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.paused : true;
  });
  console.log(`Browser 2 reloaded video paused: ${video3Paused}`);
  expect(video3Paused).toBe(false);
  const video3Time = await page3.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });
  console.log(`Browser 2 reloaded video current time: ${video3Time}`);
  await page3.waitForTimeout(10000); // Wait for 10 seconds
  const video3Time2 = await page3.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.currentTime : -1;
  });
  console.log(`Browser 2 reloaded video current time: ${video3Time2}`);
  expect(video3Time2).toBeGreaterThan(video3Time);

  await browser1.close();
  await browser2.close();
});
