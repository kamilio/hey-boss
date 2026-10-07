// PLAYWRIGHT_MODULE=/path/to/playwright node tests/named-accounts.cjs
// Screenshots are optional and belong in a temporary directory.
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
(async () => {
  const root = path.join(__dirname, '../src/proxy');
  const server = http.createServer((req, res) => {
    const file = req.url === '/overview.js' ? 'overview.js' : 'overview.html';
    res.setHeader('content-type', file.endsWith('.js') ? 'application/javascript' : 'text/html');
    res.end(fs.readFileSync(path.join(root, file)));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({channel:"chrome",headless:true});
    const page = await browser.newPage({viewport:{width:1440,height:1080}});
    const errors=[]; page.on('pageerror', e => errors.push(e.message));
    const catalog={mode:'standalone',relay:false,apis:[{id:'responses',name:'Responses',description:'Connect clients using the Responses API.',configured:true,base_path:'/v1',routes:[['POST','/v1/responses','Create a response']],models:[]}]};
    let status=200;
    let connections=[
      {name:'codex-personal',implementation:'codex',auth:'subscription',ready:true,account_ref:'acct_synthetic_one'},
      {name:'codex-work',implementation:'codex',auth:'subscription',ready:true,account_ref:'acct_synthetic_two'},
      {name:'codex-personal-alias',implementation:'codex',auth:'subscription',ready:true,account_ref:'acct_synthetic_one'},
      {name:'claude-personal',implementation:'claude',auth:'subscription',ready:false,account_ref:null},
      {name:'ultima',implementation:'openai',auth:'api',ready:true,account_ref:'acct_synthetic_api'}
    ];
    await page.route('**/overview/api',r=>r.fulfill({json:catalog}));
    await page.route('**/providers/v1',r=>r.fulfill({status,json:{schema_version:1,connections}}));
    await page.route('**/usage/v1/**',r=>r.fulfill({json:{state:'stale',updated_at:1790800000,error:'Rate limited; waiting before retrying',data:{windows:[{label:'Session · 5 hours',used_percent:93,remaining_percent:7,resets_at:null},{label:'Weekly',used_percent:null,remaining_percent:null,resets_at:null}]}}}));
    await page.goto(`http://127.0.0.1:${server.address().port}/apis`);
    await page.getByRole('button',{name:'View limits for codex-work',exact:true}).waitFor();
    assert.equal(await page.locator('.connection-card').count(),5);
    assert.equal(await page.getByText('Shared subscription · limits also apply to its other aliases.',{exact:true}).count(),2);
    assert(await page.getByRole('button',{name:'View limits for claude-personal',exact:true}).isDisabled());
    await page.getByRole('button',{name:'View limits for codex-work',exact:true}).click();
    await page.getByRole('heading',{name:'codex-work · subscription limits'}).waitFor();
    await page.getByText('93% used',{exact:true}).waitFor();
    assert.equal(await page.locator('#usage-windows progress').count(),1);
    assert.match(await page.locator('#usage-status').innerText(),/Stale/);
    for (const width of [1440,768,390,320]) {
      await page.setViewportSize({width,height:1080});
      for (const theme of ['light','dark']) {
        await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
        assert(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth), `overflow at ${width} ${theme}`);
        if(process.env.SCREENSHOTS) await page.screenshot({path:path.join(process.env.SCREENSHOTS,`accounts-${width}-${theme}.png`),fullPage:true});
      }
    }
    await page.locator('#refresh-accounts').focus();
    assert.equal(await page.evaluate(()=>document.activeElement.id),'refresh-accounts');
    await page.keyboard.press('Enter');
    connections=[{name:'a'.repeat(128),implementation:'codex',auth:'subscription',ready:false,account_ref:null}];
    await page.getByRole('button',{name:'Refresh accounts',exact:true}).click();
    await page.getByRole('heading',{name:'a'.repeat(128),exact:true}).waitFor();
    assert(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth),'long account name overflow');
    connections=[];
    await page.getByRole('button',{name:'Refresh accounts',exact:true}).click();
    await page.getByText('No named accounts configured. Default connections remain available below.',{exact:true}).waitFor();
    status=503;
    await page.getByRole('button',{name:'Refresh accounts',exact:true}).click();
    await page.getByText('Account status unavailable. Refresh to try again.',{exact:true}).waitFor();
    assert.equal(await page.locator('.connection-card').count(),0);
    assert.deepEqual(errors,[]);
    console.log('PASS: desktop/tablet/mobile, light/dark, overflow, keyboard, readiness, shared aliases, account limits, stale/unknown, empty/error states');
  } finally { if(browser) await browser.close(); await new Promise(resolve=>server.close(resolve)); }
})().catch(e=>{console.error(e);process.exitCode=1});
