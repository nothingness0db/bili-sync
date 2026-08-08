# AGENTS.md

## 项目概述

bili-sync 的 fork（原项目 https://github.com/amtoaer/bili-sync），新增动态同步功能：
- 全类型动态同步（图文/纯文字/转发/视频动态），落盘 dynamic.json + content.md + 图片
- 评论同步：动态发布后 5 天窗口自动同步，手动单条/全部重扫（慢任务分批）
- 账号数据可视化：粉丝/关注/投稿/播放时间序列（upper_stat 表），名字/签名/头像版本历史
- 新增表：`dynamic_source` / `dynamic` / `reply` / `upper_stat`

## B 站 API 风控（重要！务必遵守）

### 事实
- **-403「访问权限不足」是冷却型风控（IP 级）**：触发后无论换什么凭据（新 SESSDATA/新 buvid3）都会被拒，几十分钟到 1 小时自动解除
- 触发原因：短时间大量请求，评论接口（`x/v2/reply/wbi/main`、`x/v2/reply/reply`）和动态列表（`feed/space`）最敏感
- 不要依赖「换 buvid3 / 重新扫码登录」作为自愈手段——稳定运行的唯一正确姿势是**保持低频 + 触发后立即停手等待**

### 已内置的节流（不要移除）
- 动态列表接口：每页请求间隔 600ms（`bilibili/dynamic.rs` DYNAMIC_FEED_INTERVAL）
- 评论接口：每次请求间隔 400ms（`bilibili/reply.rs` REPLY_REQUEST_INTERVAL）
- 每轮任务处理上限：重扫动态 10 条 + 新动态 20 条（`workflow_dynamic.rs` process_unhandled_dynamics）
- 全局限流器：每 250ms 最多 4 个请求（`config/item.rs` rate_limit，原项目机制）

### 风控触发后的行为（已实现）
- `update_upper_stat` 遇风控错误立即 bail，终止本轮任务，不再继续请求
- 各处理管线检测到风控（BiliError::is_risk_control_related）时终止本轮，等待下一轮（默认 20 分钟间隔）

### 新增请求必须遵守
- 新接口加入时必须有节流（间隔不低于 400ms）
- 批量循环请求必须限制每轮数量（参照重扫 10 条/轮的约定）
- 不要在循环中并发发送大量请求

## 架构备忘

- 动态同步管线：`workflow_dynamic.rs::process_dynamic_source`（刷新 → 账号快照 → 处理动态）
- 账号信息 API：`bilibili/upper.rs`（card / upstat / arc.search 三个接口）
- 评论 API：`bilibili/reply.rs`（wbi 签名，main + reply 两级分页）
- 动态 API：`bilibili/dynamic.rs`（DynamicFeed 全类型流）
- Web API：`api/routes/dynamic_sources/` + `api/routes/dynamic_stats.rs`
- 前端：`web/src/routes/dynamic-sources/`（列表页 + [id] 详情页）

## 构建

```bash
# 后端
cargo build
# 前端（构建产物嵌入二进制，改前端必须重新 cargo build）
cd web && npm install --legacy-peer-deps && npm run build && cd ..
cargo build
# 前端检查
cd web && npm run check
```
