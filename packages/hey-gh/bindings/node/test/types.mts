import { ApiClient, HeyGhError, type CiObservation, type ChangePage, type RepositoryReport, type Report } from '../index.js'
const api = new ApiClient()
const ci: Promise<CiObservation> = api.ciForPr('acme/demo', 7, { refresh: true })
const pr: Promise<Report> = api.prReport('acme/demo', 7, { cachedOnly: true })
const repo: Promise<RepositoryReport> = api.repositoryReport('acme/demo', { branches: ['topic/one'], read: { maxAgeSeconds: 30 } })
const changes: Promise<ChangePage> = api.changes({ cursor: 'opaque', limit: 100, waitSeconds: 30 })
void [ci, pr, repo, changes, api.watch('acme/demo', { pullNumber: 7 }), api.watchRepository('acme/demo'), api.listPullRequests('acme/demo', 'all'), api.requiredChecksForPr('acme/demo', 7)]
const error = new HeyGhError('retry', 'rate_limited', { retryAfterSeconds: 2 })
const delay: number | undefined = error.retryAfterSeconds
void delay
void [api.prStatus(),api.prStatus({ cursor: 'opaque', repository: 'acme/demo', read: { cachedOnly: true } }),api.watchAccount(30)]
// @ts-expect-error Invalid PR state.
api.listPullRequests('acme/demo', 'merged')
// @ts-expect-error Native numeric inputs are JavaScript numbers, not bigint.
api.ciForPr('acme/demo', 7n)
// @ts-expect-error Read flags must be boolean.
api.prReport('acme/demo', 7, { refresh: 'yes' })

const activeRequests: number = (await api.status()).active_requests
const maxActiveRequests: number = (await api.status()).max_active_requests
void [activeRequests, maxActiveRequests]

const selected = await api.prStatus({ fields: ['number', 'state'] as const })
const selectedNumber: number = selected.pullRequests[0].number
const selectedHealth: boolean = selected.changes[0].pullRequest.complete
void [selectedNumber, selectedHealth]
// @ts-expect-error Omitted CI is unknown in projected rows.
selected.pullRequests[0].ci
// @ts-expect-error Unsupported transport fields are rejected.
api.prStatus({ fields: ['unknown'] })
const fullState = (await api.prStatus()).pullRequests[0].ci
void fullState
