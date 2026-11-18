//! Opt-in Grok Bot push-to-talk target.
//!
//! Official desktop docs identify the app and its operating systems. They do
//! not publish a conversation id, accessibility selector, bundle id, or a chat
//! API that carries the Bot's memory and tools. This module therefore never
//! looks up an app by name. The user registers the focused text control, and
//! later writes are refused unless that same control is still focused.
//! Release appends. A separate confirm action sends. Nothing here calls the
//! xAI model API or reads a credential store.

use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

pub const AGENT_ID: &str = "grok-bot";
pub const RELEASE_CONFIRMS_SEND: bool = false;
const MAX_TEXT_CHARS: usize = 4_000;
const MAX_TITLE_CHARS: usize = 300;
const MAX_PROCESS_CHARS: usize = 128;
const MAX_IDENTIFIER_CHARS: usize = 200;

const MAC_TEXT_ROLES: &[&str] = &["AXTextArea", "AXTextField", "AXSearchField", "AXComboBox"];
const WINDOWS_TEXT_ROLES: &[&str] = &[
    "ControlType.Edit",
    "ControlType.Document",
    "ControlType.ComboBox",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerIdentity {
    pub pid: i32,
    pub foreground_pid: i32,
    pub process_name: String,
    pub role: String,
    pub subrole: String,
    pub identifier: String,
    pub window_title: String,
    pub element_hash: u64,
    pub window_hash: u64,
    pub runtime_id: Vec<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseDecision {
    Append,
    Refuse(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmDecision {
    Send,
    Refuse(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardTransaction {
    pub captured: bool,
    pub mutated: bool,
    pub restored: bool,
}

#[derive(Clone, Debug, Default)]
struct TargetStore {
    preview: Option<ComposerIdentity>,
    committed: Option<ComposerIdentity>,
}

fn store() -> &'static Mutex<TargetStore> {
    static STORE: OnceLock<Mutex<TargetStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(TargetStore::default()))
}

pub fn adapter_block_reason(host_os: &str) -> Option<&'static str> {
    match host_os {
        "macos" | "windows" => None,
        _ => Some("unsupported_platform"),
    }
}

pub fn current_host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "other"
    }
}

pub fn text_role_supported(role: &str) -> bool {
    MAC_TEXT_ROLES.contains(&role) || WINDOWS_TEXT_ROLES.contains(&role)
}

fn clean_text(value: &str, max_chars: usize) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= max_chars
        && value == trimmed
        && !value.chars().any(|ch| ch.is_control() || ch == '\u{202e}')
}

fn clean_process_name(value: &str) -> bool {
    clean_text(value, MAX_PROCESS_CHARS) && !value.contains('/') && !value.contains('\\')
}

pub fn identities_match(registered: &ComposerIdentity, live: &ComposerIdentity) -> bool {
    registered.pid > 0
        && registered.pid == live.pid
        && registered.foreground_pid == registered.pid
        && live.foreground_pid == live.pid
        && clean_process_name(&registered.process_name)
        && registered.process_name.eq_ignore_ascii_case(&live.process_name)
        && registered.role == live.role
        && text_role_supported(&registered.role)
        && registered.subrole == live.subrole
        && clean_text(&registered.window_title, MAX_TITLE_CHARS)
        && registered.window_title == live.window_title
        && registered.identifier == live.identifier
        && registered.identifier.chars().count() <= MAX_IDENTIFIER_CHARS
        && !registered.identifier.chars().any(|ch| ch.is_control())
        && if registered.runtime_id.is_empty() && live.runtime_id.is_empty() {
            registered.element_hash != 0
                && registered.window_hash != 0
                && registered.element_hash == live.element_hash
                && registered.window_hash == live.window_hash
        } else {
            !registered.runtime_id.is_empty()
                && registered.runtime_id == live.runtime_id
                && registered.runtime_id.len() <= 16
                && registered.runtime_id.iter().all(|part| *part >= 0)
        }
}

