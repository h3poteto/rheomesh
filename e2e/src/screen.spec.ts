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
  await page3.waitForFunction(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video && video.readyState > 2;
  });
  const videoState3 = await page3.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.readyState : -1;
  });

  console.log(`Browser 3 video state: ${videoState3}`);
  expect(videoState3).toBeGreaterThan(2);

  // Start publishing video from browser3
  await page3.click("#capture");
  await page3.waitForSelector("#sending-video");
  await page3.waitForFunction(() => {
    const video = document.querySelector("#sending-video") as HTMLVideoElement;
    return video && video.readyState > 2;
  });
  const videoState4 = await page3.evaluate(() => {
    const video = document.querySelector("#sending-video") as HTMLVideoElement;
    return video ? video.readyState : -1;
  });
  console.log(`Browser 3 video state: ${videoState4}`);
  expect(videoState4).toBeGreaterThan(2);

  // Browser1 can receive the video from browser3
  await page1.waitForSelector(".receiving-video");
  await page1.waitForFunction(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video && video.readyState > 2;
  });
  const videoState5 = await page1.evaluate(() => {
    const video = document.querySelector(
      ".receiving-video",
    ) as HTMLVideoElement;
    return video ? video.readyState : -1;
  });
  console.log(`Browser 1 video state: ${videoState5}`);
  expect(videoState5).toBeGreaterThan(2);

  // Browser2 can receive the video from browser3. It should have 2 video elements
  await page2.waitForSelector(".receiving-video");
  const videoLocator = page2.locator(".receiving-video");
  const count = await videoLocator.count();
  expect(count).toBe(2);

  for (let i = 0; i < count; i++) {
    const videoElement = videoLocator.nth(i);
    const videoState6 = await videoElement.evaluate((video) => {
      return (video as HTMLVideoElement).readyState;
    });
    console.log(`Browser 2 video state: ${videoState6}`);
    expect(videoState6).toBeGreaterThan(2);
  }

  await browser1.close();
  await browser2.close();
});
