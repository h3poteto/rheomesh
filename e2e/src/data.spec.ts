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

test("Data", async ({ browserType }) => {
  const browser1 = await browserType.launch({});
  const browser2 = await browserType.launch({});
  const context1 = await browser1.newContext();
  const page1 = await context1.newPage();
  const context2 = await browser2.newContext();
  const page2 = await context2.newPage();

  await page1.goto("http://localhost:5173/");
  await page2.goto("http://localhost:5173/");

  await page1.click("#connect");
  await page2.click("#connect");

  await page1.waitForFunction(() => {
    const button = document.querySelector("#start") as HTMLButtonElement;
    return button && button.disabled === false;
  });
  await page1.click("#start");
  await page1.waitForFunction(() => {
    const button = document.querySelector("#send") as HTMLButtonElement;
    return button && button.disabled === false;
  });

  await page1.waitForTimeout(5000);

  await page1.fill("#data", "hello");
  await page1.click("#send");

  await page2.waitForTimeout(1000);

  const text = await page2.$eval("#remote_data", (el) => el.textContent);
  expect(text).toBe("hello");

  const browser3 = await browserType.launch({});
  const context3 = await browser3.newContext();
  const page3 = await context3.newPage();
  await page3.goto("http://localhost:5173/");
  await page3.click("#connect");
  await page3.waitForFunction(() => {
    const button = document.querySelector("#start") as HTMLButtonElement;
    return button && button.disabled === false;
  });

  await page3.waitForTimeout(5000);

  // Receive data on page3
  await page1.fill("#data", "hello from page1");
  await page1.click("#send");

  await page2.waitForTimeout(1000);

  const text2 = await page2.$eval("#remote_data", (el) => el.textContent);
  expect(text2).toBe("hello from page1");

  const text3 = await page3.$eval("#remote_data", (el) => el.textContent);
  expect(text3).toBe("hello from page1");

  await page3.click("#start");
  await page3.waitForFunction(() => {
    const button = document.querySelector("#send") as HTMLButtonElement;
    return button && button.disabled === false;
  });
  await page3.waitForTimeout(5000);
  await page3.fill("#data", "hello from page3");
  await page3.click("#send");

  await page1.waitForTimeout(1000);
  const text4 = await page1.$eval("#remote_data", (el) => el.textContent);
  expect(text4).toBe("hello from page3");
  const text5 = await page2.$eval("#remote_data", (el) => el.textContent);
  expect(text5).toBe("hello from page3");
});
