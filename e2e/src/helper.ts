import { expect, Locator } from "@playwright/test";

export const expectPlaying = async (video: Locator, label: string) => {
  const readyState = () =>
    video.evaluate((v: HTMLVideoElement) => v.readyState);

  await expect(video).toBeAttached({ timeout: 30000 });
  // A video element can exist before any frame arrives, so readyState starts at 0.
  await expect.poll(readyState, { timeout: 30000 }).toBeGreaterThan(2);

  console.log(`${label} video state: ${await readyState()}`);
};
