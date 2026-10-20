// NODE_PATH must contain Playwright. Runs the real server and lookup on a temporary database.
const assert = require('node:assert/strict');
const {mkdtempSync, rmSync, mkdirSync, readFileSync} = require('node:fs');
const {tmpdir} = require('node:os');
const {resolve, join} = require('node:path');
const {spawn, execFileSync} = require('node:child_process');
const {chromium, webkit} = require('playwright');
const binary = resolve(process.argv[2] || 'target/debug/hey-boss');
const root = mkdtempSync(join(tmpdir(), 'hey-boss-filters-'));
const db = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:db, HEY_BOSS_FLEET_STATE:root, HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR']) delete env[key];
const run = args => execFileSync(binary, args, {env, cwd:root, encoding:'utf8'});
const sql = query => execFileSync('sqlite3', ['-cmd','.timeout 10000',db,query]);
let web;
(async () => {
  run(['issue','projects','--json']);
  sql("INSERT INTO projects(id,name,next_number) VALUES('named:Filter QA','Filter QA',1);");
  web = spawn(binary, ['issue','web','--port','0','--no-discovery','--project','named:Filter QA','--json'], {env,cwd:root,stdio:['ignore','pipe','pipe']});
  const info = await new Promise((yes,no) => {
    let output = '', errors = '';
    const timer = setTimeout(() => no(Error('Web startup timed out: '+errors)), 20000);
    web.stderr.on('data', chunk => { errors += chunk; });
    web.stdout.on('data', chunk => { output += chunk; if (output.includes('\n')) { clearTimeout(timer); try { yes(JSON.parse(output.split('\n')[0])); } catch(e) { no(e); } } });
    web.on('error', no);
    web.on('exit', code => { clearTimeout(timer); no(Error(`Web exited ${code}: ${errors}`)); });
  });
  const base = info.url.replace(/\/$/, '');
  const bootstrap = await (await fetch(base+'/api/bootstrap')).json();
  const api = async operation => {
    const value = await (await fetch(base+'/api/action', {method:'POST', headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':bootstrap.csrf},body:JSON.stringify({project:'named:Filter QA',operation})})).json();
    assert.equal(value.ok, true, JSON.stringify(value));
    return value;
  };
  const titles = [
    'Reconnect streaming agents after a dropped connection',
    'Keep queued work visible during reconnects',
    'Verify desktop deep links across machines',
    'Approve the release checklist',
    'Wait for the dependency review',
    'Review the retry policy before rollout',
    'Preserve filters in shared links',
    'Retire the old assignment selector',
  ];
  for (const [index,title] of titles.entries()) await api({action:'create',title,body:'Isolated filter verification.',labels:[index === 2 ? 'interface' : 'reliability', 'needs & review']});
  for (const number of [4,6]) await api({action:'block',number,comment:'Waiting for a decision.',force:false});
  for (const number of [5,6]) await api({action:'set_blockers',number,blockers:[3],force:false});
  await api({action:'close',number:7,force:false});
  await api({action:'delete',number:8,force:false});
  sql("UPDATE issues SET assignee='human:boss' WHERE number IN (1,2); UPDATE issues SET manual_blocked=1 WHERE number=6;");
  for (const [index,count] of [5,4,8,6,7,10,5,6].entries()) {
    for (let launch=0;launch<count;launch++) sql(`INSERT INTO issue_agent_launches VALUES('run-${index}-${launch}','named:Filter QA',${index+1},1);`);
  }
  const url = state => `${base}/#project=named%3AFilter+QA&state=${state}`;
  for (const engine of [chromium,webkit]) {
    const browser = await engine.launch({headless:true});
    try {
      const context = await browser.newContext({viewport:{width:1440,height:1000}});
      const page = await context.newPage();
      page.setDefaultTimeout(10000);
      const errors=[];
      page.on('pageerror', e => errors.push(e.message));
      await page.route('**/api/inbox', route => route.fulfill({json:{ok:true,tasks:[],unread:0}}));
      const rows = () => page.locator('#issue-list .issue-row').evaluateAll(items => items.map(i => Number(i.dataset.issueNumber)).sort((a,b) => a-b));
      const expect = async numbers => {
        await page.waitForFunction(numbers => JSON.stringify([...document.querySelectorAll('#issue-list .issue-row')].map(i => Number(i.dataset.issueNumber)).sort((a,b) => a-b)) === JSON.stringify(numbers), numbers);
        assert.deepEqual(await rows(), numbers);
        const value = JSON.parse(run(['lookup',page.url(),'--json']));
        assert.deepEqual(value.issues.map(i => i.number).sort((a,b) => a-b), numbers, 'lookup returns the same issues as the screen');
        assert.equal(value.total, numbers.length);
        assert.equal(await page.locator('#issue-filter-count').innerText(), `${numbers.length} ${numbers.length === 1 ? 'issue' : 'issues'}`);
      };
      const preset = key => page.locator(`[data-preset="${key}"]`);
      const choose = async (category, value) => {
        await page.getByRole('button',{name:'Add filter',exact:true}).click();
        await page.locator(`[data-filter-category="${category}"]`).click();
        await page.locator(`[data-filter-key="${category}"][data-filter-value="${value}"]`).click();
      };
      await page.goto(url('open'));
      await expect([1,2,3]);
      await preset('min_launches').click(); await expect([1,3]);
      await preset('owner').click(); await expect([1]);
      sql("INSERT INTO issue_agent_launches VALUES('new-launch','named:Filter QA',2,1);");
      await page.locator('#refresh').click(); await expect([1,2]);
      sql("DELETE FROM issue_agent_launches WHERE run_id='new-launch';");
      await page.locator('#refresh').click(); await expect([1]);
      assert.equal(await preset('owner').getAttribute('aria-pressed'), 'true');
      await choose('label','reliability'); await expect([1]);
      await page.locator('#issue-search').fill('streaming'); await expect([1]);
      await page.waitForFunction(() => model.route.search === 'streaming');
      await page.reload(); await expect([1]);
      await page.locator('.issue-title').click();
      await page.locator('#detail-view .issue-detail-title, #detail-view h1').waitFor();
      await page.getByRole('button',{name:'All issues',exact:true}).click(); await expect([1]);
      await page.locator('#clear-issue-filters').click(); await expect([1,2,3]);
      await page.goBack(); await expect([1]);
      await page.goForward(); await expect([1,2,3]);

      // Search directly across filter categories, including escaped labels.
      await page.getByRole('button',{name:'Add filter',exact:true}).click();
      await page.getByRole('searchbox',{name:'Search filters'}).fill('needs & review');
      await page.getByRole('searchbox',{name:'Search filters'}).press('ArrowDown');
      await page.keyboard.press('Enter'); await expect([1,2,3]);
      assert.equal(new URL(page.url()).hash.includes('needs+%26+review'), true);
      await page.getByRole('button',{name:'Remove Label filter',exact:true}).click();
      await page.getByRole('button',{name:'Add filter',exact:true}).click();
      await page.keyboard.press('Escape');
      assert.equal(await page.locator('#add-issue-filter').evaluate(el => el === document.activeElement), true);
      assert.equal(await page.locator('#issue-filter-menu').evaluate(el => el.matches(':popover-open')), false);

      await page.getByRole('tab',{name:/Blocked/}).click(); await expect([4,5,6]);
      await preset('blocked').click(); await expect([4,6]);
      await preset('min_launches').click(); await expect([4,6]);
      await preset('owner').click(); await expect([]);
      await preset('owner').click(); await expect([4,6]);
      await choose('blocked','dependencies'); await expect([5,6]);
      assert.equal(await preset('blocked').getAttribute('aria-pressed'), 'false');
      await choose('min_launches','10'); await expect([6]);
      await page.getByRole('tab',{name:/Open/}).click(); await expect([]);
      assert.equal(await preset('blocked').count(), 0);
      await page.getByRole('button',{name:'Clear filters',exact:true}).click(); await expect([1,2,3]);

      for (const [state,launches,expected] of [['all',[1,3,4,5,6,7],[1]],['closed',[7],[]],['deleted',[8],[]]]) {
        await page.goto(url(state));
        await preset('min_launches').click();
        await expect(launches);
        await preset('owner').click();
        await expect(expected);
      }
      await page.goto(url('blocked'));
      await preset('blocked').click(); await preset('min_launches').click(); await choose('label','reliability');
      await expect([4,6]);
      await page.evaluate(() => Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text => window.copiedFilter=text}}));
      await page.locator('#copy-list-command').click();
      const copied = await page.evaluate(() => window.copiedFilter);
      assert.match(copied, /^hey-boss lookup '/);
      for (const part of ['min_launches=5','blocked=hold','label=reliability']) assert.ok(copied.includes(part), copied);
      const commandResult = execFileSync('/bin/sh',['-c',copied.replace('hey-boss ', '"'+binary+'" ')+' --json'],{env,cwd:root,encoding:'utf8'});
      assert.deepEqual(JSON.parse(commandResult).issues.map(i=>i.number).sort((a,b)=>a-b),[4,6]);

      for (const scheme of ['light','dark']) for (const width of [1440,768,390,320]) {
        await page.emulateMedia({colorScheme:scheme,reducedMotion:'reduce'});
        await page.setViewportSize({width,height:1000});
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${engine.name()} ${scheme}/${width}: no horizontal overflow`);
        for (const key of ['min_launches','owner','blocked']) {
          const bounds = await preset(key).boundingBox();
          assert.ok(bounds && bounds.x >= 0 && bounds.x+bounds.width <= width && bounds.height >= 30);
        }
        await page.getByRole('button',{name:'Add filter',exact:true}).click();
        const menu = await page.locator('#issue-filter-menu').boundingBox();
        assert.ok(menu.x >= 0 && menu.x+menu.width <= width && menu.y >= 0 && menu.y+menu.height <= 1000, `${width}: filter menu fits`);
        if (process.env.FILTER_SCREENSHOTS) {
          mkdirSync(process.env.FILTER_SCREENSHOTS,{recursive:true});
          await page.screenshot({path:join(process.env.FILTER_SCREENSHOTS,`${engine.name()}-${scheme}-${width}-menu.png`)});
        }
        await page.keyboard.press('Escape');
        if (process.env.FILTER_SCREENSHOTS) await page.screenshot({path:join(process.env.FILTER_SCREENSHOTS,`${engine.name()}-${scheme}-${width}.png`)});
      }
      // The phone hub ships these same files with a different page shell.
      execFileSync(process.execPath, ['mobile/sync-issue-web.mjs'], {cwd:resolve('.')});
      await page.route('**/issue-web/*', route => {
        const name = new URL(route.request().url()).pathname.split('/').pop();
        route.fulfill({body:readFileSync(resolve('mobile/public/issue-web',name)),contentType:name.endsWith('.js')?'application/javascript':name.endsWith('.css')?'text/css':'image/png'});
      });
      await page.route('**/issues', route => route.fulfill({body:readFileSync('mobile/public/issue-web/index.html'),contentType:'text/html'}));
      await page.goto(`${base}/issues#project=named%3AFilter+QA&state=blocked`);
      await preset('blocked').click(); await preset('min_launches').click(); await expect([4,6]);
      for (const width of [390,320]) {
        await page.setViewportSize({width,height:900});
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, 'Phone shell fits');
        await choose('label','reliability'); await expect([4,6]);
        if (process.env.FILTER_SCREENSHOTS) await page.screenshot({path:join(process.env.FILTER_SCREENSHOTS,`${engine.name()}-phone-${width}.png`)});
      }
      assert.deepEqual(errors, [], 'No browser exceptions');
      console.log(`${engine.name()}: presets, combinations, lookup, history, keyboard, copy command, light/dark and four widths passed`);
      await context.close();
    } catch (error) {
      const page = browser.contexts()[0]?.pages()[0];
      if (page) {
        await page.screenshot({path:join(tmpdir(),'hey-boss-filter-failure.png')});
        console.error(await page.locator('body').innerText());
      }
      throw error;
    } finally { await browser.close(); }
  }
})().catch(e => { console.error(e); process.exitCode=1; }).finally(async () => {
  if (web && web.exitCode === null) { const ended = new Promise(r => web.once('exit',r)); web.kill(); await ended; }
  rmSync(root,{recursive:true,force:true});
});
