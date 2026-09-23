'use strict'
const { test } = require('node:test')
const assert = require('node:assert/strict')
const http = require('node:http')
const { once } = require('node:events')
const { spawn } = require('node:child_process')
const fs = require('node:fs/promises')
const os = require('node:os')
const path = require('node:path')
const { ApiClient, HeyGhError } = require('..')
const HEAD = 'a'.repeat(40), BASE = 'b'.repeat(40), NEW = 'c'.repeat(40)
const ID = '1'.repeat(64)
const fixture = path.resolve(__dirname, '../../../../../target/debug/examples/node_test_daemon' + (process.platform === 'win32' ? '.exe' : ''))
const commit = sha => ({ sha, html_url: `https://github.com/acme/demo/commit/${sha}`, author: { login: 'dev' }, commit: { message: 'commit', author: { date: '2026-09-19T00:00:00Z' } }, parents: [] })
async function listen(server) {
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  return `http://127.0.0.1:${server.address().port}/`
}
async function close(server) {
  server.closeAllConnections()
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
}
async function daemon(database, upstream, address = '127.0.0.1:0') {
  const child = spawn(fixture, [database, upstream, address], { stdio: ['pipe', 'pipe', 'pipe'] })
  let output = '', diagnostic = ''
  child.stderr.on('data', data => { diagnostic += data })
  const server = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { child.kill(); reject(new Error('fixture startup timed out')) }, 15000)
    child.once('error', error => { clearTimeout(timer); reject(error) })
    child.once('exit', code => { clearTimeout(timer); reject(new Error(`fixture exited ${code}: ${diagnostic}`)) })
    child.stdout.on('data', data => {
      output += data
      if (output.includes('\n')) {
        clearTimeout(timer)
        try { resolve(JSON.parse(output.split('\n')[0]).server) } catch (error) { reject(error) }
      }
    })
  })
  return { server, async stop() {
    if (child.exitCode !== null || child.signalCode !== null) return
    const done = once(child, 'exit')
    const timer = setTimeout(() => child.kill(), 10000)
    child.stdin.end('\n')
    try { const [code, signal] = await done; assert.equal(code, 0, `fixture stopped by ${signal}: ${diagnostic}`) } finally { clearTimeout(timer) }
  } }
}
function isCode(code) { return error => error instanceof HeyGhError && error.code === code }

