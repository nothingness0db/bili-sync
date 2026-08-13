use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use parking_lot::RwLock;
use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;

pub const MAX_HISTORY_LOGS: usize = 800;

/// 日志文件路径（容器内 data 卷可写；本地开发时可设置环境变量覆盖）
const LOG_FILE_PATH: &str = "/app/data/bili-sync.log";

/// LogHelper 维护了日志发送器、日志历史缓冲区（启动时从日志文件恢复）和日志文件落盘
pub struct LogHelper {
    pub sender: broadcast::Sender<String>,
    pub log_history: Arc<RwLock<VecDeque<String>>>,
    log_file: Arc<Mutex<Option<File>>>,
}

impl LogHelper {
    pub fn new(sender: broadcast::Sender<String>, log_history: Arc<RwLock<VecDeque<String>>>) -> Self {
        let log_file = Arc::new(Mutex::new(open_log_file()));
        // 启动时将日志文件尾部内容加载进历史，重启后日志不丢失
        if let Some(file) = log_file.lock().unwrap().as_mut() {
            load_history_from_file(file, &mut log_history.write(), MAX_HISTORY_LOGS);
        }
        LogHelper {
            sender,
            log_history,
            log_file,
        }
    }
}

fn open_log_file() -> Option<File> {
    let path = std::env::var("BILI_SYNC_LOG_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(LOG_FILE_PATH));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn load_history_from_file(file: &mut File, history: &mut VecDeque<String>, max: usize) {
    let _ = file.seek(SeekFrom::Start(0));
    let mut content = String::new();
    if file.read_to_string(&mut content).is_err() {
        return;
    }
    let lines = content.lines().map(|s| s.to_string()).collect::<Vec<_>>();
    let skip = lines.len().saturating_sub(max);
    for line in lines.into_iter().skip(skip) {
        if !line.trim().is_empty() {
            history.push_back(line);
        }
    }
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
        // 同步落盘（日志量小，可接受）
        if let Some(file) = self.log_file.lock().unwrap().as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = self.log_file.lock().unwrap().as_mut() {
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
