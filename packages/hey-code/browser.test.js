import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve, extname, sep } from 'node:path';
import { chromium, webkit } from 'playwright';

const root = fileURLToPath(new URL('.', import.meta.url));
const output = resolve(root, '../../out/hey-code');
await mkdir(output, { recursive: true });
const server = createServer(async (req, res) => {
  try {
    const path = resolve(root, '.' + new URL(req.url, 'http://localhost').pathname);
    if (!path.startsWith(root.endsWith(sep) ? root : root + sep)) throw Error('path');
    const content = await readFile(path);
    res.setHeader('Content-Type', ({ '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.html': 'text/html' })[extname(path)] ?? 'text/plain');
    res.end(content);
  } catch { res.writeHead(404); res.end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}/fixture.html`;
const browsers = [];
try {
  for (const engine of ['chromium', 'webkit']) {
    const browser = await (engine === 'chromium' ? chromium.launch({ channel: 'chrome' }) : webkit.launch());
    browsers.push(browser);
    const page = await browser.newPage({ viewport: { width: 1280, height: 1000 } });
    page.setDefaultTimeout(10000);
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(url);
    const main = page.locator('body > hey-code');
    await page.waitForFunction(() => !!window.view?.chat);
    await main.locator('h1').first().waitFor();
    await page.screenshot({ path: `${output}/${engine}-desktop.png` });
    const input = page.getByRole('textbox', { name: 'Message', exact: true }).first();
    await input.fill('first draft');
    await page.reload(); await page.waitForFunction(() => !!window.view?.chat);
    assert.equal(await input.inputValue(), 'first draft', 'draft should survive reload');
    await page.evaluate(() => window.fail = true);
    await input.press('Enter');
    await page.getByRole('alert').filter({ hasText: 'Host is disconnected' }).waitFor();
    assert.equal(await input.inputValue(), 'first draft', 'failed send lost draft');
    await page.evaluate(() => { window.fail = false; window.hold = true; window.calls = []; });
    await input.press('Enter'); await input.press('Enter');
    assert.equal(await page.evaluate(() => calls.length), 1, 'duplicate send while pending');
    assert.equal(await page.evaluate(() => calls[0].delivery), 'steer');
    await input.fill('typed during send');
    await page.evaluate(() => { window.hold = false; window.release(); });
    await page.waitForFunction(() => !view.shadowRoot.querySelector('.send').disabled);
    assert.equal(await input.inputValue(), 'typed during send');
    await page.getByRole('combobox', { name: 'Reply delivery' }).first().selectOption('queue');
    await input.press('Enter');
    await page.waitForFunction(() => view.shadowRoot.querySelector('textarea[aria-label="Message"]').value === '');
    assert.equal(await page.evaluate(() => calls.at(-1).delivery), 'queue');

    // A completion from a previous conversation cannot clear the new draft or show its error.
    await page.evaluate(() => { window.hold = true; window.calls = []; });
    await input.fill('old thread send'); await input.press('Enter');
    await page.evaluate(() => { view.chat = { ...initial(), id: 'other', provider: 'claude', status: 'completed', canSteer: false }; });
    await input.fill('other draft');
    await page.evaluate(() => { window.hold = false; window.release(); });
    assert.equal(await input.inputValue(), 'other draft');
    assert.equal(await page.evaluate(() => calls[0].threadId), 'example-codex');
    await input.press('Enter');
    await page.waitForFunction(() => calls.length === 2);
    assert.equal(await page.evaluate(() => calls[1].delivery), 'send');
    assert.equal(await page.evaluate(() => calls[1].threadId), 'other');

    await page.evaluate(() => view.chat = initial());
    assert.equal(await input.inputValue(), '', 'accepted reply left a stale draft on the previous thread');
    await page.getByRole('button', { name: 'Pause', exact: true }).first().click();
    await page.getByRole('button', { name: 'Resume', exact: true }).first().waitFor();
    assert.equal(await page.evaluate(() => view.chat.goal.status), 'paused');
    await page.getByRole('button', { name: 'Resume', exact: true }).first().click();
    await page.getByRole('button', { name: 'Edit', exact: true }).first().click();
    const objective = page.getByRole('textbox', { name: 'Goal objective' });
    await objective.fill('Keep my edited goal');
    await page.evaluate(() => view.chat = { ...view.chat, goal: { ...view.chat.goal, tokensUsed: 20000 } });
    assert.equal(await objective.inputValue(), 'Keep my edited goal', 'stream update reset goal edit');
    await page.getByRole('button', { name: 'Start goal' }).click();
    await page.waitForFunction(() => view.chat.goal.objective === 'Keep my edited goal');
    await page.getByRole('button', { name: 'Complete', exact: true }).first().click();
    await page.waitForFunction(() => view.chat.goal.status === 'complete');
    await page.getByRole('button', { name: 'Edit', exact: true }).first().click();
    await objective.fill('Unsent objective from the previous chat');
    await page.evaluate(() => view.chat = { ...initial(), id: 'new-goal-chat', goal: null });
    assert.equal(await objective.count(), 0, 'switching chat retained a goal editor for the old chat');
    await page.getByRole('button', { name: '◎ Set a goal' }).first().click();
    await objective.fill('New goal');
    await page.getByRole('button', { name: 'Start goal' }).click();
    assert.equal(await page.evaluate(() => calls.at(-1).threadId), 'new-goal-chat');
    await page.evaluate(() => view.chat = initial());

    await page.evaluate(() => view.chat = { ...view.chat, requests: [
      { id: 'ask', kind: 'input', title: 'Choose a target', questions: [{ id: 'target', question: 'Which platform?', options: [{ label: 'macOS' }, { label: 'Linux' }] }] },
      { id: 'approve', kind: 'approval', title: 'Run command', payload: { command: 'pwd' } },
    ] });
    await page.getByRole('button', { name: 'macOS', exact: true }).click();
    await page.evaluate(() => view.chat = { ...view.chat, status: 'waiting' });
    await page.getByRole('button', { name: 'Submit answer' }).click();
    assert.deepEqual(await page.evaluate(() => calls.at(-1).answers), { target: 'macOS' });
    await page.getByRole('button', { name: 'Deny', exact: true }).click();
    assert.equal(await page.evaluate(() => calls.at(-1).allow), false);
    await page.getByRole('button', { name: 'Stop', exact: true }).first().click();
    assert.equal(await page.evaluate(() => view.chat.status), 'interrupted');

    await main.locator('summary').filter({ hasText: 'Check transcript recovery' }).click();
    const childInput = main.locator('hey-code').getByRole('textbox', { name: 'Message', exact: true });
    await childInput.fill('Please check again'); await childInput.press('Enter');
    assert.equal(await page.evaluate(() => calls.at(-1).threadId), 'example-claude');
    await main.locator('summary').filter({ hasText: 'Check transcript recovery' }).click();

    // Escape arbitrary transcript HTML and unsafe links; no remote image fetches.
    await page.evaluate(() => view.chat = { ...view.chat, items: [{ id: 'unsafe', type: 'message', role: 'assistant', text: '<img src=x onerror="window.pwned=true"><video src=x autoplay></video><iframe src=x></iframe><script>window.pwned=true</script>[bad](javascript:alert(1)) **Safe**' }] });
    assert.equal(await main.locator('.markdown img, .markdown script, .markdown video, .markdown iframe, .markdown a[href^="javascript:"]').count(), 0);
    assert.equal(await page.evaluate(() => !!window.pwned), false);

    // Long transcripts retain the reader's position while new text streams.
    await page.evaluate(() => view.chat = { ...initial(), subagents: [], items: Array.from({ length: 100 }, (_, i) => ({ id: String(i), type: 'message', role: 'assistant', text: `Message ${i}\n\n${'Details. '.repeat(20)}` })) });
    await page.evaluate(() => { const s = view.shadowRoot.querySelector('.scroll'); s.scrollTop = 500; });
    await page.waitForTimeout(100);
    const before = await page.evaluate(() => view.shadowRoot.querySelector('.scroll').scrollTop);
    await page.evaluate(() => emit({ TextDelta: { text: 'New output' } }));
    assert.equal(await page.evaluate(() => view.shadowRoot.querySelector('.scroll').scrollTop), before);
    await page.getByRole('button', { name: '↓ Latest', exact: true }).click();
    assert.ok(await page.evaluate(() => { const s = view.shadowRoot.querySelector('.scroll'); return s.scrollHeight - s.scrollTop - s.clientHeight < 2; }));
    await page.evaluate(() => { view.chat = { ...view.chat, hasMore: true }; const s = view.shadowRoot.querySelector('.scroll'); s.scrollTop = 0; });
    await page.waitForTimeout(100);
    const oldHeight = await page.evaluate(() => view.shadowRoot.querySelector('.scroll').scrollHeight);
    await page.getByRole('button', { name: 'Load earlier messages' }).click();
    assert.ok(await page.evaluate(h => { const s = view.shadowRoot.querySelector('.scroll'); return Math.abs(s.scrollTop - (s.scrollHeight - h)) < 2; }, oldHeight));

    await page.evaluate(() => view.chat = { ...initial(), readOnly: 'This session belongs to another host.' });
    assert.equal(await page.getByRole('textbox', { name: 'Message', exact: true }).first().isDisabled(), true);
    assert.equal(await page.getByRole('button', { name: 'Pause', exact: true }).count(), 0);
    await page.evaluate(() => view.chat = initial());
    await page.setViewportSize({ width: 390, height: 844 });
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.screenshot({ path: `${output}/${engine}-phone.png` });
    assert.ok(await page.evaluate(() => [...view.shadowRoot.querySelectorAll('.composer,.goal,.scroll,header')].every(el => el.getBoundingClientRect().right <= innerWidth + 1)));
    assert.equal(errors.length, 0, errors.join('\n'));
    await browser.close();
    console.log(`${engine}: replies, drafts, goals, requests, subagents, history, streaming, isolation, and mobile passed`);
  }
} finally {
  await Promise.all(browsers.map(browser => browser.close()));
  await new Promise(resolve => server.close(resolve));
}
