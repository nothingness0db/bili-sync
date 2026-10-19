use std::sync::LazyLock;

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

// Tokio 的写锁优先于后来的读锁；新视频任务最多等当前一页评论请求结束。
static VIDEO_PRIORITY: LazyLock<RwLock<()>> = LazyLock::new(|| RwLock::new(()));

pub async fn video_permit() -> RwLockWriteGuard<'static, ()> {
    VIDEO_PRIORITY.write().await
}

pub async fn background_permit() -> RwLockReadGuard<'static, ()> {
    VIDEO_PRIORITY.read().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn video_waits_for_current_page_and_runs_before_later_background_requests() {
        let page = background_permit().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let video = tokio::spawn({
            let started = started.clone();
            let release = release.clone();
            async move {
                let _video = video_permit().await;
                started.notify_one();
                release.notified().await;
            }
        });
        tokio::task::yield_now().await;
        let mut later_comment = tokio::spawn(async {
            let _page = background_permit().await;
        });
        drop(page);
        started.notified().await;
        // 使用真实的准入函数：视频运行期间，新评论页不能越过它。
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut later_comment)
                .await
                .is_err()
        );
        release.notify_one();
        video.await.unwrap();
        later_comment.await.unwrap();
    }
}