pub fn validate_for_registration(identity: &ComposerIdentity, own_pid: i32) -> Result<(), &'static str> {
    if identity.pid == own_pid {
        return Err("own_process");
    }
    if identity.foreground_pid != identity.pid {
        return Err("focus_ambiguous");
    }
    if !text_role_supported(&identity.role) {
        return Err("role_unsupported");
    }
    if identity.window_title.trim().is_empty() {
        return Err("window_title_missing");
    }
    if !identities_match(identity, identity) {
        return Err("identity_incomplete");
    }
    Ok(())
}

fn text_policy(text: &str) -> Result<(), &'static str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty_text");
    }
    if trimmed.chars().count() > MAX_TEXT_CHARS {
        return Err("text_too_long");
    }
    Ok(())
}

pub fn on_release(
    registered: Option<&ComposerIdentity>,
    live: Option<&ComposerIdentity>,
    text: &str,
) -> ReleaseDecision {
    let Some(registered) = registered else {
        return ReleaseDecision::Refuse("unregistered");
    };
    let Some(live) = live else {
        return ReleaseDecision::Refuse("focus_unavailable");
    };
    if !identities_match(registered, live) {
        return ReleaseDecision::Refuse("focus_changed");
    }
    match text_policy(text) {
        Ok(()) => ReleaseDecision::Append,
        Err(code) => ReleaseDecision::Refuse(code),
    }
}

pub fn on_confirm(
    registered: Option<&ComposerIdentity>,
    live: Option<&ComposerIdentity>,
    text: &str,
    draft_inserted: bool,
) -> ConfirmDecision {
    if !draft_inserted {
        return ConfirmDecision::Refuse("draft_not_inserted");
    }
    match on_release(registered, live, text) {
        ReleaseDecision::Append => ConfirmDecision::Send,
        ReleaseDecision::Refuse(code) => ConfirmDecision::Refuse(code),
    }
}

pub fn clipboard_transaction_accepted(tx: ClipboardTransaction) -> bool {
    if !tx.mutated {
        return true;
    }
    tx.captured && tx.restored
}

pub fn user_message(code: &str) -> String {
    let text = match code {
        "unsupported_platform" => "Grok Bot 按键语音目前只能在 macOS 和 Windows 上写入已登记的前台输入框。其他系统不会向窗口输入。",
        "permission_missing" => "尚未获得辅助功能权限。请在系统设置里手动为哈基米打开辅助功能；这次操作不会申请权限，也不会打开系统设置。",
        "role_unsupported" => "当前焦点不是系统识别的文本输入控件，未登记，也没有输入文字。",
        "window_title_missing" => "前台窗口没有可读标题，无法区分对话，未登记。",
        "identity_incomplete" => "无法确认前台输入框的进程或控件身份，未登记。",
        "focus_ambiguous" => "前台窗口和焦点控件不一致，未输入。",
        "own_process" => "焦点在哈基米自己的窗口里，不会把语音写进本程序。",
        "not_previewed" => "请先读取当前前台输入框，再确认登记。",
        "focus_changed" => "前台输入框已变化，与已登记的 Grok Bot 对话不一致，未写入也未发送。",
        "empty_text" => "识别文字为空，未写入也未发送。",
        "text_too_long" => "识别文字过长，未写入。",
        "unregistered" => "还没有登记 Grok Bot 输入框。请先让目标对话的输入框处于前台并完成登记。松开录音不会自动发送。",
        "draft_not_inserted" => "这次识别还没有写入已登记输入框，未发送。",
        "draft_missing" => "已登记输入框里看不到这次识别文字，未发送。",
        "value_unreadable" => "无法读取已登记输入框的文字，未发送。",
        "clipboard_busy" => "剪贴板正被占用，已保留原剪贴板，未写入。",
        "write_unconfirmed" => "未能确认文字已写入目标输入框，未发送。请先查看该输入框，不要立刻重试，以免重复追加。",
        "input_failed" => "定向输入没有送达已登记进程，未改用全局按键，也未发送。",
        "focus_unavailable" => "读不到当前前台文本焦点，未输入。",
        _ => "无法确认 Grok Bot 输入目标，未写入也未发送。",
    };
    text.to_string()
}

