//! Read-only Bambu status for the existing `widget/data` list channel.
//!
//! Official notes say MQTT status pushes stay available and list LAN MQTT 8883
//! and LAN FTP 990. They do not publish the report schema, a current-task
//! thumbnail, or permission for this app to log into the cloud. This module
//! does not open a socket, send a printer packet, or call a cloud API.
//! Reports are ingested only by tests and by a future subscribe-only client.

use crate::widget_data::{bounded_text, DataRow, DataSnapshot};
use serde_json::{json, Value};
use tauri::Manager;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const SOURCE: &str = "printer.bambu";
const SCHEMA_VERSION: u32 = 1;
const TTL_MS: u32 = 30_000;
const STALE_MS: u64 = 30_000;
const MAX_ARCHIVE_ENTRIES: u32 = 32;
const MAX_ARCHIVE_BYTES: u64 = 2_000_000;
const MAX_IMAGE_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    H2c,
    H3c,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Printing,
    Paused,
    Finished,
    Cancelled,
    Failed,
    Idle,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation<T> {
    value: T,
    observed_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Temperature {
    role: &'static str,
    source_key: &'static str,
    celsius: f64,
    observed_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    session_id: String,
    source_task_id: String,
    name: Option<String>,
    plate: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Alert {
    id: String,
    kind: &'static str,
    job_session_id: String,
    source_code: String,
}

#[derive(Clone, Debug)]
pub struct Session {
    model: Model,
    epoch: u64,
    connected: bool,
    configured: bool,
    seq: u64,
    received_at_ms: u64,
    saw_non_terminal: bool,
    phase: Option<Observation<Phase>>,
    progress: Option<Observation<u8>>,
    remaining_seconds: Option<Observation<u32>>,
    remaining_raw_unverified: Option<i64>,
    temperatures: Vec<Temperature>,
    job: Option<Job>,
    job_counter: u64,
    alerts: Vec<Alert>,
    acked: BTreeMap<String, bool>,
    last_report_hash: u64,
}

impl Session {
    pub fn new(model: Model) -> Self {
        Self {
            model,
            epoch: 0,
            connected: false,
            configured: false,
            seq: 0,
            received_at_ms: 0,
            saw_non_terminal: false,
            phase: None,
            progress: None,
            remaining_seconds: None,
            remaining_raw_unverified: None,
            temperatures: Vec::new(),
            job: None,
            job_counter: 0,
            alerts: Vec::new(),
            acked: BTreeMap::new(),
            last_report_hash: 0,
        }
    }

    pub fn configure(&mut self) {
        self.configured = true;
    }

    pub fn link_up(&mut self, now_ms: u64) {
        self.epoch = self.epoch.saturating_add(1);
        self.connected = true;
        self.saw_non_terminal = false;
        self.received_at_ms = now_ms;
        self.last_report_hash = 0;
    }

    pub fn link_down(&mut self, now_ms: u64) {
        self.connected = false;
        self.received_at_ms = now_ms;
    }

    pub fn ingest(&mut self, report: &Value, now_ms: u64) -> Result<(), &'static str> {
        if report.get("cloud").is_some() || report.get("login").is_some() {
            return Err("cloud_unsupported");
        }
        let print = report.get("print").ok_or("report_missing")?;
        if !print.is_object() {
            return Err("report_missing");
        }
        let hash = hash_value(print);
        if hash == self.last_report_hash {
            return Ok(());
        }
        self.last_report_hash = hash;
        self.seq = self.seq.saturating_add(1);
        self.received_at_ms = now_ms;
        let task_id = text_field(print, &["subtask_id", "task_id"]);
        let name = text_field(print, &["subtask_name"]);
        let plate = plate_index(print);
        self.bind_job(task_id.as_deref(), name.as_deref(), plate);
        if let Some(state) = text_field(print, &["gcode_state"]) {
            if let Some(phase) = phase_for_model(self.model, &state, print) {
                let previous = self.phase.as_ref().map(|item| item.value);
                let code = error_code(print);
                self.note_transition(previous, phase, &code);
                if !matches!(phase, Phase::Finished | Phase::Failed | Phase::Cancelled) {
                    self.saw_non_terminal = true;
                }
                self.phase = Some(Observation { value: phase, observed_at_ms: now_ms });
            }
        }
        if let Some(percent) = whole_percent(print.get("mc_percent")) {
            self.progress = Some(Observation { value: percent, observed_at_ms: now_ms });
        }
        if let Some(seconds) = explicit_seconds(print.get("remaining_seconds")) {
            self.remaining_seconds = Some(Observation { value: seconds, observed_at_ms: now_ms });
        }
        if let Some(raw) = print.get("mc_remaining_time").and_then(Value::as_i64) {
            self.remaining_raw_unverified = Some(raw);
        }
        self.read_temperatures(print, now_ms);
        Ok(())
    }

    pub fn ack(&mut self, alert_id: &str) {
        if self.alerts.iter().any(|alert| alert.id == alert_id) {
            self.acked.insert(alert_id.to_string(), true);
        }
    }

    fn bind_job(&mut self, task_id: Option<&str>, name: Option<&str>, plate: Option<u16>) {
        let Some(task_id) = task_id.filter(|value| !value.is_empty()) else {
            if self.job.as_ref().is_some_and(|job| !job.source_task_id.is_empty()) {
                self.clear_job();
            }
            return;
        };
        let same = self.job.as_ref().is_some_and(|job| job.source_task_id == task_id);
        if !same {
            self.start_job(task_id, name, plate);
            return;
        }
        let job = self.job.as_mut().unwrap();
        if job.plate != plate && plate.is_some() {
            let id = task_id.to_string();
            self.start_job(&id, name, plate);
            return;
        }
        if job.name.is_none() {
            job.name = name.map(str::to_string);
        }
    }

    fn start_job(&mut self, task_id: &str, name: Option<&str>, plate: Option<u16>) {
        self.job_counter = self.job_counter.saturating_add(1);
        self.job = Some(Job {
            session_id: format!("job-{}-{}", self.epoch, self.job_counter),
            source_task_id: task_id.to_string(),
            name: name.map(str::to_string),
            plate,
        });
        self.saw_non_terminal = false;
        self.progress = None;
        self.remaining_seconds = None;
        self.remaining_raw_unverified = None;
    }

    fn clear_job(&mut self) {
        self.job = None;
        self.progress = None;
        self.remaining_seconds = None;
        self.remaining_raw_unverified = None;
    }

    fn note_transition(&mut self, previous: Option<Phase>, next: Phase, code: &str) {
        let Some(job) = self.job.clone() else { return };
        if previous == Some(next) {
            return;
        }
        if matches!(previous, Some(Phase::Finished | Phase::Idle | Phase::Cancelled))
            && matches!(next, Phase::Printing | Phase::Preparing)
        {
            let task = job.source_task_id.clone();
            let name = job.name.clone();
            let plate = job.plate;
            self.start_job(&task, name.as_deref(), plate);
            return;
        }
        if next == Phase::Finished && self.saw_non_terminal && self.connected {
            self.push_alert("completed", &job.session_id, "");
        }
        if next == Phase::Failed && self.saw_non_terminal && self.connected {
            self.push_alert("error", &job.session_id, code);
        }
    }

    fn push_alert(&mut self, kind: &'static str, job_session_id: &str, code: &str) {
        if self.alerts.iter().any(|alert| alert.kind == kind && alert.job_session_id == job_session_id) {
            return;
        }
        let id = format!("{kind}-{}", self.alerts.len() + 1);
        self.alerts.push(Alert { id, kind, job_session_id: job_session_id.to_string(), source_code: code.to_string() });
    }

    fn read_temperatures(&mut self, print: &Value, now_ms: u64) {
        let mut next = Vec::new();
        if let Some(value) = finite_temp(print.get(match self.model {
            Model::H2c => "nozzle_temper",
            Model::H3c => "nozzle_temper",
        })) {
            next.push(Temperature {
                role: match self.model {
                    Model::H2c => "reported_nozzle",
                    Model::H3c => "current_tool",
                },
                source_key: "nozzle_temper",
                celsius: value,
                observed_at_ms: now_ms,
            });
        }
        if let Some(value) = finite_temp(print.get("bed_temper")) {
            next.push(Temperature { role: "bed", source_key: "bed_temper", celsius: value, observed_at_ms: now_ms });
        }
        if let Some(value) = finite_temp(print.get("chamber_temper")) {
            next.push(Temperature { role: "chamber", source_key: "chamber_temper", celsius: value, observed_at_ms: now_ms });
        }
        if next.is_empty() {
            return;
        }
        for item in next {
            if let Some(existing) = self.temperatures.iter_mut().find(|current| current.role == item.role) {
                *existing = item;
            } else {
                self.temperatures.push(item);
            }
        }
    }

    pub fn contract(&self, now_ms: u64) -> Value {
        let stale = !self.connected || now_ms.saturating_sub(self.received_at_ms) > STALE_MS;
        let phase = self.phase.as_ref().map(|item| phase_name(item.value));
        let preview = if self.job.is_none() { "empty" } else { "unavailable" };
        let preview_reason = match self.model {
            Model::H3c => "h3c_thumbnail_unsupported",
            Model::H2c => "h2c_no_reliable_current_plate_image",
        };
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "origin": "live",
            "source": SOURCE,
            "model": model_name(self.model),
            "capabilities": {
                "cloud": "unsupported",
                "lanMqttSubscribe": "not_started",
                "thumbnail": "unsupported",
                "control": "disabled"
            },
            "connectionEpoch": self.epoch,
            "seq": self.seq,
            "receivedAtMs": self.received_at_ms,
            "stale": stale || !self.configured,
            "completeness": if self.phase.is_some() { "partial" } else { "empty" },
            "phase": phase,
            "phaseObservedAtMs": self.phase.as_ref().map(|item| item.observed_at_ms),
            "progressPercent": self.progress.as_ref().map(|item| item.value),
            "progressObservedAtMs": self.progress.as_ref().map(|item| item.observed_at_ms),
            "remainingSeconds": self.remaining_seconds.as_ref().map(|item| item.value),
            "remainingObservedAtMs": self.remaining_seconds.as_ref().map(|item| item.observed_at_ms),
            "remainingUnitUnverified": self.remaining_raw_unverified.is_some(),
            "temperatures": self.temperatures.iter().map(|item| json!({
                "role": item.role,
                "sourceKey": item.source_key,
                "celsius": item.celsius,
                "observedAtMs": item.observed_at_ms
            })).collect::<Vec<_>>(),
            "job": self.job.as_ref().map(|job| json!({
                "jobSessionId": job.session_id,
                "sourceTaskId": job.source_task_id,
                "name": job.name,
                "plate": job.plate
            })),
            "preview": {
                "state": preview,
                "reason": if self.job.is_none() { "no_job" } else { preview_reason },
                "assetRef": Value::Null,
                "matchedJobSessionId": Value::Null,
                "plate": Value::Null
            },
            "alerts": self.alerts.iter().filter(|alert| !self.acked.contains_key(&alert.id)).map(|alert| json!({
                "id": alert.id,
                "kind": alert.kind,
                "jobSessionId": alert.job_session_id,
                "sourceCode": alert.source_code
            })).collect::<Vec<_>>(),
        })
    }

    pub fn device_snapshot(&self) -> DataSnapshot {
        let contract = self.contract(self.received_at_ms.saturating_add(if self.connected { 0 } else { STALE_MS + 1 }));
        let stale = contract["stale"].as_bool().unwrap_or(true);
        let phase = contract["phase"].as_str().unwrap_or("unknown");
        let progress = contract["progressPercent"].as_u64().map(|value| format!("{value}%")).unwrap_or_else(|| "—".into());
        let remaining = contract["remainingSeconds"].as_u64().map(|seconds| format!("{seconds} 秒")).unwrap_or_else(|| "—".into());
        let nozzle = temp_text(&contract, match self.model { Model::H2c => "reported_nozzle", Model::H3c => "current_tool" });
        let rows = vec![
            row("phase", "状态", phase_label(phase), if stale { "非实时" } else { "" }),
            row("progress", "进度", &progress, ""),
            row("remaining", "剩余", &remaining, if contract["remainingUnitUnverified"].as_bool() == Some(true) { "原单位未核" } else { "" }),
            row("nozzle", "喷嘴", &nozzle, match self.model { Model::H2c => "未分左右" , Model::H3c => "当前工具" }),
            row("bed", "热床", &temp_text(&contract, "bed"), ""),
            row("chamber", "热室", &temp_text(&contract, "chamber"), ""),
            row("preview", "缩略图", if contract["preview"]["state"] == "empty" { "无任务" } else { "无预览" }, "未取图"),
        ];
        DataSnapshot {
            schema: 1,
            date: String::new(),
            source: SOURCE.into(),
            ttl_ms: TTL_MS,
            status: if !self.configured { "empty".into() } else if stale { "error".into() } else { "ok".into() },
            message: if !self.configured {
                "尚未配置局域网打印机".into()
            } else if !self.connected {
                "只读订阅尚未连接".into()
            } else {
                String::new()
            },
            rows,
        }
    }
}

