/**
 * [Input] Agent-to-appearance assignment helper.
 * [Output] Node regression coverage for stable ChatGPT（Codex）/Claude labels, default ordering/selection, per-channel appearance binding, and single active desktop-channel behavior.
 * [Pos] test node in pc/src/lib
 * [Sync] If this file changes, update `pc/src/.folder.md`.
 */

import test from "node:test";
import assert from "node:assert/strict";
import {
  activeDesktopAssignment,
  assignAppearanceToAgent,
  assignedAgentIds,
  DEFAULT_AGENT_ID,
  ENABLED_AGENTS_STORAGE_KEY,
  FIXED_AGENT_OPTIONS,
  channelListPresentation,
  channelOptionLabel,
  isVoiceOnlyChannel,
  loadEnabledAgents,
  normalizeDetectedAgents,
  saveEnabledAgents,
  voiceOnlyChannelNotice,
  pickFirstDetectedAgentId,
  sanitizeAgentAppearanceMap,
  shouldConfirmChannelSwitch,
  toSingleAgentSet,
} from "./agent-appearance-config.js";

function installStorage(initial = {}) {
  const values = new Map(Object.entries(initial));
  globalThis.localStorage = {
    getItem: (key) => values.get(key) || null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
    clear: () => values.clear(),
  };
  return values;
}

test("the fixed Agents put ChatGPT（Codex） first and use it as the first-run default", () => {
  assert.equal(DEFAULT_AGENT_ID, "workbuddy");
  assert.deepEqual(
    FIXED_AGENT_OPTIONS.map((agent) => agent.id),
    [
      "codex",
      "claude-code",
      "openclaw",
      "mimocode",
      "deepseek-harness",
      "workbuddy",
      "hermes",
      "grok-bot",
    ],
  );
  assert.deepEqual(
    FIXED_AGENT_OPTIONS.slice(0, 2).map((agent) => agent.label),
    ["ChatGPT（Codex）", "Claude"],
  );
});

test("Grok Bot stays selectable for voice without a detected or activity-follow status", () => {
  const agents = normalizeDetectedAgents([
    { id: "codex", detected: true, detail: "installed" },
    { id: "grok-bot", detected: true, detail: "fake telemetry", activityFollow: true },
  ]);
  const grok = agents.find((agent) => agent.id === "grok-bot");
  const codex = agents.find((agent) => agent.id === "codex");
  assert.equal(agents.some((agent) => agent.id === "grok-bot"), true);
  assert.equal(grok.detected, false);
  assert.equal(grok.activityFollow, false);
  assert.equal(grok.voiceInput, true);
  assert.equal(grok.detail.includes("fake"), false);
  assert.equal(isVoiceOnlyChannel(grok), true);
  assert.equal(codex.detected, true);
  assert.equal(codex.activityFollow, true);

  const idle = channelListPresentation(grok, "codex");
  assert.equal(idle.status, "状态跟随不可用");
  assert.equal(idle.followLabel, "用于语音");
  assert.equal(idle.appearanceDisabled, false);
  assert.equal(idle.followLabel === "已跟随" || idle.followLabel === "不可用", false);

  const selected = channelListPresentation(grok, "grok-bot");
  assert.equal(selected.followLabel, "语音目标");
  assert.match(selected.rowClass, /is-voice-only/);
  assert.doesNotMatch(selected.rowClass, /is-undetected/);
  assert.equal(channelOptionLabel(grok), "Grok Bot（语音输入，状态不可跟随）");
  assert.equal(channelListPresentation(codex, "codex").followLabel, "已跟随");
  assert.equal(channelListPresentation({ id: "hermes", detected: false, label: "Hermes Agent" }, "codex").followLabel, "不可用");

  installStorage();
  saveEnabledAgents(toSingleAgentSet(["grok-bot"]));
  assert.deepEqual([...loadEnabledAgents()], ["grok-bot"]);
  assert.equal(JSON.parse(globalThis.localStorage.getItem(ENABLED_AGENTS_STORAGE_KEY))[0], "grok-bot");
  assert.match(voiceOnlyChannelNotice(), /不会跟随/);
});

test("assignAppearanceToAgent keeps one channel bound to at most one appearance", () => {
  const next = assignAppearanceToAgent(
    {
      codex: "builtin-terrier",
    },
    "codex",
    "custom-cloud",
  );

  assert.deepEqual(next, {
    codex: "custom-cloud",
  });
});

