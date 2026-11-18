/**
 * [Input] Detected local coding agents, persisted appearance records, and localStorage bridge assignment state.
 * [Output] ChatGPT（Codex）-first fixed Agent ordering, explicit first-run selection, shared per-channel appearance mapping, and single active desktop-channel helpers used by setup, dashboard, and gallery.
 * [Pos] lib node in pc/src/lib
 * [Sync] If this file changes, update `pc/src/.folder.md` and consumers that persist agent assignments.
 */

import { BUILTIN_TERRIER_APPEARANCE_ID } from "./builtin-appearances.js";

export const AGENT_APPEARANCE_MAP_STORAGE_KEY = "pet-manager.agent-appearance-map";
export const ENABLED_AGENTS_STORAGE_KEY = "pet-manager.enabled-agents";
export const DEFAULT_AGENT_ID = "workbuddy";

export const FIXED_AGENT_OPTIONS = [
  {
    id: "codex",
    label: "ChatGPT（Codex）",
    detail: "ChatGPT 客户端的 Codex 任务与状态渠道",
  },
  {
    id: "claude-code",
    label: "Claude",
    detail: "Claude 客户端与本地 CLI 渠道",
  },
  {
    id: "openclaw",
    label: "OpenClaw",
    detail: "OpenClaw 本地运行时渠道",
  },
  {
    id: "mimocode",
    label: "MiMo Desktop / MiMoCode",
    detail: "跟随小米 MiMo 桌面应用和 MiMoCode 的会话状态",
  },
  {
    id: "deepseek-harness",
    label: "DeepSeek Harness",
    detail: "DeepSeek Harness 桌面对话与本地桥接渠道",
  },
  {
    id: "workbuddy",
    label: "WorkBuddy",
    detail: "WorkBuddy 状态与气泡跟随；向当前可见对话追加语音草稿",
  },
  {
    id: "hermes",
    label: "Hermes Agent",
    detail: "跟随 Hermes 本地会话状态",
  },
  {
    id: "grok-bot",
    label: "Grok Bot",
    detail: "按键语音可写入已登记的前台输入框。没有公开的思考或输入状态，桌宠不会跟随 Grok Bot 的工作状态。",
    voiceInput: true,
    activityFollow: false,
  },
];

export function loadEnabledAgents() {
  try {
    const raw = localStorage.getItem(ENABLED_AGENTS_STORAGE_KEY);
    if (raw) return toSingleAgentSet(JSON.parse(raw));
  } catch {}
  return null;
}

export function saveEnabledAgents(set) {
  localStorage.setItem(ENABLED_AGENTS_STORAGE_KEY, JSON.stringify([...set]));
}

export function isVoiceOnlyChannel(agent) {
  return agent?.id === "grok-bot" || (agent?.voiceInput === true && agent?.activityFollow === false);
}

export function voiceOnlyChannelNotice() {
  return "已把按键语音目标设为 Grok Bot。桌宠不会跟随它的思考或输入状态。";
}

export function channelOptionLabel(agent) {
  if (isVoiceOnlyChannel(agent)) return `${agent.label}（语音输入，状态不可跟随）`;
  return agent?.label || "";
}

export function channelListPresentation(agent, activeAgentId = "") {
  const selected = Boolean(agent?.id) && agent.id === activeAgentId;
  if (isVoiceOnlyChannel(agent)) {
    return {
      voiceOnly: true,
      detected: false,
      rowClass: selected ? "is-active is-voice-only" : "is-voice-only",
      status: "状态跟随不可用",
      statusTitle: agent.detail || "没有公开的工作状态接口。",
      appearanceDisabled: false,
      followKind: selected ? "voice-current" : "voice-select",
      followLabel: selected ? "语音目标" : "用于语音",
    };
  }
  const detected = agent?.detected === true;
  return {
    voiceOnly: false,
    detected,
    rowClass: `${selected ? "is-active" : ""}${detected ? "" : " is-undetected"}`.trim(),
    status: detected ? "已检测" : "未检测到客户端或 CLI",
    statusTitle: agent?.detail || undefined,
    appearanceDisabled: !detected,
    followKind: selected ? "followed" : detected ? "follow" : "unavailable",
    followLabel: selected ? "已跟随" : detected ? "跟随" : "不可用",
  };
}