fn row(id: &str, label: &str, value: &str, detail: &str) -> DataRow {
    DataRow {
        id: bounded_text(id, 23),
        label: bounded_text(label, 36),
        value: bounded_text(value, 24),
        detail: bounded_text(detail, 24),
        meta: String::new(),
        tone: 0,
    }
}

fn phase_for_model(model: Model, state: &str, print: &Value) -> Option<Phase> {
    let _ = model;
    let phase = match state {
        "IDLE" => Phase::Idle,
        "RUNNING" => Phase::Printing,
        "PAUSE" => Phase::Paused,
        "FINISH" => Phase::Finished,
        "PREPARE" => Phase::Preparing,
        "FAILED" => {
            let coded = print.get("print_error").and_then(Value::as_i64).unwrap_or(0) != 0
                || print.get("hms").and_then(Value::as_array).is_some_and(|rows| !rows.is_empty());
            if coded { Phase::Failed } else { Phase::Unknown }
        }
        "CANCEL" | "CANCELED" | "CANCELLED" => Phase::Cancelled,
        _ => return None,
    };
    Some(phase)
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Preparing => "preparing",
        Phase::Printing => "printing",
        Phase::Paused => "paused",
        Phase::Finished => "finished",
        Phase::Cancelled => "cancelled",
        Phase::Failed => "failed",
        Phase::Idle => "idle",
        Phase::Unknown => "unknown",
    }
}