test('native SDK talks to the authenticated Rust daemon', { timeout: 90000 }, async t => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'hey-gh-node-'))
  const database = path.join(directory, 'cache.sqlite')
  let phase = 0, delay = 0, calls = []
  const github = http.createServer(async (request, response) => {
    const url = new URL(request.url, 'http://localhost')
    const p = url.pathname
    calls.push(p)
    let text = ''
    for await (const chunk of request) text += chunk
    const body = text ? JSON.parse(text) : {}
    const sha = phase ? NEW : HEAD
    let value
    if (p === '/repos/acme/demo') value = { default_branch: 'main' }
    else if (p === '/user') value = { login: 'me' }
    else if (p === '/repos/acme/demo/pulls') value = [{ number: 7, user: { login: 'me' }, state: 'open', head: { ref: 'topic/one', repo: { full_name: 'acme/demo' } }, base: { ref: 'main' } }]
    else if (p === '/repos/acme/demo/pulls/7') value = { number: 7, title: 'Unicode 🦀', state: 'open', head: { sha, ref: 'topic/one', repo: { full_name: 'acme/demo' } }, base: { sha: BASE, ref: 'main' }, merge_commit_sha: null, mergeable: phase ? false : true, requested_reviewers: [{ login: 'reviewer' }] }
    else if (p === '/repos/acme/demo/issues/7/comments') {
      if (!url.searchParams.has('page')) response.setHeader('link', `<http://${request.headers.host}${p}?per_page=100&page=2>; rel="next"`)
      value = url.searchParams.has('page') ? [{ id: 4294967297, body: `reply ${phase}`, user: null }] : [{ id: 1, body: 'Hello 🦀', nullable: null }]
    } else if (p === '/repos/acme/demo/pulls/7/comments') value = []
    else if (p === '/repos/acme/demo/pulls/7/reviews') value = [{ id: 4, state: 'APPROVED', user: { login: 'reviewer', type: 'User' }, submitted_at: '2026-09-19T00:00:00Z' }]
    else if (p.endsWith('/timeline')) value = []
    else if (p.endsWith('/check-runs')) {
      if (delay) await new Promise(resolve => setTimeout(resolve, delay))
      value = { check_runs: [{ id: 5, name: 'tests', head_sha: sha, app: { id: 42 }, status: 'completed', conclusion: phase ? 'failure' : 'success', details_url: 'https://github.com/acme/demo/checks/5' }] }
    } else if (p.endsWith('/status')) value = { statuses: [{ id: 6, context: 'external', state: 'success' }] }
    else if (p === '/repos/acme/demo/actions/runs') value = { workflow_runs: [] }
    else if (p.endsWith('/protection/required_status_checks')) value = { strict: false, checks: [{ context: 'tests', app_id: 42 }], contexts: [] }
    else if (p.includes('/rules/branches/')) value = []
    else if (p === '/repos/acme/demo/branches') value = [{ name: 'main', commit: { sha } }]
    else if (p.includes('/branches/')) value = { name: 'main', commit: { sha }, protected: true }
    else if (p.includes('/compare/')) {
      const newSha = p.split('...')[1]
      value = { status: 'ahead', total_commits: 1, merge_base_commit: { sha: BASE }, commits: [commit(newSha)], html_url: 'https://github.com/acme/demo/compare/old...new' }
    } else if (p.includes('/commits/')) value = commit(p.split('/').at(-1))
    else if (p === '/graphql') {
      const connection = { nodes: [], pageInfo: { hasNextPage: false, endCursor: null } }
      value = body.query.includes('MyOpenPullRequests')
        ? { data: { viewer: { pullRequests: { totalCount: 1, nodes: [{ number: 7, title: 'Unicode 🦀', state: 'OPEN', url: 'https://github.com/acme/demo/pull/7', repository: { nameWithOwner: 'acme/demo' } }], pageInfo: connection.pageInfo } } } }
        : { data: { repository: { pullRequest: body.query.includes('ReviewEvents') ? { timelineItems: connection } : { reviewThreads: connection } } } }
    } else { response.writeHead(404); response.end(JSON.stringify({ message: `unexpected ${p}` })); return }
    const etag = `"${Buffer.from(JSON.stringify(value)).toString('base64')}"`
    response.setHeader('etag', etag)
    if (request.headers['if-none-match'] === etag) { response.writeHead(304); response.end(); return }
    response.setHeader('content-type', 'application/json')
    response.end(JSON.stringify(value))
  })
  const upstream = await listen(github)
  let processFixture
  t.after(async () => { try { if (processFixture) await processFixture.stop() } finally { await close(github); await fs.rm(directory, { recursive: true, force: true }) } })
  processFixture = await daemon(database, upstream)
  const api = new ApiClient(processFixture.server)

  await t.test('CommonJS/ESM exports and JSON payload conversion', async () => {
    const esm = await import('../index.mjs')
    assert.equal(esm.ApiClient, ApiClient)
    assert.equal(esm.HeyGhError, HeyGhError)
    const pr = await api.prReport('acme/demo', 7, { refresh: true })
    assert.equal(pr.complete, true)
    assert.equal(pr.data.pull_request.title, 'Unicode 🦀')
    assert.equal(pr.data.conflicts, 'clean')
    assert.equal(pr.data.comments.length, 2)
    assert.equal(pr.data.comments[0].nullable, null)
    assert.equal(pr.data.comments[1].id, 4294967297)
    assert.deepEqual(pr.data.review_status.approved_by, ['reviewer'])
    assert.equal(pr.data.ci.summary.state, 'success')
  })
  await t.test('all SDK reads, watches, and literal branch encoding', async () => {
    assert.equal((await api.ciForPr('acme/demo', 7)).data.head_sha, HEAD)
    const policy = await api.requiredChecksForPr('acme/demo', 7)
    assert.equal(policy.state, 'satisfied')
    assert.equal(policy.base_sha, HEAD)
    assert.equal(policy.pr_base_sha, BASE)
    assert.equal(policy.merge_sha, null)
    assert.equal((await api.myPullRequests('acme/demo')).length, 1)
    assert.equal((await api.listPullRequests('acme/demo', 'all')).length, 1)
    const repo = await api.repositoryReport('acme/demo', { branches: ['topic/one'], allBranches: true, read: { refresh: true } })
    assert.deepEqual(repo.errors, [])
    assert.equal(repo.branches.length, 2)
    assert.ok(calls.some(p => p.includes('topic%2Fone')))
    const watch = await api.watch('acme/demo', { pullNumber: 7, intervalSeconds: 10 })
    const branchWatch = await api.watchRepository('acme/demo', { branches: ['topic/one'], intervalSeconds: 10 })
    assert.equal(watch.kind, 'pull_requests')
    assert.equal(branchWatch.kind, 'branches')
    assert.equal((await api.watches()).length, 2)
    assert.equal(await api.unwatch(watch.id), undefined)
    await api.unwatch(branchWatch.id)
    assert.deepEqual(await api.watches(), [])
  })
  await t.test('cached-only reads make no GitHub calls', async () => {
    const before = (await api.status()).network_requests
    const ci = await api.ciForPr('acme/demo', 7, { cachedOnly: true })
    assert.equal(ci.complete, true)
    assert.equal((await api.status()).network_requests, before)
  })
  await t.test('parallel native Promises leave the JavaScript event loop responsive', async () => {
    delay = 100
    let ticks = 0
    const timer = setInterval(() => ticks++, 5)
    try { await Promise.all([api.ciForPr('acme/demo', 7, { refresh: true }), api.status(), new ApiClient(processFixture.server).status()]) }
    finally { clearInterval(timer); delay = 0 }
    assert.ok(ticks > 2)
  })
  let baseline
  await t.test('account-wide status has typed replacements and its own cursor', async () => {
    await assert.rejects(api.prStatus({ read: { cachedOnly: true } }), isCode('cache_miss'))
    const initial = await api.prStatus({ read: { refresh: true } })
    assert.equal(initial.pullRequests.length, 1)
    assert.equal(initial.pullRequests[0].ci.summary.state, 'success')
    assert.ok(initial.cursor.startsWith('pr1:'))
    const watch = await api.watchAccount(30)
    assert.equal(watch.kind, 'account')
    await api.unwatch(watch.id)
    const updates = await api.prStatus({ cursor: initial.cursor, read: { cachedOnly: true } })
    assert.deepEqual(updates.pullRequests, [])
    await assert.rejects(api.prStatus({ cursor: initial.cursor, repository: 'acme/demo', read: { cachedOnly: true } }), isCode('invalid'))
    const projected = await api.prStatus({ fields: ['number', 'complete', 'number'], read: { cachedOnly: true } })
    assert.equal(projected.cursor, initial.cursor)
    assert.deepEqual(Object.keys(projected.pullRequests[0]).sort(), ['complete', 'number', 'sourceErrors'])
    assert.equal(projected.pullRequests[0].complete, initial.pullRequests[0].complete)
    assert.deepEqual(projected.pullRequests[0].sourceErrors, initial.pullRequests[0].sourceErrors)
    const before = (await api.status()).network_requests
    await assert.rejects(api.prStatus({ fields: [], read: { refresh: true } }), isCode('invalid'))
    await assert.rejects(api.prStatus({ fields: ['unknown'], read: { refresh: true } }), isCode('invalid'))
    assert.equal((await api.status()).network_requests, before)
    // Cancelled monitors may leave coalesced operations finishing in the queue.
    for (let i = 0; i < 100 && (await api.status()).outstanding_requests; i++) await new Promise(resolve => setTimeout(resolve, 10))
  })
  await t.test('bootstrap, long polling, and changed PR/commit snapshots', async () => {
    baseline = await api.bootstrap()
    const waiting = api.changes({ cursor: baseline.cursor, limit: 100, waitSeconds: 2 })
    await new Promise(resolve => setTimeout(resolve, 30))
    phase = 1
    const pr = await api.prReport('acme/demo', 7, { refresh: true })
    assert.equal(pr.data.conflicts, 'conflicting')
    assert.equal(pr.data.ci.summary.state, 'failure')
    const page = await waiting
    assert.ok(page.changes.length > 0)
    const repo = await api.repositoryReport('acme/demo', { read: { refresh: true } })
    assert.equal(repo.branches[0].transition.old_sha, HEAD)
    assert.equal(repo.branches[0].transition.new_sha, NEW)
    const all = await api.changes({ cursor: baseline.cursor })
    assert.ok(all.changes.some(c => c.resource.startsWith('comments://')))
    assert.ok(all.changes.some(c => c.resource.startsWith('branch://')))
    assert.ok(all.changes.some(c => c.resource.endsWith(NEW)))
    assert.deepEqual(await api.changes({ cursor: baseline.cursor }), all)
  })
  await t.test('existing client reloads credentials and resumes its cursor after restart', async () => {
    const snapshot = await api.bootstrap()
    const server = processFixture.server
    await processFixture.stop()
    processFixture = await daemon(database, upstream, `127.0.0.1:${new URL(server).port}`)
    assert.equal(processFixture.server, server)
    assert.ok((await api.status()).queue_capacity > 0)
    assert.equal((await api.bootstrap()).cursor, snapshot.cursor)
    assert.deepEqual((await api.changes({ cursor: snapshot.cursor })).changes, [])
  })
  await t.test('invalid inputs are rejected before any daemon or GitHub requests', async () => {
    const before = (await api.status()).network_requests
    for (const number of [-1, 0, 1.5, NaN, Infinity, 2 ** 53]) await assert.rejects(api.ciForPr('acme/demo', number), isCode('invalid'))
    for (const options of [{ refresh: true, cachedOnly: true }, { maxAgeSeconds: -1 }, { maxAgeSeconds: 86401 }, { maxAgeSeconds: 1.5 }]) await assert.rejects(api.prReport('acme/demo', 7, options), isCode('invalid'))
    await assert.rejects(api.ciForPr('bad/repo/escape', 7), isCode('invalid'))
    await assert.rejects(api.listPullRequests('acme/demo', 'merged'), isCode('invalid'))
    await assert.rejects(api.repositoryReport('acme/demo', { branches: ['../escape'] }), isCode('invalid'))
    await assert.rejects(api.watch('acme/demo', { intervalSeconds: -1 }), isCode('invalid'))
    for (const options of [{ limit: 0 }, { limit: 1001 }, { waitSeconds: 31 }, { waitSeconds: 0.5 }]) await assert.rejects(api.changes(options), isCode('invalid'))
    await assert.rejects(api.unwatch('bad-id'), isCode('invalid'))
    assert.equal((await api.status()).network_requests, before)
    for (const server of ['https://127.0.0.1/', 'http://github.com/', 'http://user:pass@127.0.0.1/', 'http://127.0.0.1/path']) assert.throws(() => new ApiClient(server), isCode('invalid'))
  })
  await t.test('expired cursor requires a new bootstrap', async () => {
    const old = (await api.bootstrap()).cursor
    for (let i = 2; i < 21; i++) { phase = i; await api.prReport('acme/demo', 7, { refresh: true }) }
    await assert.rejects(api.changes({ cursor: old }), isCode('cursor_expired'))
    await assert.rejects(api.changes({ cursor: 'foreign.0' }), isCode('invalid'))
    const snapshot = await api.bootstrap()
    assert.deepEqual((await api.changes({ cursor: snapshot.cursor })).changes, [])
  })
})