export function normalizeDetectedAgents(agents = []) {
  const detectedById = new Map((agents || []).map((agent) => [agent.id, agent]));
  return FIXED_AGENT_OPTIONS.map((option) => {
    const detected = detectedById.get(option.id);
    const voiceOnly = isVoiceOnlyChannel(option);
    return {
      ...option,
      ...(detected || {}),
      // Keep product-facing names stable even when an older bridge scan is cached.
      label: option.label,
      detail: voiceOnly ? option.detail : (detected?.detail || option.detail),
      detected: voiceOnly ? false : Boolean(detected?.detected),
      voiceInput: voiceOnly ? true : option.voiceInput === true,
      activityFollow: voiceOnly ? false : option.activityFollow !== false,
    };
  });
}

export function toSingleAgentSet(value) {
  const values = typeof value === "string" ? [value] : Array.isArray(value) ? value : [...(value || [])];
  const first = values[0];
  return first ? new Set([first]) : new Set();
}

export function pickFirstDetectedAgentId(agents = [], preferredAgentId = "") {
  if (preferredAgentId) {
    const preferred = agents.find(
      (agent) => agent.id === preferredAgentId && agent.detected,
    );
    if (preferred) return preferred.id;
  }
  return agents.find((agent) => agent.detected)?.id || "";
}

export function defaultAgentAppearanceMap(records = []) {
  const defaultAppearance =
    records.find((record) => record.id === BUILTIN_TERRIER_APPEARANCE_ID) || records[0];
  if (!defaultAppearance) return {};
  return Object.fromEntries(FIXED_AGENT_OPTIONS.map((agent) => [agent.id, defaultAppearance.id]));
}

export function sanitizeAgentAppearanceMap(map, records = []) {
  const validAppearanceIds = new Set(records.map((record) => record.id));
  const validAgentIds = new Set(FIXED_AGENT_OPTIONS.map((agent) => agent.id));
  const validEntries = Object.entries(map || {}).filter(
    ([agentId, appearanceId]) =>
      validAgentIds.has(agentId) &&
      typeof appearanceId === "string" &&
      validAppearanceIds.has(appearanceId),
  );
  return Object.fromEntries(validEntries);
}

export function loadAgentAppearanceMap(records = []) {
  try {
    const raw = localStorage.getItem(AGENT_APPEARANCE_MAP_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        return sanitizeAgentAppearanceMap(parsed, records);
      }
    }
  } catch {}
  return defaultAgentAppearanceMap(records);
}

export function saveAgentAppearanceMap(map) {
  localStorage.setItem(AGENT_APPEARANCE_MAP_STORAGE_KEY, JSON.stringify(map));
}

export function assignedAgentIds(agentAppearanceMap, enabledAgents = null) {
  const active = activeDesktopAssignment(agentAppearanceMap, enabledAgents);
  return active.agentId ? [active.agentId] : [];
}

export function appearanceById(records, appearanceId) {
  return records.find((record) => record.id === appearanceId) || null;
}

export function channelLabelForId(agents, agentId) {
  return agents.find((agent) => agent.id === agentId)?.label || agentId;
}

export function firstAgentIdForAppearance(agentAppearanceMap, appearanceId) {
  return Object.entries(agentAppearanceMap || {}).find(([, id]) => id === appearanceId)?.[0] || "";
}

function firstEnabledAgentId(enabledAgents) {
  if (!enabledAgents) return "";
  const values = typeof enabledAgents === "string"
    ? [enabledAgents]
    : Array.isArray(enabledAgents)
      ? enabledAgents
      : [...enabledAgents];
  return values[0] || "";
}

export function activeDesktopAssignment(agentAppearanceMap, enabledAgents = null) {
  const enabledAgentId = firstEnabledAgentId(enabledAgents);
  if (enabledAgentId) {
    return {
      agentId: enabledAgentId,
      appearanceId: agentAppearanceMap?.[enabledAgentId] || BUILTIN_TERRIER_APPEARANCE_ID,
    };
  }

  const [agentId = "", appearanceId = ""] =
    Object.entries(agentAppearanceMap || {}).find(([, id]) => Boolean(id)) || [];
  return { agentId, appearanceId: appearanceId || (agentId ? BUILTIN_TERRIER_APPEARANCE_ID : "") };
}

export function shouldConfirmChannelSwitch(agentAppearanceMap, nextAgentId, enabledAgents = null) {
  const currentAgentId = activeDesktopAssignment(agentAppearanceMap, enabledAgents).agentId;
  return Boolean(currentAgentId && nextAgentId && currentAgentId !== nextAgentId);
}

export function assignAppearanceToAgent(map, agentId, appearanceId) {
  const next = { ...(map || {}) };
  if (!agentId) return next;
  if (!appearanceId) {
    delete next[agentId];
    return next;
  }
  next[agentId] = appearanceId;
  return next;
}