fn phase_label(phase: &str) -> &'static str {
    match phase {
        "preparing" => "准备中",
        "printing" => "打印中",
        "paused" => "已暂停",
        "finished" => "打印完成",
        "cancelled" => "已取消",
        "failed" => "打印异常",
        "idle" => "空闲",
        _ => "未知",
    }
}

fn model_name(model: Model) -> &'static str {
    match model { Model::H2c => "H2C", Model::H3c => "H3C" }
}

fn text_field(print: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        print.get(*key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty() && value.len() <= 160 && !value.chars().any(|ch| ch.is_control())).map(str::to_string)
    })
}

fn error_code(print: &Value) -> String {
    if let Some(code) = print.get("print_error").and_then(Value::as_i64).filter(|code| *code != 0) {
        return format!("print_error:{code}");
    }
    if let Some(code) = print.get("hms").and_then(Value::as_array).and_then(|rows| rows.first()).and_then(Value::as_str) {
        return format!("hms:{code}");
    }
    String::new()
}

fn plate_index(print: &Value) -> Option<u16> {
    let value = print.get("plate_idx")?.as_u64()?;
    u16::try_from(value).ok().filter(|plate| (1..=36).contains(plate))
}

fn whole_percent(value: Option<&Value>) -> Option<u8> {
    let number = value?.as_u64()?;
    u8::try_from(number).ok().filter(|percent| *percent <= 100)
}

