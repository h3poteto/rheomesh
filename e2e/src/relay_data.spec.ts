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

  await page1.goto("http://localhost:3001");
  await page2.goto("http://localhost:3002");

  await page1.waitForSelector("#connect");
  await page2.waitForSelector("#connect");

  await page1.click("#connect");
  await page2.click("#connect");
  await page2.waitForTimeout(5000);

  await page1.click("#start");
  await page1.waitForFunction(() => {
    const button = document.querySelector("#send") as HTMLButtonElement;
    return button && button.disabled === false;
  });
  await page2.waitForTimeout(5000);

  const text = "hello world";
  await page1.fill("#data", text);
  await page1.click("#send");
  console.log("Sending text: ", text);

  await page2.waitForFunction(() => {
    const span = document.querySelector("#remote_data") as HTMLSpanElement;
    return span && span.innerText.length > 0;
  });

  const response = await page2.evaluate(() => {
    const span = document.querySelector("#remote_data") as HTMLSpanElement;
    return span?.innerText;
  });

  expect(response).toBe(text);
  console.log("Received text: ", text);

  await browser1.close();
  await browser2.close();
});
