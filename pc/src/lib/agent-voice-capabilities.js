export function deviceVoicePlatformWarning(agentId, platform) {
  if (agentId === "grok-bot" && platform !== "macos" && platform !== "windows") {
    return "Grok Bot 的按键语音只支持在 macOS 或 Windows 上登记当前前台输入框；当前系统不会向窗口输入文字。";
  }
  return agentId === "mimocode" && platform !== "macos"
    ? "MiMoCode 的按键语音输入目前仅支持 macOS 终端；当前系统可使用状态跟随与设备实时对话。"
    : "";
}
export function deviceVoiceTransportReady(agentId, platform, cliReady) {
  // macOS Codex drafts use the native desktop input path. CLI readiness
  // describes a different transport; native permission/window checks run at use.
  // Grok Bot has no session bus. Listening can start on a supported OS; insertion
  // still requires an explicit foreground registration.
  if (agentId === "grok-bot") return platform === "macos" || platform === "windows";
  return (platform === "macos" && agentId === "codex") || cliReady === true;
}