fn explicit_seconds(value: Option<&Value>) -> Option<u32> {
    let number = value?.as_u64()?;
    u32::try_from(number).ok().filter(|seconds| *seconds <= 7 * 24 * 3600)
}

fn finite_temp(value: Option<&Value>) -> Option<f64> {
    let number = value?.as_f64()?;
    (number.is_finite() && (0.0..=400.0).contains(&number)).then_some(number)
}

fn temp_text(contract: &Value, role: &str) -> String {
    contract["temperatures"].as_array().and_then(|rows| {
        rows.iter().find(|item| item["role"] == role).and_then(|item| item["celsius"].as_f64())
    }).map(|value| format!("{value:.0}°C")).unwrap_or_else(|| "—".into())
}

fn hash_value(value: &Value) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let text = value.to_string();
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn admit_plate_entry(path: &str, declared_len: u64, entries: &mut u32, bytes: &mut u64) -> Result<u16, &'static str> {
    if path.is_empty() || path.len() > 80 || path.contains('\\') || path.starts_with('/') || path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("path_rejected");
    }
    let plate = path.strip_prefix("Metadata/plate_").and_then(|rest| rest.strip_suffix(".png")).and_then(|number| number.parse::<u16>().ok()).filter(|plate| (1..=36).contains(plate)).ok_or("not_plate_png")?;
    *entries = entries.saturating_add(1);
    *bytes = bytes.saturating_add(declared_len);
    if *entries > MAX_ARCHIVE_ENTRIES || *bytes > MAX_ARCHIVE_BYTES || declared_len > MAX_ARCHIVE_BYTES {
        return Err("archive_budget");
    }
    Ok(plate)
}