test('structured daemon errors preserve recovery and retry information', async t => {
  let code = 'rate_limited', status = 503
  let upstreamStatus
  let cause = 'source diagnostic', authHostname = 'github.example.com', errorMessage = 'synthetic error'
  const server = http.createServer((_request, response) => {
    response.writeHead(status, { 'content-type': 'application/json', 'retry-after': '2' })
    response.end(JSON.stringify({ code, error: errorMessage, upstream_status: upstreamStatus, upstream_message: 'upstream diagnostic', cause, auth_hostname: authHostname }))
  })
  const origin = await listen(server)
  t.after(async () => { if (server.listening) await close(server) })
  const api = new ApiClient(origin)
  await assert.rejects(api.status(), error => isCode('rate_limited')(error) && error.retryAfterSeconds === 2)
  for (const next of ['queue_full', 'deadline', 'cache_miss', 'local_auth', 'invalid']) { code = next; await assert.rejects(api.status(), isCode(code)) }
  code = 'github'; status = 403
  await assert.rejects(api.status(), error => isCode('github')(error) && error.httpStatus === 403)
  code = 'upstream'; status = 502
  for (const value of [302, 403, 503]) {
    upstreamStatus = value
    await assert.rejects(api.status(), error => isCode('github')(error) && error.httpStatus === value && error.message === `GitHub returned HTTP ${value}: upstream diagnostic`)
  }
  for (const value of [null, '403', 200, 999, -1, 4.2]) {
    upstreamStatus = value
    await assert.rejects(api.status(), error => isCode('github')(error) && error.httpStatus === 502 && error.message.includes('synthetic error'))
  }
  upstreamStatus = undefined
  status = 502
  for (const next of ['graphql', 'graphql_access_denied']) {
    code = next
    await assert.rejects(api.status(), error => isCode(code)(error) && error.httpStatus === undefined && !error.message.includes('HTTP 502'))
  }
  for (const next of ['auth', 'storage', 'transport', 'stopped']) {
    code = next
    await assert.rejects(api.status(), error => isCode(code)(error) && error.httpStatus === undefined)
  }
  cause = undefined; authHostname = undefined
  for (const [next, message] of [
    ['auth', 'GitHub authentication unavailable; run gh auth login --hostname github.example.com'],
    ['storage', 'cache storage error: cache locked'],
    ['transport', 'GitHub transport error: connection stalled']
  ]) {
    code = next; errorMessage = message
    await assert.rejects(api.status(), error => isCode(code)(error) && error.message === message && error.httpStatus === undefined)
  }
  code = 'auth'; errorMessage = 'missing authentication detail'
  await assert.rejects(api.status(), error => isCode('invalid')(error) && error.message.includes('missing authentication detail'))
  await close(server)
  await assert.rejects(api.status(), isCode('transport'))
})