fn map_platform_error(code: &str) -> String {
    let code = code.trim();
    match code {
        "unsupported_platform" | "permission_missing" | "role_unsupported" | "window_title_missing"
        | "identity_incomplete" | "focus_ambiguous" | "own_process" | "not_previewed"
        | "focus_changed" | "empty_text" | "text_too_long" | "unregistered" | "draft_not_inserted"
        | "draft_missing" | "value_unreadable" | "clipboard_busy" | "write_unconfirmed"
        | "input_failed" | "focus_unavailable" => user_message(code),
        _ => user_message("input_failed"),
    }
}

fn display_payload(identity: Option<&ComposerIdentity>, preview: bool) -> Value {
    let registered = identity.is_some() && !preview;
    json!({
        "ok": true,
        "supported": adapter_block_reason(current_host_os()).is_none(),
        "platform": current_host_os(),
        "permissionGranted": permission_granted(),
        "registered": registered || store().lock().ok().is_some_and(|slot| slot.committed.is_some()),
        "preview": preview,
        "processName": identity.map(|item| item.process_name.clone()).unwrap_or_default(),
        "windowTitle": identity.map(|item| item.window_title.clone()).unwrap_or_default(),
        "role": identity.map(|item| item.role.clone()).unwrap_or_default(),
    })
}

pub fn permission_granted() -> bool {
    if adapter_block_reason(current_host_os()).is_some() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        return crate::codex_composer::grok_bot_permission_granted();
    }
    #[cfg(windows)]
    {
        return true;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}

fn capture_live() -> Result<ComposerIdentity, String> {
    if let Some(code) = adapter_block_reason(current_host_os()) {
        return Err(code.into());
    }
    #[cfg(target_os = "macos")]
    {
        return crate::codex_composer::capture_grok_bot_identity();
    }
    #[cfg(windows)]
    {
        return capture_windows(None, "");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err("unsupported_platform".into())
    }
}

pub fn status_payload() -> Value {
    let committed = store().lock().ok().and_then(|slot| slot.committed.clone());
    let mut payload = display_payload(committed.as_ref(), false);
    if let Some(object) = payload.as_object_mut() {
        object.insert("registered".into(), json!(committed.is_some()));
        object.insert("preview".into(), json!(false));
        object.insert(
            "blockReason".into(),
            json!(adapter_block_reason(current_host_os()).map(user_message).unwrap_or_default()),
        );
    }
    payload
}

pub fn preview_focused(own_pid: i32) -> Result<Value, String> {
    let live = capture_live().map_err(|error| map_platform_error(&error))?;
    let mut slot = store().lock().map_err(|_| user_message("input_failed"))?;
    validate_for_registration(&live, own_pid).map_err(user_message)?;
    slot.preview = Some(live.clone());
    drop(slot);
    Ok(display_payload(Some(&live), true))
}

pub fn commit_focused(own_pid: i32) -> Result<Value, String> {
    let live = capture_live().map_err(|error| map_platform_error(&error))?;
    let mut slot = store().lock().map_err(|_| user_message("input_failed"))?;
    let Some(preview) = slot.preview.clone() else {
        return Err(user_message("not_previewed"));
    };
    validate_for_registration(&live, own_pid).map_err(user_message)?;
    if !identities_match(&preview, &live) {
        slot.preview = None;
        return Err(user_message("focus_changed"));
    }
    slot.committed = Some(live.clone());
    slot.preview = None;
    drop(slot);
    let mut payload = display_payload(Some(&live), false);
    if let Some(object) = payload.as_object_mut() {
        object.insert("registered".into(), json!(true));
        object.insert("preview".into(), json!(false));
    }
    Ok(payload)
}

pub fn clear_registration() -> Value {
    if let Ok(mut slot) = store().lock() {
        slot.preview = None;
        slot.committed = None;
    }
    status_payload()
}

pub fn registered_identity() -> Option<ComposerIdentity> {
    store().lock().ok().and_then(|slot| slot.committed.clone())
}