pub fn inspect_image(bytes: &[u8]) -> Result<(u32, u32), &'static str> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES || bytes.starts_with(b"<") || bytes.starts_with(b"{") {
        return Err("image_rejected");
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(640);
    limits.max_image_height = Some(480);
    limits.max_alloc = Some(8 * 1024 * 1024);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|_| "image_rejected")?;
    reader.limits(limits);
    let image = reader.decode().map_err(|_| "image_rejected")?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width > 640 || height > 480 {
        return Err("image_bounds");
    }
    Ok((width, height))
}

pub fn decide_preview(matched_job: bool, matched_plate: bool, demo: bool, newest_file: bool) -> &'static str {
    if demo || newest_file || !matched_job || !matched_plate { "unavailable" } else { "available" }
}

fn store() -> &'static Mutex<Session> {
    static STORE: OnceLock<Mutex<Session>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Session::new(Model::H2c)))
}

pub fn should_publish() -> bool {
    store().lock().is_ok_and(|session| session.configured)
}

pub fn device_snapshot() -> DataSnapshot {
    store().lock().map(|session| session.device_snapshot()).unwrap_or_else(|_| Session::new(Model::H2c).device_snapshot())
}

#[tauri::command]
pub fn printer_bambu_snapshot() -> Value {
    store().lock().map(|session| session.contract(session.received_at_ms)).unwrap_or_else(|_| json!({"schemaVersion": SCHEMA_VERSION, "origin": "live", "stale": true}))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterConfigInput {
    pub model: String,
    pub host: String,
    pub serial: String,
    pub access_code: String,
}

#[tauri::command]
pub fn printer_bambu_configure(app: tauri::AppHandle, input: PrinterConfigInput) -> Result<Value, String> {
    let dir = app.path().app_config_dir().map_err(|_| "配置目录不可用".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|_| "配置目录不可用".to_string())?;
    configure_in(&dir, &input.model, &input.host, &input.serial, &input.access_code)
}

#[tauri::command]
pub fn printer_bambu_forget(app: tauri::AppHandle) -> Result<Value, String> {
    let dir = app.path().app_config_dir().map_err(|_| "配置目录不可用".to_string())?;
    forget_in(&dir)
}

pub fn configure_in(dir: &Path, model: &str, host: &str, serial: &str, access_code: &str) -> Result<Value, String> {
    let model = parse_model(model)?;
    validate_host(host)?;
    validate_token(serial, 8, 32)?;
    validate_token(access_code, 8, 8)?;
    let stored = json!({"model": model_name(model), "host": host, "serial": serial, "accessCode": access_code});
    let raw = serde_json::to_vec(&stored).map_err(|_| "配置无法保存".to_string())?;
    crate::secure_config::write(&secret_path(dir), "printer-bambu", &raw)?;
    let mut session = store().lock().map_err(|_| "状态繁忙".to_string())?;
    *session = Session::new(model);
    session.configure();
    Ok(public_status(model, host, serial, true))
}

pub fn forget_in(dir: &Path) -> Result<Value, String> {
    let path = secret_path(dir);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|_| "配置无法删除".to_string())?;
    }
    let mut session = store().lock().map_err(|_| "状态繁忙".to_string())?;
    *session = Session::new(Model::H2c);
    Ok(json!({"configured": false, "accessCodeStored": false, "socket": "not_started", "thumbnail": "unsupported", "cloud": "unsupported"}))
}