test("assignAppearanceToAgent preserves other channel appearance settings", () => {
  const next = assignAppearanceToAgent(
    {
      codex: "builtin-terrier",
      "claude-code": "custom-cloud",
    },
    "claude-code",
    "builtin-terrier",
  );

  assert.deepEqual(next, {
    codex: "builtin-terrier",
    "claude-code": "builtin-terrier",
  });
});

test("assignAppearanceToAgent clears only the selected channel when no appearance is selected", () => {
  const next = assignAppearanceToAgent(
    {
      codex: "builtin-terrier",
      "claude-code": "custom-cloud",
    },
    "claude-code",
    "",
  );

  assert.deepEqual(next, {
    codex: "builtin-terrier",
  });
});

test("sanitizeAgentAppearanceMap keeps valid per-channel assignments", () => {
  const next = sanitizeAgentAppearanceMap(
    {
      "claude-code": "custom-cloud",
      codex: "builtin-terrier",
    },
    [{ id: "builtin-terrier" }, { id: "custom-cloud" }],
  );

  assert.deepEqual(next, {
    "claude-code": "custom-cloud",
    codex: "builtin-terrier",
  });
});

test("channel switch warnings only appear when changing the current followed channel", () => {
  const map = { codex: "builtin-terrier", "claude-code": "custom-cloud" };

  assert.deepEqual(activeDesktopAssignment(map, new Set(["claude-code"])), {
    agentId: "claude-code",
    appearanceId: "custom-cloud",
  });
  assert.equal(shouldConfirmChannelSwitch(map, "claude-code", new Set(["claude-code"])), false);
  assert.equal(shouldConfirmChannelSwitch(map, "codex", new Set(["claude-code"])), true);
  assert.deepEqual(assignedAgentIds(map, "claude-code"), ["claude-code"]);
  assert.equal(shouldConfirmChannelSwitch({}, "claude-code"), false);
});

test("loadEnabledAgents keeps legacy values to a single active channel", () => {
  installStorage({
    [ENABLED_AGENTS_STORAGE_KEY]: JSON.stringify("claude-code"),
  });

  assert.deepEqual([...loadEnabledAgents()], ["claude-code"]);
});

test("normalizeDetectedAgents marks unavailable channels as not bindable", () => {
  const agents = normalizeDetectedAgents([
    { id: "codex", label: "Codex", detected: true, detail: "installed" },
    { id: "claude-code", label: "Claude Code", detected: true, detail: "installed" },
  ]);

  assert.equal(agents.find((agent) => agent.id === "codex")?.detected, true);
  assert.equal(agents.find((agent) => agent.id === "codex")?.label, "ChatGPT（Codex）");
  assert.equal(agents.find((agent) => agent.id === "claude-code")?.label, "Claude");
  assert.equal(agents.find((agent) => agent.id === "openclaw")?.detected, false);
  assert.equal(agents.find((agent) => agent.id === "mimocode")?.detected, false);
});

test("toSingleAgentSet respects stored order without preferring any specific agent", () => {
  assert.deepEqual([...toSingleAgentSet(["openclaw", "codex"])], ["openclaw"]);
  assert.deepEqual([...toSingleAgentSet(["claude-code", "codex"])], ["claude-code"]);
  assert.deepEqual([...toSingleAgentSet("openclaw")], ["openclaw"]);
  assert.deepEqual([...toSingleAgentSet([])], []);
});

test("pickFirstDetectedAgentId prefers the currently followed agent when it is detected", () => {
  const agents = [
    { id: "claude-code", detected: true },
    { id: "codex", detected: true },
    { id: "openclaw", detected: true },
  ];

  assert.equal(pickFirstDetectedAgentId(agents, "openclaw"), "openclaw");
  assert.equal(pickFirstDetectedAgentId(agents, "codex"), "codex");
});

test("pickFirstDetectedAgentId falls back to first detected when followed agent is missing or undetected", () => {
  const agents = [
    { id: "claude-code", detected: false },
    { id: "codex", detected: true },
    { id: "openclaw", detected: true },
  ];

  assert.equal(pickFirstDetectedAgentId(agents, "openclaw"), "openclaw");
  assert.equal(pickFirstDetectedAgentId(agents, "claude-code"), "codex");
  assert.equal(pickFirstDetectedAgentId(agents), "codex");
  assert.equal(pickFirstDetectedAgentId([]), "");
});