pub fn append_registered(text: &str) -> Result<(), String> {
    let registered = registered_identity().ok_or_else(|| user_message("unregistered"))?;
    let live = capture_live().map_err(|error| map_platform_error(&error))?;
    match on_release(Some(&registered), Some(&live), text) {
        ReleaseDecision::Refuse(code) => Err(user_message(code)),
        ReleaseDecision::Append => {
            #[cfg(target_os = "macos")]
            {
                return crate::codex_composer::append_grok_bot_text(&registered, text.trim())
                    .map_err(|error| map_platform_error(&error));
            }
            #[cfg(windows)]
            {
                return append_windows(&registered, text.trim());
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = (registered, live);
                Err(user_message("unsupported_platform"))
            }
        }
    }
}

pub fn submit_registered(text: &str) -> Result<(), String> {
    let registered = registered_identity().ok_or_else(|| user_message("unregistered"))?;
    let live = capture_live().map_err(|error| map_platform_error(&error))?;
    match on_confirm(Some(&registered), Some(&live), text, true) {
        ConfirmDecision::Refuse(code) => Err(user_message(code)),
        ConfirmDecision::Send => {
            #[cfg(target_os = "macos")]
            {
                return crate::codex_composer::submit_grok_bot_text(&registered, text.trim())
                    .map_err(|error| map_platform_error(&error));
            }
            #[cfg(windows)]
            {
                return submit_windows(&registered, text.trim());
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = (registered, live);
                Err(user_message("unsupported_platform"))
            }
        }
    }
}

