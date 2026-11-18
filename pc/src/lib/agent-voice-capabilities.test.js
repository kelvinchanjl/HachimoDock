import test from "node:test";
import assert from "node:assert/strict";
import { deviceVoicePlatformWarning, deviceVoiceTransportReady } from "./agent-voice-capabilities.js";

test("native Mac Codex draft path does not require the CLI transport", () => {
  assert.equal(deviceVoiceTransportReady("codex", "macos", false), true);
  assert.equal(deviceVoiceTransportReady("codex", "windows", false), false);
  assert.equal(deviceVoiceTransportReady("openclaw", "macos", false), false);
  assert.equal(deviceVoiceTransportReady("openclaw", "macos", true), true);
});

test("MiMoCode platform limitations are surfaced before starting voice recognition", () => {
  assert.match(deviceVoicePlatformWarning("mimocode", "windows"), /目前仅支持 macOS/);
  assert.match(deviceVoicePlatformWarning("mimocode", "other"), /目前仅支持 macOS/);
  assert.equal(deviceVoicePlatformWarning("mimocode", "macos"), "");
  for (const id of ["codex", "claude-code", "openclaw", "workbuddy"]) {
    assert.equal(deviceVoicePlatformWarning(id, "windows"), "");
    assert.equal(deviceVoicePlatformWarning(id, "macos"), "");
  }
});

test("Grok Bot voice is an opt-in desktop target and does not use another agent's transport", () => {
  assert.equal(deviceVoiceTransportReady("grok-bot", "macos", false), true);
  assert.equal(deviceVoiceTransportReady("grok-bot", "windows", false), true);
  assert.equal(deviceVoiceTransportReady("grok-bot", "linux", false), false);
  assert.equal(deviceVoicePlatformWarning("grok-bot", "macos"), "");
  assert.equal(deviceVoicePlatformWarning("grok-bot", "windows"), "");
  assert.match(deviceVoicePlatformWarning("grok-bot", "linux"), /不会向窗口输入/);
  assert.equal(deviceVoicePlatformWarning("codex", "linux"), "");
});
