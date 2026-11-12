# Issue search

Smart search is **off by default on each machine**. Normal installs use literal
search and never require an OpenAI key. Opt in with the search box's **Enable
smart search** button, or:

```sh
hey-boss issue search-index --enable
hey-boss issue search 'connection dies after waking'
hey-boss issue search 'connection dies after waking' --state all
hey-boss issue search-index
hey-boss issue search-index --disable
```

Opting in sends titles, descriptions, and comments (including archived issues)
to OpenAI through the existing hey-proxy connection. Credentials stay in the
proxy. The local index and preference are excluded from fleet sync. Turning it
off stops new background work; an already running request may finish. The cache
is retained for later re-enabling. Each enabled machine indexes its own replica.

Title matches carry weight 5, descriptions 2, and comments 0.5. Exact issue
numbers come first. Phrases and identifiers beat incidental words. Only the best
passage per field contributes; repeating words or adding comments cannot inflate
a score. Up to 300 candidate issues per field and 300 semantic candidates enter
ranking. Results show the matching field and excerpt.

Indexing runs in small resumable pages at background priority, yields to database
writers, deduplicates unchanged text, and backs off when the proxy is unavailable.
No provider request holds an issue transaction. Browser search shows keywords
first, then refines with related meaning; unavailable embeddings leave keyword
search usable. Indexing progress appears beside the search results.

The model is `text-embedding-3-small` with 512 dimensions. Source vectors use
compact 8-bit storage; queries keep full precision. Query vectors are cached
locally (256 recent queries); source vectors are shared by identical text. The
rebuildable cache is next to the issue database at `<database>.search.db`.

`issue list --search TEXT` keeps its literal behavior. `issue search TEXT
--literal` also uses the old title/body filter and queue order without an API call.

To measure search latency against a disposable 9,000-issue corpus, build the
synthetic provider and run the workload against the installed CLI:

```sh
cargo build --example search-provider
HEY_BOSS_TEST_BINARY="$(command -v hey-boss)" cargo test --example web-fixture full_index_search_with_foreground -- --ignored --nocapture
```

It reports keyword and hybrid retrieval times, plus foreground read and write
times during indexing and a simulated provider outage. No external provider is contacted.