pub const WINDOWS_BRIDGE_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
function Emit([hashtable]$fields) {
  $allowed = [ordered]@{ ok = [bool]$fields.ok; code = [string]$fields.code }
  foreach ($name in @('pid','foregroundPid','processName','role','subrole','identifier','windowTitle')) {
    if ($fields.ContainsKey($name) -and $null -ne $fields[$name]) { $allowed[$name] = [string]$fields[$name] }
  }
  if ($fields.ContainsKey('runtimeId') -and $null -ne $fields['runtimeId']) {
    $allowed['runtimeId'] = @($fields['runtimeId'] | ForEach-Object { [int]$_ })
  }
  Write-Output ($allowed | ConvertTo-Json -Compress)
}
if (-not ([System.Management.Automation.PSTypeName]'ForegroundWindowProbe').Type) {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ForegroundWindowProbe {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
}
'@
}
Add-Type -AssemblyName UIAutomationClient | Out-Null
Add-Type -AssemblyName UIAutomationTypes | Out-Null
Add-Type -AssemblyName System.Windows.Forms | Out-Null
function Read-Focus {
  $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
  if ($null -eq $focused) { throw 'focus_unavailable' }
  $focusedPid = [int]$focused.Current.ProcessId
  if ($focusedPid -le 0) { throw 'identity_incomplete' }
  $process = Get-Process -Id $focusedPid -ErrorAction Stop
  $processName = [string]$process.ProcessName
  if ([string]::IsNullOrWhiteSpace($processName)) { throw 'identity_incomplete' }
  $role = [string]$focused.Current.ControlType.ProgrammaticName
  $runtime = @($focused.GetRuntimeId() | ForEach-Object { [int]$_ })
  if ($runtime.Count -lt 1) { throw 'identity_incomplete' }
  $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
  $node = $focused
  $title = ''
  for ($depth = 0; $depth -lt 20 -and $null -ne $node; $depth++) {
    if ([string]$node.Current.ControlType.ProgrammaticName -eq 'ControlType.Window') {
      $title = [string]$node.Current.Name
      break
    }
    $parent = $walker.GetParent($node)
    if ($null -eq $parent) { break }
    $node = $parent
  }
  $foreground = [ForegroundWindowProbe]::GetForegroundWindow()
  $foregroundPidRaw = [uint32]0
  [void][ForegroundWindowProbe]::GetWindowThreadProcessId($foreground, [ref]$foregroundPidRaw)
  [pscustomobject]@{
    Element = $focused
    pid = $focusedPid
    foregroundPid = [int]$foregroundPidRaw
    processName = $processName.Trim()
    role = $role
    subrole = ''
    identifier = ''
    windowTitle = $title.Trim()
    runtimeId = $runtime
  }
}
function Same-Focus($live, $expected) {
  if ($null -eq $live -or $null -eq $expected) { return $false }
  $liveId = (@($live.runtimeId) | ForEach-Object { [string][int]$_ }) -join ','
  $expectedId = (@($expected.runtimeId) | ForEach-Object { [string][int]$_ }) -join ','
  return ([int]$live.pid -eq [int]$expected.pid -and
    [int]$live.foregroundPid -eq [int]$live.pid -and
    [string]::Equals([string]$live.processName, [string]$expected.processName, 'OrdinalIgnoreCase') -and
    [string]$live.role -eq [string]$expected.role -and
    [string]$live.windowTitle -eq [string]$expected.windowTitle -and
    $liveId -eq $expectedId -and $liveId -ne '')
}
$payloadPath = $args[0]
$payload = Get-Content -LiteralPath $payloadPath -Raw -Encoding UTF8 | ConvertFrom-Json
$action = [string]$payload.action
if ($action -eq 'capture') {
  $live = Read-Focus
  Emit @{ ok = $true; code = 'ready'; pid = $live.pid; foregroundPid = $live.foregroundPid; processName = $live.processName; role = $live.role; subrole = ''; identifier = ''; windowTitle = $live.windowTitle; runtimeId = $live.runtimeId }
  return
}
$live = Read-Focus
if (-not (Same-Focus $live $payload.expected)) { Emit @{ ok = $false; code = 'focus_changed' }; return }
if ($action -eq 'append') {
    $clipboard = $null
    $captured = $false
    $mutated = $false
    try {
      $again = Read-Focus
      if (-not (Same-Focus $again $payload.expected)) { throw 'focus_changed' }
      $clipboard = [System.Windows.Forms.Clipboard]::GetDataObject()
      $captured = $true
      $mutated = $true
      [System.Windows.Forms.Clipboard]::SetText([string]$payload.text)
      $third = Read-Focus
      if (-not (Same-Focus $third $payload.expected)) { throw 'focus_changed' }
      [System.Windows.Forms.SendKeys]::SendWait('^v')
      $pattern = $null
      try { $pattern = $third.Element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern) } catch { $pattern = $null }
      if ($null -eq $pattern) { Emit @{ ok = $false; code = 'write_unconfirmed' }; return }
      $combined = [string]$pattern.Current.Value
      if (-not $combined.EndsWith([string]$payload.text)) { Emit @{ ok = $false; code = 'write_unconfirmed' }; return }
      Emit @{ ok = $true; code = 'appended' }
    } catch {
    $code = [string]$_.Exception.Message
    if ($code -ne 'focus_changed' -and $code -ne 'clipboard_busy') { $code = 'input_failed' }
    Emit @{ ok = $false; code = $code }
  } finally {
    if ($captured -and $mutated -and $null -ne $clipboard) {
      [System.Windows.Forms.Clipboard]::SetDataObject($clipboard, $true) | Out-Null
    }
  }
  return
}
if ($action -eq 'submit') {
  $pattern = $null
  try { $pattern = $live.Element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern) } catch { $pattern = $null }
  if ($null -eq $pattern) { Emit @{ ok = $false; code = 'value_unreadable' }; return }
  $value = [string]$pattern.Current.Value
  $pending = [string]$payload.text
  if ([string]::IsNullOrWhiteSpace($pending) -or -not $value.EndsWith($pending)) { Emit @{ ok = $false; code = 'draft_missing' }; return }
  $again = Read-Focus
  if (-not (Same-Focus $again $payload.expected)) { Emit @{ ok = $false; code = 'focus_changed' }; return }
  [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
  Emit @{ ok = $true; code = 'submitted' }
  return
}
Emit @{ ok = $false; code = 'input_failed' }
"#;

#[cfg(windows)]
fn windows_expected(identity: &ComposerIdentity) -> Value {
    json!({
        "pid": identity.pid,
        "foregroundPid": identity.foreground_pid,
        "processName": identity.process_name,
        "role": identity.role,
        "windowTitle": identity.window_title,
        "runtimeId": identity.runtime_id,
    })
}

