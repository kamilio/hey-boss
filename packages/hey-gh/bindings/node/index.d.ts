export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }
export type GitHubRecord = { [key: string]: JsonValue }
export type CiState = 'success' | 'failure' | 'running' | 'skipped' | 'unknown'
export type PolicyState = 'satisfied' | 'failure' | 'pending' | 'missing' | 'unknown' | 'not_required'
export type ErrorCode = 'invalid' | 'auth' | 'local_auth' | 'storage' | 'transport' | 'github' | 'graphql' | 'graphql_access_denied' | 'queue_full' | 'deadline' | 'rate_limited' | 'cursor_expired' | 'cache_miss' | 'stopped'

export interface ReadOptions {
  refresh?: boolean
  cachedOnly?: boolean
  maxAgeSeconds?: number
}
export interface RepositoryOptions {
  branches?: string[]
  allBranches?: boolean
  read?: ReadOptions
}
export interface WatchOptions {
  /** Omit or use zero to discover your open PRs. */
  pullNumber?: number
  intervalSeconds?: number
}
export interface RepositoryWatchOptions {
  branches?: string[]
  allBranches?: boolean
  intervalSeconds?: number
}
export interface ChangesOptions {
  cursor?: string
  limit?: number
  waitSeconds?: number
}
export interface PrStatusOptions<K extends keyof PrStatus = keyof PrStatus> extends ChangesOptions {
  /** Select row fields on the HTTP transport; health fields are always retained. */
  fields?: readonly K[]
  repository?: string
  read?: ReadOptions
}
export type PrActivityKind = 'commits_pushed' | 'conflicts_changed' | 'ci_changed' | 'review_status_changed' | 'required_checks_changed' | 'comment_added' | 'comment_edited' | 'comment_deleted' | 'review_comment_added' | 'review_comment_edited' | 'review_comment_deleted' | 'review_submitted' | 'review_changed' | 'review_deleted' | 'thread_resolved' | 'thread_reopened' | 'closed' | 'merged' | 'reopened' | 'removed'
export interface PrActivity { kind: PrActivityKind; [field: string]: JsonValue }
export interface PrStatus {
  id?: string
  number: number
  title: string
  additions?: number | null
  deletions?: number | null
  body?: string | null
  labels?: GitHubRecord[] | null
  assignees?: GitHubRecord[] | null
  url: string
  state: 'OPEN' | 'CLOSED' | 'MERGED' | 'UNKNOWN'
  repository: { nameWithOwner: string }
  author: { login: string } | null
  isDraft: boolean
  createdAt: string
  updatedAt: string
  mergedAt: string | null
  closedAt: string | null
  headRefName: string | null
  headRefOid: string
  baseRefName: string
  baseRefOid: string | null
  mergeable: 'MERGEABLE' | 'CONFLICTING' | 'UNKNOWN'
  mergeStateStatus: string | null
  reviewDecision: string | null
  conflicts: 'clean' | 'conflicting' | 'unknown'
  ci: CiReport | null
  comments: GitHubRecord[] | null
  reviewComments: GitHubRecord[] | null
  reviews: GitHubRecord[] | null
  reviewThreads: GitHubRecord[] | null
  reviewStatus: ReviewStatus | null
  requiredChecks: RequiredChecksReport | null
  statusCheckRollup: GitHubRecord[]
  statusCheckRollupComplete: boolean
  headCiState: 'SUCCESS' | 'FAILURE' | 'ERROR' | 'PENDING' | 'EXPECTED' | null
  complete: boolean
  sourceErrors: Record<string,string>
  removed: boolean
}
export interface PrStatusChange<Row = PrStatus> {
  cursor: string
  changedFields: string[]
  observedAtMs: number
  kind: 'baseline' | 'opened' | 'updated' | 'closed' | 'merged' | 'reopened' | 'removed'
  activity: PrActivity[]
  pullRequest: Row
}
/** Account PR status uses gh-style camelCase fields and its own scoped cursor. */
export interface PrStatusPage<Row = PrStatus> {
  pullRequests: Row[]
  changes: PrStatusChange<Row>[]
  cursor: string
  hasMore: boolean
  complete: boolean
  errors: string[]
}
export interface SourceError { source: string; message: string }
export interface ResourceValidation {
  resource: string
  validated_at_ms: number
  source: 'cache' | 'network' | 'revalidated'
}
export interface CiSummary {
  state: CiState
  successful: number
  failed: number
  pending: number
  skipped: number
  unknown: number
}
export interface FailedResult {
  kind: string
  name: string
  conclusion: string
  url: string | null
  failed_steps: GitHubRecord[]
}
export interface CiReport {
  head_sha: string
  merge_sha: string | null
  check_runs: GitHubRecord[]
  commit_statuses: GitHubRecord[]
  workflow_runs: GitHubRecord[]
  jobs: GitHubRecord[]
  summary: CiSummary
  failures: FailedResult[]
  errors: SourceError[]
}
export interface CiObservation {
  data: CiReport
  complete: boolean
  cursor: string | null
  observed_at_ms: number
  oldest_validation_at_ms: number
  validations: ResourceValidation[]
}
export interface ReviewStatus {
  requested_reviewers: GitHubRecord[]
  requested_teams: GitHubRecord[]
  latest_reviews: GitHubRecord[]
  approved_by: string[]
  changes_requested_by: string[]
  dismissed_reviews: GitHubRecord[]
  resolved_threads: number
  unresolved_threads: number
  outdated_threads: number
}
export interface PrReport {
  repository: string
  number: number
  pull_request: GitHubRecord
  conflicts: 'clean' | 'conflicting' | 'unknown'
  comments: GitHubRecord[]
  review_comments: GitHubRecord[]
  reviews: GitHubRecord[]
  timeline: GitHubRecord[]
  review_events: GitHubRecord[]
  review_threads: GitHubRecord[]
  review_status: ReviewStatus
  ci: CiReport
  errors: SourceError[]
}
export interface Report {
  data: PrReport
  observed_at_ms: number
  cursor: string | null
  complete: boolean
  oldest_validation_at_ms: number
  validations: ResourceValidation[]
}
export interface BranchTransition {
  kind: 'baseline' | 'created' | 'updated' | 'deleted' | 'missing'
  old_sha: string | null
  new_sha: string | null
  ancestry: 'forward' | 'rewind' | 'rewritten' | 'unknown'
  comparison_complete: boolean
  commits: GitHubRecord[]
  compare_url: string | null
}
export interface BranchReport {
  repository: string
  branch: string
  sha: string | null
  tip_commit: GitHubRecord | null
  transition: BranchTransition
  errors: SourceError[]
}
export interface RepositoryReport {
  repository: string
  default_branch: string
  branches: BranchReport[]
  errors: SourceError[]
  changed_branches: string[]
  cursor: string
}
export interface RequiredCheck {
  context: string
  app_id: number | null
  state: Exclude<PolicyState, 'not_required'>
  sha: string | null
  url: string | null
}
export interface RequiredChecksReport {
  repository: string
  pull_number: number
  head_sha: string
  base_branch: string
  base_sha: string | null
  pr_base_sha: string | null
  merge_sha: string | null
  state: PolicyState
  strict: boolean
  up_to_date: boolean | null
  checks: RequiredCheck[]
  rules: GitHubRecord[]
  errors: SourceError[]
  cursor: string
}
export interface RateLimit { remaining: number; reset_at_seconds: number }
export interface Status {
  outstanding_requests: number
  active_requests: number
  max_active_requests: number
  queue_capacity: number
  cache_hits: number
  coalesced_requests: number
  network_requests: number
  conditional_requests: number
  not_modified_responses: number
  rate_limits: Record<string, RateLimit>
}
export interface Watch {
  id: string
  repository: string
  pull_number: number
  interval_seconds: number
  kind: 'pull_requests' | 'branches' | 'account'
  branches: string[]
  all_branches: boolean
}
export interface WatchStatus extends Watch {
  covered_by_account: boolean
  last_poll_at_ms: number | null
  last_success_at_ms: number | null
  last_error: string | null
  ci_last_poll_at_ms: number | null
  ci_last_success_at_ms: number | null
  ci_last_error: string | null
  last_cycle: AccountRefreshCycle | null
  ci_last_cycle: AccountRefreshCycle | null
  discovery_last_poll_at_ms: number | null
  discovery_last_success_at_ms: number | null
  discovery_last_error: string | null
}
/** Latest completed account hydration work; not completeness or readiness. */
export interface AccountRefreshCycle {
  started_at_ms: number
  finished_at_ms: number
  total: number
  attempted: number
  succeeded: number
  failed: number
  interrupted: number
  deferred: number
  cycle_budget_exhausted: boolean
}
export interface Snapshot { resource: string; data: JsonValue; observed_at_ms: number }
export interface SnapshotPage { snapshots: Snapshot[]; cursor: string }
export interface Change {
  cursor: string
  resource: string
  changed_fields: string[]
  observed_at_ms: number
  data: JsonValue
}
export interface ChangePage {
  changes: Change[]
  next_cursor: string
  head_cursor: string
  has_more: boolean
}
export class HeyGhError extends Error {
  readonly name: 'HeyGhError'
  readonly code: ErrorCode
  readonly retryAfterSeconds?: number
  readonly httpStatus?: number
  constructor(message: string, code: ErrorCode, details?: { cause?: unknown; retryAfterSeconds?: number; httpStatus?: number })
}
/** Connects to the shared daemon; requires `hey-gh serve` using your existing gh login. */
export class ApiClient {
  constructor(server?: string)
  prStatus<K extends keyof PrStatus>(options: PrStatusOptions<K> & { fields: readonly K[] }): Promise<PrStatusPage<Pick<PrStatus, K | 'complete' | 'sourceErrors'>>>
  prStatus(options?: PrStatusOptions & { fields?: undefined }): Promise<PrStatusPage>
  watchAccount(intervalSeconds?: number): Promise<Watch>
  prReport(repository: string, number: number, options?: ReadOptions): Promise<Report>
  ciForPr(repository: string, number: number, options?: ReadOptions): Promise<CiObservation>
  requiredChecksForPr(repository: string, number: number, options?: ReadOptions): Promise<RequiredChecksReport>
  repositoryReport(repository: string, options?: RepositoryOptions): Promise<RepositoryReport>
  myPullRequests(repository: string, options?: ReadOptions): Promise<GitHubRecord[]>
  listPullRequests(repository: string, state?: 'open' | 'closed' | 'all', options?: ReadOptions): Promise<GitHubRecord[]>
  watch(repository: string, options?: WatchOptions): Promise<Watch>
  watchRepository(repository: string, options?: RepositoryWatchOptions): Promise<Watch>
  watches(): Promise<WatchStatus[]>
  unwatch(id: string): Promise<void>
  status(): Promise<Status>
  bootstrap(): Promise<SnapshotPage>
  changes(options?: ChangesOptions): Promise<ChangePage>
}