fn public_status(model: Model, host: &str, serial: &str, stored: bool) -> Value {
    json!({
        "configured": true,
        "model": model_name(model),
        "host": host,
        "serialRedacted": redact_serial(serial),
        "accessCodeStored": stored,
        "socket": "not_started",
        "cloud": "unsupported",
        "thumbnail": "unsupported",
        "control": "disabled"
    })
}

fn secret_path(dir: &Path) -> PathBuf {
    dir.join("printer-bambu.secure.json")
}

fn parse_model(value: &str) -> Result<Model, String> {
    match value {
        "H2C" => Ok(Model::H2c),
        "H3C" => Ok(Model::H3c),
        _ => Err("只接受 H2C 或 H3C".into()),
    }
}

fn validate_host(host: &str) -> Result<(), String> {
    if host.contains(':') || host.contains('/') || host.contains('\\') {
        return Err("只接受局域网 IPv4 地址".into());
    }
    let address: Ipv4Addr = host.parse().map_err(|_| "只接受局域网 IPv4 地址".to_string())?;
    if address.is_unspecified() || address.is_multicast() || address.is_broadcast() || address.octets() == [0, 0, 0, 0] {
        return Err("只接受局域网 IPv4 地址".into());
    }
    Ok(())
}

fn validate_token(value: &str, min: usize, max: usize) -> Result<(), String> {
    let count = value.chars().count();
    if count < min || count > max || !value.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Err("打印机标识或访问码格式无效".into());
    }
    Ok(())
}

fn redact_serial(serial: &str) -> String {
    let tail: String = serial.chars().rev().take(4).collect::<String>().chars().rev().collect();
    format!("••••{tail}")
}

#[cfg(test)]
mod tests {
    //! Fixture reports only. Nothing here contacts a printer or uses a saved access code as a live session.
    use super::*;
    use serde_json::json;

    fn report(state: &str, task: &str, extra: Value) -> Value {
        let mut print = json!({"gcode_state": state, "subtask_id": task});
        if let (Some(base), Some(more)) = (print.as_object_mut(), extra.as_object()) {
            for (key, value) in more { base.insert(key.clone(), value.clone()); }
        }
        json!({"print": print})
    }

    #[test]
    fn incremental_report_does_not_refresh_unrelated_fields() {
        let mut session = Session::new(Model::H2c);
        session.configure();
        session.link_up(1_000);
        session.ingest(&report("RUNNING", "task-1", json!({"nozzle_temper": 200, "mc_percent": 10})), 1_000).unwrap();
        let first = session.contract(1_000);
        session.ingest(&report("RUNNING", "task-1", json!({"mc_percent": 11})), 5_000).unwrap();
        let second = session.contract(5_000);
        assert_eq!(second["progressObservedAtMs"], 5_000);
        assert_eq!(second["temperatures"][0]["observedAtMs"], first["temperatures"][0]["observedAtMs"]);
        assert_eq!(second["phaseObservedAtMs"], 5_000);
    }