#[cfg(windows)]
fn run_windows_bridge(action: &str, text: &str, expected: Option<&ComposerIdentity>) -> Result<Value, String> {
    let directory = std::env::temp_dir();
    let token = uuid::Uuid::new_v4();
    let script_path = directory.join(format!("hachimodock-foreground-{token}.ps1"));
    let payload_path = directory.join(format!("hachimodock-foreground-{token}.json"));
    let _guard = FileCleanup { paths: vec![script_path.clone(), payload_path.clone()] };
    std::fs::write(&script_path, WINDOWS_BRIDGE_SCRIPT).map_err(|_| "input_failed".to_string())?;
    let payload = json!({
        "action": action,
        "text": text,
        "expected": expected.map(windows_expected),
    });
    std::fs::write(&payload_path, payload.to_string()).map_err(|_| "input_failed".to_string())?;
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell.exe")
        .creation_flags(0x08000000)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .arg(&payload_path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| "input_failed".to_string())?;
    if !output.status.success() {
        return Err("input_failed".into());
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| "input_failed".to_string())?;
    let line = stdout.lines().rev().find(|line| line.trim_start().starts_with('{')).unwrap_or("");
    serde_json::from_str(line).map_err(|_| "input_failed".to_string())
}

#[cfg(windows)]
struct FileCleanup {
    paths: Vec<std::path::PathBuf>,
}

#[cfg(windows)]
impl Drop for FileCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(windows)]
fn identity_from_windows(value: &Value) -> Result<ComposerIdentity, String> {
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(value.get("code").and_then(Value::as_str).unwrap_or("input_failed").to_string());
    }
    let runtime_id = value
        .get("runtimeId")
        .and_then(Value::as_array)
        .ok_or("identity_incomplete")?
        .iter()
        .map(|part| part.as_i64().and_then(|number| i32::try_from(number).ok()).ok_or("identity_incomplete"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ComposerIdentity {
        pid: value.get("pid").and_then(Value::as_str).and_then(|text| text.parse().ok())
            .or_else(|| value.get("pid").and_then(Value::as_i64).and_then(|number| i32::try_from(number).ok()))
            .ok_or("identity_incomplete")?,
        foreground_pid: value.get("foregroundPid").and_then(Value::as_str).and_then(|text| text.parse().ok())
            .or_else(|| value.get("foregroundPid").and_then(Value::as_i64).and_then(|number| i32::try_from(number).ok()))
            .ok_or("identity_incomplete")?,
        process_name: value.get("processName").and_then(Value::as_str).unwrap_or("").to_string(),
        role: value.get("role").and_then(Value::as_str).unwrap_or("").to_string(),
        subrole: String::new(),
        identifier: String::new(),
        window_title: value.get("windowTitle").and_then(Value::as_str).unwrap_or("").to_string(),
        element_hash: 0,
        window_hash: 0,
        runtime_id,
    })
}

#[cfg(windows)]
fn capture_windows(_expected: Option<&ComposerIdentity>, _text: &str) -> Result<ComposerIdentity, String> {
    let value = run_windows_bridge("capture", "", None)?;
    identity_from_windows(&value)
}

#[cfg(windows)]
fn append_windows(expected: &ComposerIdentity, text: &str) -> Result<(), String> {
    let value = run_windows_bridge("append", text, Some(expected))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(value.get("code").and_then(Value::as_str).unwrap_or("input_failed").to_string())
    }
}

