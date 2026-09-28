// Offline integration with an installed Pi (verified with 0.87.1).
// node tests/pi_compaction.mjs PI_PACKAGE_DIR [SAVED_SESSION_JSONL]
// Build hey-proxy first with `cargo build`. No credentials or model calls needed.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

assert(process.argv[2], "Pass the installed Pi package directory");
const pi = resolve(process.argv[2]);
const load = (file) => import(pathToFileURL(join(pi, file)).href);
const { SettingsManager } = await load("dist/core/settings-manager.js");
const { composeModelProvider } = await load("dist/core/provider-composer.js");
const { buildSessionProjection } = await load("dist/core/session-manager.js");
const { convertToLlm } = await load("dist/core/messages.js");
const { prepareCompaction, shouldCompact, estimateProjectedContextTokens } = await load("dist/core/compaction/compaction.js");
const { clampMaxTokensToContext } = await load("node_modules/@earendil-works/pi-ai/dist/api/simple-options.js");
const agent = mkdtempSync(join(tmpdir(), "hey-proxy-pi-"));
try {
    const before = { compaction: { enabled: true, keepRecentTokens: 100000, reserveTokens: 200000 } };
    writeFileSync(join(agent, "settings.json"), JSON.stringify(before));
    writeFileSync(join(agent, "proxy.json"), JSON.stringify({
        listen: "127.0.0.1:8080",
        aliases: [{ from: "gemini-test", to: "gemini/gemini-early-exp" }],
    }));
    execFileSync(resolve("target/debug/hey-proxy"), ["--config", join(agent, "proxy.json"), "configure-pi"], {
        env: { ...process.env, PI_CODING_AGENT_DIR: agent },
    });
    const catalog = JSON.parse(readFileSync(join(agent, "models.json")));
    const provider = composeModelProvider("hey-proxy", undefined, {
        getProvider: (id) => catalog.providers[id],
    });
    const model = provider.getModels().find((model) => model.id === "gemini-test");
    assert.equal(model.contextWindow, 128000);
    assert.equal(model.maxTokens, 16384);
    const settings = SettingsManager.inMemory(JSON.parse(readFileSync(join(agent, "settings.json"))))
        .getCompactionSettings(model);
    assert.deepEqual(settings, { enabled: true, keepRecentTokens: 20000, reserveTokens: 16384 });

    // Each branch has about 90k text-estimated tokens but much larger reported
    // usage. The old retention has no cut point as output space approaches zero.
    let checked = 0;
    const check = (branch, label, lengthRecovery = false) => {
        const projection = buildSessionProjection(branch);
        const tokens = estimateProjectedContextTokens(projection, branch).tokens;
        const threshold = shouldCompact(tokens, model.contextWindow, settings);
        // Context edits can invalidate prior usage in the threshold estimator;
        // Pi also requests compaction directly during length-error recovery.
        assert(threshold || lengthRecovery);
        assert.equal(prepareCompaction(branch, before.compaction), undefined);
        const plan = prepareCompaction(branch, settings);
        assert(plan, `${label}: no compaction cut point`);
        assert(plan.messagesToSummarize.length + plan.turnPrefixMessages.length > 0);
        // Test the rebuilt context using a synthetic summary; do not call a model.
        const compacted = [...branch, {
            type: "compaction", id: "test-compaction", parentId: branch.at(-1).id,
            timestamp: new Date().toISOString(), summary: "Earlier work summarized for offline testing.",
            firstKeptEntryId: plan.firstKeptEntryId, tokensBefore: plan.tokensBefore,
        }];
        const budget = clampMaxTokensToContext(model, { messages: convertToLlm(buildSessionProjection(compacted).messages) }, model.maxTokens);
        assert.equal(budget, model.maxTokens, `${label}: output still starved after compaction`);
        const oldBudget = clampMaxTokensToContext(model, { messages: convertToLlm(projection.messages) }, model.maxTokens);
        assert(oldBudget < 2000, `${label}: fixture does not reproduce output starvation`);
        console.log(JSON.stringify({ label, tokens, threshold, oldBudget, budgetAfterSyntheticSummary: budget, cutPoint: true }));
        checked++;
    };
    if (process.argv[3]) {
        // Reconstruct each failing response's input via parent IDs, including forks.
        // Read only; neither the session nor real Pi settings are modified.
        const entries = readFileSync(process.argv[3], "utf8").trim().split("\n").map(JSON.parse);
        const byId = new Map(entries.map((entry) => [entry.id, entry]));
        for (const entry of entries) {
            if (entry.type !== "message" || entry.message.role !== "assistant" ||
                entry.message.model !== model.id || entry.message.stopReason !== "length") continue;
            const branch = [];
            for (let cursor = byId.get(entry.parentId); cursor; cursor = byId.get(cursor.parentId)) branch.push(cursor);
            check(branch.reverse(), `length response ${checked + 1}`, true);
        }
    } else {
        for (const totalTokens of [122133, 123070, 125008]) {
            const branch = Array.from({ length: 18 }, (_, index) => ({
                type: "message", id: `entry-${index}`, parentId: index ? `entry-${index - 1}` : null,
                timestamp: new Date(0).toISOString(),
                message: { timestamp: 0, role: index % 2 ? "assistant" : "user", content: [{ type: "text", text: "x".repeat(20000) }],
                    ...(index % 2 ? { model: model.id, provider: model.provider, api: model.api, stopReason: "stop",
                        usage: { input: totalTokens - 100, output: 100, cacheRead: 0, cacheWrite: 0, totalTokens,
                            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } } : {}) },
            }));
            check(branch, `reported context ${totalTokens}`);
        }
    }
    assert(checked >= 3, `Expected at least three regression cases, found ${checked}`);
    console.log(`Passed ${checked} Pi compaction regression cases`);
} finally {
    rmSync(agent, { recursive: true, force: true });
}