    #[test]
    fn remaining_without_a_unit_stays_null_and_models_do_not_share_nozzle_roles() {
        let mut h2 = Session::new(Model::H2c);
        h2.link_up(1);
        h2.ingest(&report("PREPARE", "h2", json!({"mc_remaining_time": 42, "mc_percent": 0, "nozzle_temper": 180, "bed_temper": 60})), 1).unwrap();
        let h2c = h2.contract(1);
        assert_eq!(h2c["phase"], "preparing");
        assert!(h2c["remainingSeconds"].is_null());
        assert_eq!(h2c["remainingUnitUnverified"], true);
        assert_eq!(h2c["progressPercent"], 0);
        assert_eq!(h2c["temperatures"][0]["role"], "reported_nozzle");
        assert!(h2c["temperatures"].as_array().unwrap().iter().all(|item| item["role"] != "left" && item["role"] != "right"));

        let mut h3 = Session::new(Model::H3c);
        h3.link_up(1);
        h3.ingest(&report("RUNNING", "h3", json!({"nozzle_temper": 220, "remaining_seconds": 90})), 1).unwrap();
        let h3c = h3.contract(1);
        assert_eq!(h3c["temperatures"][0]["role"], "current_tool");
        assert_eq!(h3c["remainingSeconds"], 90);
        assert!(h3c["temperatures"].as_array().unwrap().iter().all(|item| item["role"] != "left" && item["role"] != "right"));
    }

    #[test]
    fn task_identity_name_delay_reprint_and_alerts() {
        let mut session = Session::new(Model::H2c);
        session.link_up(10);
        session.ingest(&report("FINISH", "old", json!({})), 10).unwrap();
        assert!(session.contract(10)["alerts"].as_array().unwrap().is_empty());
        session.ingest(&report("RUNNING", "job-a", json!({})), 20).unwrap();
        session.ingest(&report("RUNNING", "job-a", json!({"subtask_name": "late.3mf", "plate_idx": 1})), 25).unwrap();
        let plate_one = session.contract(25)["job"]["jobSessionId"].as_str().unwrap().to_string();
        session.ingest(&report("RUNNING", "job-a", json!({"plate_idx": 2})), 26).unwrap();
        assert_ne!(session.contract(26)["job"]["jobSessionId"], plate_one);
        assert_eq!(session.contract(26)["preview"]["state"], "unavailable");
        session.ingest(&report("RUNNING", "job-a", json!({"subtask_name": "late.3mf"})), 30).unwrap();
        let named = session.contract(30);
        let session_id = named["job"]["jobSessionId"].as_str().unwrap().to_string();
        assert_eq!(named["job"]["name"], "late.3mf");
        session.ingest(&report("RUNNING", "job-b", json!({"subtask_name": "late.3mf"})), 40).unwrap();
        assert_ne!(session.contract(40)["job"]["jobSessionId"], session_id);
        assert_eq!(session.contract(40)["preview"]["state"], "unavailable");
        session.ingest(&report("FINISH", "job-b", json!({})), 50).unwrap();
        assert_eq!(session.contract(50)["alerts"][0]["kind"], "completed");
        session.ingest(&report("FINISH", "job-b", json!({})), 60).unwrap();
        assert_eq!(session.contract(60)["alerts"].as_array().unwrap().len(), 1);
        let before = session.contract(60)["alerts"].as_array().unwrap().len();
        session.link_down(70);
        session.link_up(80);
        session.ingest(&report("FINISH", "reconnect", json!({})), 80).unwrap();
        assert_eq!(session.contract(80)["alerts"].as_array().unwrap().len(), before);
        assert_eq!(session.contract(80)["job"]["sourceTaskId"], "reconnect");
    }