#[cfg(windows)]
fn submit_windows(expected: &ComposerIdentity, text: &str) -> Result<(), String> {
    let value = run_windows_bridge("submit", text, Some(expected))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(value.get("code").and_then(Value::as_str).unwrap_or("input_failed").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: i32, title: &str) -> ComposerIdentity {
        ComposerIdentity {
            pid,
            foreground_pid: pid,
            process_name: "Reported App".into(),
            role: "AXTextArea".into(),
            subrole: "AXStandardText".into(),
            identifier: "composer".into(),
            window_title: title.into(),
            element_hash: 11,
            window_hash: 22,
            runtime_id: Vec::new(),
        }
    }

    #[test]
    fn release_appends_only_for_the_same_nonempty_target_and_never_sends() {
        let target = sample(10, "Travel notes");
        assert!(!RELEASE_CONFIRMS_SEND);
        assert_eq!(on_release(Some(&target), Some(&target), "book the train"), ReleaseDecision::Append);
        assert_eq!(on_release(None, Some(&target), "book the train"), ReleaseDecision::Refuse("unregistered"));
        assert_eq!(on_release(Some(&target), None, "book the train"), ReleaseDecision::Refuse("focus_unavailable"));
        assert_eq!(on_release(Some(&target), Some(&target), "   "), ReleaseDecision::Refuse("empty_text"));
        let mut other = target.clone();
        other.window_title = "Other chat".into();
        assert_eq!(on_release(Some(&target), Some(&other), "book the train"), ReleaseDecision::Refuse("focus_changed"));
        other.window_title = target.window_title.clone();
        other.pid = 11;
        assert_eq!(on_release(Some(&target), Some(&other), "hello"), ReleaseDecision::Refuse("focus_changed"));
        other.pid = target.pid;
        other.element_hash = 99;
        assert_eq!(on_release(Some(&target), Some(&other), "hello"), ReleaseDecision::Refuse("focus_changed"));
    }

    #[test]
    fn confirm_requires_the_same_composer_and_inserted_text() {
        let target = sample(10, "Travel notes");
        assert_eq!(on_confirm(Some(&target), Some(&target), "book the train", true), ConfirmDecision::Send);
        assert_eq!(on_confirm(Some(&target), Some(&target), "book the train", false), ConfirmDecision::Refuse("draft_not_inserted"));
        assert_eq!(on_confirm(Some(&target), Some(&target), "", true), ConfirmDecision::Refuse("empty_text"));
        let mut moved = target.clone();
        moved.foreground_pid = 0;
        assert_eq!(on_confirm(Some(&target), Some(&moved), "book the train", true), ConfirmDecision::Refuse("focus_changed"));
        assert_eq!(on_confirm(None, Some(&target), "book the train", true), ConfirmDecision::Refuse("unregistered"));
    }

    #[test]
    fn registration_rejects_ambiguous_own_window_and_untitled_targets() {
        let own = sample(42, "Travel notes");
        assert_eq!(validate_for_registration(&own, 42), Err("own_process"));
        let mut background = own.clone();
        background.foreground_pid = 7;
        assert_eq!(validate_for_registration(&background, 1), Err("focus_ambiguous"));
        let mut untitled = own.clone();
        untitled.window_title.clear();
        assert_eq!(validate_for_registration(&untitled, 1), Err("window_title_missing"));
        let mut role = own.clone();
        role.role = "AXButton".into();
        assert_eq!(validate_for_registration(&role, 1), Err("role_unsupported"));
        let mut path = own.clone();
        path.process_name = "C:/secret/app".into();
        assert_eq!(validate_for_registration(&path, 1), Err("identity_incomplete"));
        assert!(validate_for_registration(&own, 1).is_ok());
    }

    #[test]
    fn windows_runtime_identity_does_not_fall_back_to_a_title_match() {
        let mut registered = sample(4, "Same title");
        registered.role = "ControlType.Edit".into();
        registered.element_hash = 0;
        registered.window_hash = 0;
        registered.runtime_id = vec![3, 9];
        let mut live = registered.clone();
        live.runtime_id = vec![3, 8];
        assert!(!identities_match(&registered, &live));
        live.runtime_id = vec![3, 9];
        assert!(identities_match(&registered, &live));
        assert_eq!(adapter_block_reason("linux"), Some("unsupported_platform"));
        assert_eq!(adapter_block_reason("macos"), None);
        assert_eq!(adapter_block_reason("windows"), None);
    }

    #[test]
    fn clipboard_must_be_restored_after_a_mutation() {
        assert!(clipboard_transaction_accepted(ClipboardTransaction { captured: true, mutated: true, restored: true }));
        assert!(clipboard_transaction_accepted(ClipboardTransaction { captured: false, mutated: false, restored: false }));
        assert!(!clipboard_transaction_accepted(ClipboardTransaction { captured: true, mutated: true, restored: false }));
        assert!(!clipboard_transaction_accepted(ClipboardTransaction { captured: false, mutated: true, restored: true }));
    }

    #[test]
    fn preview_commit_and_clear_follow_the_live_control() {
        let _guard = store().lock().unwrap();
        // The guard above would deadlock if preview locks the same mutex.
        drop(_guard);
        clear_registration();
        let first = sample(15, "First conversation");
        let second = sample(15, "Second conversation");
        let mut slot = store().lock().unwrap();
        assert!(validate_for_registration(&first, 1).is_ok());
        slot.preview = Some(first.clone());
        assert!(slot.committed.is_none());
        drop(slot);
        assert_eq!(on_release(registered_identity().as_ref(), Some(&first), "text"), ReleaseDecision::Refuse("unregistered"));
        let mut slot = store().lock().unwrap();
        assert!(identities_match(slot.preview.as_ref().unwrap(), &first));
        slot.committed = Some(first.clone());
        slot.preview = None;
        drop(slot);
        assert_eq!(on_release(registered_identity().as_ref(), Some(&first), "text"), ReleaseDecision::Append);
        assert_eq!(on_release(registered_identity().as_ref(), Some(&second), "text"), ReleaseDecision::Refuse("focus_changed"));
        clear_registration();
        assert!(registered_identity().is_none());
    }

    #[test]
    fn windows_bridge_is_targeted_and_does_not_name_an_app_identity() {
        let script = WINDOWS_BRIDGE_SCRIPT;
        assert!(!script.to_ascii_lowercase().contains("grok"));
        assert!(!script.contains("anysphere"));
        assert!(!script.contains("bundle"));
        assert!(script.contains("Same-Focus"));
        assert!(script.contains("SetDataObject"));
        assert!(script.contains("finally"));
        let paste = script.find("SendWait('^v')").unwrap();
        let enter = script.find("SendWait('{ENTER}')").unwrap();
        let compare = script.find("Same-Focus").unwrap();
        assert!(compare < paste && paste < enter);
        assert!(!script.contains("api.x.ai"));
        assert!(!script.contains("Authorization"));
        assert!(script.contains("ValuePattern"));
        assert!(!script.contains("Write-Output $value"));
        assert!(!script.contains("$payload.text |"));
    }

    #[test]
    fn native_bridge_does_not_prompt_for_permission_or_use_global_events() {
        let macos = include_str!("codex_composer_macos.rs");
        let function_body = |name: &str| {
            let start = macos.find(&format!("fn {name}(")).unwrap();
            let rest = &macos[start + 1..];
            let next_fn = rest.find("\nfn ").unwrap_or(rest.len());
            let next_pub = rest.find("\npub(super) fn ").unwrap_or(rest.len());
            &macos[start..start + 1 + next_fn.min(next_pub)]
        };
        let append = function_body("append_grok_bot_text");
        let submit = function_body("submit_grok_bot_text");
        let capture = function_body("read_focused_composer_identity");
        for body in [append, submit, capture] {
            assert!(!body.contains("request_accessibility_permission"));
            assert!(!body.contains("AXIsProcessTrustedWithOptions"));
            assert!(!body.contains("post_keyboard_event"));
            assert!(!body.contains("CGEventTapLocation::HID"));
        }
        assert!(append.contains("snapshot.restore()"));
        assert!(append.contains("post_workbuddy_key"));
        assert!(submit.contains("post_workbuddy_key"));
        assert!(!macos.split("fn agent_bundle_identifiers(").nth(1).unwrap().split("\nfn ").next().unwrap().to_ascii_lowercase().contains("grok"));
        let lib = include_str!("lib.rs");
        let visible = lib.split("fn agent_uses_visible_composer(").nth(1).unwrap().split("\nfn ").next().unwrap();
        assert!(!visible.contains("grok-bot"));
        let confirm = lib.split("fn confirm_pending_device_voice_draft(").nth(1).unwrap();
        let grok_confirm = confirm.find("grok_bot_voice::AGENT_ID").unwrap();
        let bus = confirm.find("submit_device_voice_via_agent_bus").unwrap();
        assert!(grok_confirm < bus);
        let finalize = lib.split("fn finalize_device_voice_final(").nth(1).unwrap().split("fn input_event_matches_device_voice_confirm(").next().unwrap();
        assert!(finalize.contains("deliver_grok_bot_voice_final"));
        assert!(!lib.contains("api.x.ai"));
    }
}
