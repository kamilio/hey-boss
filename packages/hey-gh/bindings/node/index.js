'use strict'
const { NativeApiClient } = require('./native.js')

class HeyGhError extends Error {
  constructor(message, code, details = {}) {
    super(message, details.cause ? { cause: details.cause } : undefined)
    this.name = 'HeyGhError'
    this.code = code
    if (details.retryAfterSeconds != null) this.retryAfterSeconds = details.retryAfterSeconds
    if (details.httpStatus != null) this.httpStatus = details.httpStatus
  }
}
function bindingError(error) {
  const prefix = 'HEY_GH_ERROR:'
  if (typeof error?.message === 'string' && error.message.startsWith(prefix)) {
    try {
      const data = JSON.parse(error.message.slice(prefix.length))
      return new HeyGhError(data.message, data.code, { ...data, cause: error })
    } catch { /* Preserve unrecognized native errors below. */ }
  }
  if (error?.code === 'InvalidArg') return new HeyGhError(error.message, 'invalid', { cause: error })
  return error
}
async function invoke(call) {
  try { return await call() } catch (error) { throw bindingError(error) }
}
class ApiClient {
  #native
  constructor(server) {
    try { this.#native = new NativeApiClient(server) } catch (error) { throw bindingError(error) }
  }
  prStatus(options) { return invoke(() => this.#native.prStatus(options)) }
  watchAccount(intervalSeconds) { return invoke(() => this.#native.watchAccount(intervalSeconds)) }
  prReport(repository, number, options) { return invoke(() => this.#native.prReport(repository, number, options)) }
  ciForPr(repository, number, options) { return invoke(() => this.#native.ciForPr(repository, number, options)) }
  requiredChecksForPr(repository, number, options) { return invoke(() => this.#native.requiredChecksForPr(repository, number, options)) }
  repositoryReport(repository, options) { return invoke(() => this.#native.repositoryReport(repository, options)) }
  myPullRequests(repository, options) { return invoke(() => this.#native.myPullRequests(repository, options)) }
  listPullRequests(repository, state, options) { return invoke(() => this.#native.listPullRequests(repository, state, options)) }
  watch(repository, options) { return invoke(() => this.#native.watch(repository, options)) }
  watchRepository(repository, options) { return invoke(() => this.#native.watchRepository(repository, options)) }
  watches() { return invoke(() => this.#native.watches()) }
  unwatch(id) { return invoke(() => this.#native.unwatch(id)) }
  status() { return invoke(() => this.#native.status()) }
  bootstrap() { return invoke(() => this.#native.bootstrap()) }
  changes(options) { return invoke(() => this.#native.changes(options)) }
}
module.exports = { ApiClient, HeyGhError }
