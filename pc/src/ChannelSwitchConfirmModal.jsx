/**
 * [Input] Current and next desktop-pet channel labels plus modal actions.
 * [Output] Confirmation dialog shown before switching the single followed device subject.
 * [Pos] component node in pc/src
 * [Sync] If this file changes, update this header and `pc/src/.folder.md`.
 */

import React from "react";
import { AlertCircle, X } from "lucide-react";

export default function ChannelSwitchConfirmModal({
  currentLabel,
  nextLabel,
  voiceOnly = false,
  onCancel,
  onConfirm,
}) {
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal-card channel-switch-confirm-modal" onClick={(event) => event.stopPropagation()}>
        <div className="modal-header">
          <div>
            <h3 className="modal-title">{voiceOnly ? "设为语音输入目标" : "切换设备跟随主体"}</h3>
            <div className="modal-subtitle">
              {voiceOnly
                ? "桌宠不会跟随 Grok Bot 的思考或输入状态。"
                : "设备端当前只能跟随一个本地编程工具。"}
            </div>
          </div>
          <button className="icon-btn" onClick={onCancel} aria-label="关闭">
            <X size={16} />
          </button>
        </div>
        <div className="modal-body channel-switch-confirm-modal__body">
          <div className="message-banner message-banner--muted channel-switch-confirm-modal__message">
            <AlertCircle size={14} />
            {voiceOnly
              ? `按键语音目标将从 ${currentLabel || "当前目标"} 改为 ${nextLabel || "Grok Bot"}。这不提供思考或输入状态。`
              : `当前设备正在跟随 ${currentLabel || "当前主体"}。继续后，设备端跟随主体会从 ${currentLabel || "当前主体"} 切换到 ${nextLabel || "目标主体"}。`}
          </div>
        </div>
        <div className="channel-switch-confirm-modal__actions">
          <button className="btn-secondary" type="button" onClick={onCancel}>
            取消
          </button>
          <button className="btn-primary" type="button" onClick={onConfirm}>
            {voiceOnly ? "确认语音目标" : "继续切换"}
          </button>
        </div>
      </div>
    </div>
  );
}