    #[test]
    fn ambiguous_failure_is_unknown_and_cancel_is_not_a_fault() {
        let mut session = Session::new(Model::H3c);
        session.link_up(1);
        session.ingest(&report("RUNNING", "task", json!({})), 1).unwrap();
        session.ingest(&report("FAILED", "task", json!({})), 2).unwrap();
        assert_eq!(session.contract(2)["phase"], "unknown");
        assert!(session.contract(2)["alerts"].as_array().unwrap().is_empty());
        session.ingest(&report("CANCELLED", "task", json!({})), 3).unwrap();
        assert_eq!(session.contract(3)["phase"], "cancelled");
        assert!(session.contract(3)["alerts"].as_array().unwrap().iter().all(|alert| alert["kind"] != "error"));
        session.ingest(&report("FAILED", "task", json!({"print_error": 2008})), 4).unwrap();
        assert_eq!(session.contract(4)["phase"], "failed");
    }

    #[test]
    fn archive_and_image_bounds_reject_traversal_markup_and_demo_matches() {
        let mut entries = 0;
        let mut bytes = 0;
        assert_eq!(admit_plate_entry("Metadata/plate_2.png", 100, &mut entries, &mut bytes), Ok(2));
        assert_eq!(admit_plate_entry("../Metadata/plate_2.png", 1, &mut entries, &mut bytes), Err("path_rejected"));
        assert_eq!(admit_plate_entry("Metadata/newest.3mf", 1, &mut entries, &mut bytes), Err("not_plate_png"));
        assert_eq!(inspect_image(b"<?xml version=\"1.0\"?><Error>403</Error>"), Err("image_rejected"));
        assert_eq!(inspect_image(&vec![0u8; MAX_IMAGE_BYTES + 1]), Err("image_rejected"));
        assert_eq!(decide_preview(true, true, true, false), "unavailable");
        assert_eq!(decide_preview(true, true, false, true), "unavailable");
        assert_eq!(decide_preview(false, true, false, false), "unavailable");
        assert_eq!(decide_preview(true, true, false, false), "available");
    }

    #[test]
    fn configuration_stores_a_secret_without_returning_it_and_clears_it() {
        let dir = std::env::temp_dir().join(format!("bambu-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let status = configure_in(&dir, "H2C", "192.168.1.20", "ABCDEFGH1234", "A1B2C3D4").unwrap();
        assert_eq!(status["accessCodeStored"], true);
        assert_eq!(status["socket"], "not_started");
        assert_eq!(status["cloud"], "unsupported");
        assert_eq!(status["thumbnail"], "unsupported");
        let rendered = status.to_string();
        assert!(!rendered.contains("A1B2C3D4"));
        assert!(rendered.contains("192.168.1.20"));
        let sealed = std::fs::read(secret_path(&dir)).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("A1B2C3D4"));
        assert!(configure_in(&dir, "H2C", "10.0.0.1:6000", "ABCDEFGH1234", "A1B2C3D4").is_err());
        assert!(configure_in(&dir, "cloud", "192.168.1.20", "ABCDEFGH1234", "A1B2C3D4").is_err());
        let forgotten = forget_in(&dir).unwrap();
        assert_eq!(forgotten["configured"], false);
        assert!(!secret_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn device_projection_uses_the_list_channel_and_marks_missing_values() {
        let mut session = Session::new(Model::H3c);
        session.configure();
        session.link_up(1);
        session.ingest(&report("RUNNING", "task", json!({"mc_percent": 68})), 1).unwrap();
        let snapshot = session.device_snapshot();
        assert_eq!(snapshot.source, SOURCE);
        assert_eq!(snapshot.ttl_ms, TTL_MS);
        assert!(snapshot.rows.iter().any(|row| row.id == "progress" && row.value == "68%"));
        assert!(snapshot.rows.iter().any(|row| row.id == "remaining" && row.value == "—"));
        assert!(snapshot.rows.iter().any(|row| row.id == "preview" && row.value == "无预览"));
        let idle = Session::new(Model::H2c).device_snapshot();
        assert_eq!(idle.status, "empty");
    }
}
