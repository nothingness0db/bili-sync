use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Local;
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;

pub const MAX_HISTORY_LOGS: usize = 800;

/// 日志文件路径（容器内 data 卷可写；本地开发时可设置环境变量覆盖）
const LOG_FILE_PATH: &str = "/app/data/bili-sync.log";

/// 单文件大小上限（10MB）：按天切分之外的兜底，防止单日日志超大
const LOG_MAX_SIZE: u64 = 10 * 1024 * 1024;

/// 保留的归档文件份数（当前文件之外的最近 N 份，按名字排序后删除最旧的）
const LOG_KEEP_ARCHIVES: usize = 7;

/// LogHelper 维护了日志发送器、日志历史缓冲区（启动时从日志文件恢复）和日志文件落盘
pub struct LogHelper {
    pub sender: broadcast::Sender<String>,
    pub log_history: Arc<RwLock<VecDeque<String>>>,
    log_file: Arc<Mutex<LogFileHandle>>,
}

/// 日志文件句柄：当前打开的文件 + 打开时的日期（YYYYMMDD），用于按天轮转
struct LogFileHandle {
    file: Option<File>,
    opened_date: String,
}

impl LogHelper {
    pub fn new(sender: broadcast::Sender<String>, log_history: Arc<RwLock<VecDeque<String>>>) -> Self {
        let log_file = Arc::new(Mutex::new(LogFileHandle {
            file: open_log_file(),
            opened_date: Local::now().format("%Y%m%d").to_string(),
        }));
        // 启动时将日志文件（当前文件 + 归档）内容加载进历史，重启后日志不丢失
        load_history_from_files(&mut log_history.write(), MAX_HISTORY_LOGS);
        LogHelper {
            sender,
            log_history,
            log_file,
        }
    }
}

fn log_file_path() -> PathBuf {
    std::env::var("BILI_SYNC_LOG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(LOG_FILE_PATH))
}

fn log_file_name() -> String {
    log_file_path()
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "bili-sync.log".to_string())
}

fn open_log_file() -> Option<File> {
    let path = log_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    OpenOptions::new().read(true).create(true).append(true).open(path).ok()
}

/// 启动时按文件名升序（旧→新）收集日志目录下所有本日志文件（当前 + 归档），恢复历史，保留最近 max 行
fn load_history_from_files(history: &mut VecDeque<String>, max: usize) {
    let path = log_file_path();
    let Some(parent) = path.parent() else {
        return;
    };
    let prefix = log_file_name();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let mut names = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.as_str() == prefix || name.starts_with(&format!("{prefix}.")))
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Ok(mut file) = File::open(parent.join(name)) else {
            continue;
        };
        let mut content = String::new();
        if file.read_to_string(&mut content).is_err() {
            continue;
        }
        for line in content.lines() {
            if !line.trim().is_empty() {
                history.push_back(line.to_string());
            }
        }
    }
    while history.len() > max {
        history.pop_front();
    }
}

/// 每次写入前检查：日期变化或文件超过大小上限则轮转（关闭旧文件 → 归档 → 清理 → 开新文件）
fn rotate_if_needed(handle: &mut LogFileHandle) {
    let path = log_file_path();
    let today = Local::now().format("%Y%m%d").to_string();
    let over_size = handle
        .file
        .as_ref()
        .map(|f| f.metadata().map(|m| m.len() >= LOG_MAX_SIZE).unwrap_or(false))
        .unwrap_or(false);
    if handle.opened_date == today && !over_size {
        return;
    }
    // 关闭当前文件
    if let Some(file) = handle.file.take() {
        drop(file);
    }
    // 归档：bili-sync.log -> bili-sync.log.YYYYMMDD，同日重复触发（大小兜底）则追加 -1、-2…
    let name = log_file_name();
    let archive_base = format!("{name}.{today}");
    let mut archive_name = archive_base.clone();
    let mut seq = 1usize;
    while path.with_file_name(&archive_name).exists() {
        archive_name = format!("{archive_base}-{seq}");
        seq += 1;
    }
    let _ = std::fs::rename(&path, path.with_file_name(&archive_name));
    // 清理归档：按文件名升序删除最旧的，保留最近 LOG_KEEP_ARCHIVES 份
    let prefix = log_file_name();
    if let Some(parent) = path.parent() {
        let Ok(entries) = std::fs::read_dir(parent) else {
            handle.file = open_log_file();
            handle.opened_date = today;
            return;
        };
        let mut archives = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(&format!("{prefix}.")))
            .collect::<Vec<_>>();
        archives.sort();
        if archives.len() > LOG_KEEP_ARCHIVES {
            let remove_count = archives.len() - LOG_KEEP_ARCHIVES;
            for name in archives.into_iter().take(remove_count) {
                let _ = std::fs::remove_file(parent.join(name));
            }
        }
    }
    // 重新打开新文件，更新记录日期
    handle.file = open_log_file();
    handle.opened_date = today;
}

impl<'a> MakeWriter<'a> for LogHelper {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl std::io::Write for LogHelper {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let log_message = String::from_utf8_lossy(buf).to_string();
        let _ = self.sender.send(log_message.clone());
        let mut history = self.log_history.write();
        history.push_back(log_message.clone());
        if history.len() > MAX_HISTORY_LOGS {
            history.pop_front();
        }
        drop(history);
        // 同步落盘（日志量小，可接受），写入前在锁内检查是否需要轮转
        let mut handle = self.log_file.lock().unwrap();
        rotate_if_needed(&mut handle);
        if let Some(file) = handle.file.as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut handle = self.log_file.lock().unwrap();
        if let Some(file) = handle.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

impl Clone for LogHelper {
    fn clone(&self) -> Self {
        LogHelper {
            sender: self.sender.clone(),
            log_history: self.log_history.clone(),
            log_file: self.log_file.clone(),
        }
    }
}
