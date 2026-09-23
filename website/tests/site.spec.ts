import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import document from '../plugin.json' with { type: 'json' };

test('discovery metadata matches page content and all preview files exist', async ({ page, request }) => {
  await page.goto('/');
  await expect(page.locator('link[rel=island-plugin]')).toHaveAttribute('href', '/.well-known/island-plugin.json');
  const response = await request.get('/.well-known/island-plugin.json');
  expect(response.status()).toBe(200);
  expect(response.headers()['content-type']).toContain('application/json');
  expect(await response.json()).toEqual(document);
  for (const feature of document.features) await expect(page.getByText(feature.title, {exact:true})).toBeVisible();
  for (const preview of document.previews) expect((await request.get(new URL(preview.url).pathname)).status()).toBe(200);
});

for (const width of [320, 375, 414, 768, 1280, 1440]) {
  test(`fits ${width}px without clipped content`, async ({ page }, testInfo) => {
    await page.setViewportSize({width, height:800});
    await page.goto('/');
    await page.evaluate(()=>window.document.fonts.ready);
    const bad = await page.locator('main p, h1, h2, h3, .button, .text-link, nav a, section, figure').evaluateAll(nodes => nodes.filter(node=>{
      const rect=node.getBoundingClientRect();
      return rect.left < -1 || rect.right > innerWidth + 1;
    }).map(node=>node.textContent));
    expect(bad).toEqual([]);
    expect(await page.evaluate(()=>window.document.documentElement.scrollWidth > innerWidth)).toBe(false);
    if(width >= 1280) {
      const product=await page.locator('.gallery-image.compact').boundingBox();
      expect(product!.y+product!.height).toBeLessThan(800);
    }
    for (const control of await page.locator('nav a, .button, .text-link').all()) await expect(control).toHaveCSS('white-space','nowrap');
    await page.screenshot({path:testInfo.outputPath(`spotify-${width}.png`),fullPage:true});
  });
}

test('accessible page, keyboard gallery and reduced motion', async ({page})=>{
  const errors: string[]=[]; page.on('pageerror',e=>errors.push(e.message));
  await page.goto('/');
  expect((await new AxeBuilder({page}).withTags(['wcag2a','wcag2aa','wcag21aa']).analyze()).violations).toEqual([]);
  const expanded=page.getByRole('button',{name:'Expanded',exact:true});
  await expanded.focus(); const y=await page.evaluate(()=>scrollY);
  await page.keyboard.press('Enter');
  await expect(expanded).toHaveAttribute('aria-pressed','true');
  await expect(page.locator('.gallery').getByAltText(document.previews[1].alt)).toBeVisible();
  await expect(page.locator('.gallery figcaption')).toHaveText(document.previews[1].caption);
  expect(await page.evaluate(()=>scrollY)).toBe(y);
  await page.emulateMedia({reducedMotion:'reduce'});
  await expect(page.locator('.gallery-image:visible')).toHaveCSS('animation-name','none');
  expect(errors).toEqual([]);
});

test('setup links and clipboard work, with a useful denial state', async ({page,context})=>{
  await context.grantPermissions(['clipboard-read','clipboard-write']);
  await page.goto('/');
  await page.getByRole('link',{name:'Set up the plugin'}).click();
  await expect(page).toHaveURL('/#setup');
  const copy=page.getByRole('button',{name:'Copy',exact:true});
  await copy.click();
  expect(await page.evaluate(()=>navigator.clipboard.readText())).toContain('island-cli plugin enable com.sullivansome.spotify');
  await page.evaluate(()=>Object.defineProperty(navigator.clipboard,'writeText',{value:()=>Promise.reject(new Error('denied'))}));
  await page.getByRole('button',{name:'Copied ✓'}).click();
  await expect(page.getByText('Clipboard access was blocked. Select and copy the command above.')).toBeVisible();
});

test('no JavaScript still provides navigation, content, previews and 404 recovery', async ({browser,request})=>{
  const context=await browser.newContext({javaScriptEnabled:false,viewport:{width:320,height:800}});
  const page=await context.newPage();
  await page.goto('http://127.0.0.1:4328/');
  await expect(page.getByAltText(document.previews[0].alt)).toBeVisible();
  await expect(page.getByRole('button',{name:'Expanded',exact:true})).toBeHidden();
  await page.getByRole('link',{name:'Set up',exact:true}).click();
  await expect(page).toHaveURL(/#setup$/);
  expect((await request.get('/not-a-page/')).status()).toBe(404);
  await page.goto('http://127.0.0.1:4328/not-a-page/');
  await page.getByRole('link',{name:'Back to the plugin'}).click();
  await expect(page.getByRole('heading',{level:1})).toHaveText('Stay inthe song.');
  await context.close();
});
