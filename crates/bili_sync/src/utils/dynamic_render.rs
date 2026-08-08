use crate::bilibili::{DynamicInfo, ReplyInfo};

/// 渲染动态正文 markdown
pub fn render_dynamic_md(info: &DynamicInfo, upper_name: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} 的动态\n\n", upper_name));
    out.push_str(&format!(
        "- 动态 id: {}\n- 类型: {}\n- 发布时间: {}\n",
        info.id,
        info.dyn_type,
        info.pub_ts.format("%Y-%m-%d %H:%M:%S")
    ));
    if !info.location.is_empty() {
        out.push_str(&format!("- 发布位置: {}\n", info.location));
    }
    if let Some(comment) = info.stat["comment"]["count"].as_i64() {
        out.push_str(&format!("- 评论数: {comment}\n"));
    }
    if let Some(like) = info.stat["like"]["count"].as_i64() {
        out.push_str(&format!("- 点赞数: {like}\n"));
    }
    if let Some(forward) = info.stat["forward"]["count"].as_i64() {
        out.push_str(&format!("- 转发数: {forward}\n"));
    }
    out.push_str(&format!("\n- 原链接: https://www.bilibili.com/opus/{}\n", info.id));
    if !info.content.is_empty() {
        out.push_str("\n## 正文\n\n");
        out.push_str(&info.content);
        out.push('\n');
    }
    if !info.pics.is_empty() {
        out.push_str("\n## 图片\n\n");
        for (i, _) in info.pics.iter().enumerate() {
            out.push_str(&format!("![图片 {i}](pics/{:0>2}.jpg)\n\n", i + 1));
        }
    }
    out
}

/// 渲染评论 markdown
pub fn render_comments_md(replies: &[ReplyInfo]) -> String {
    let mut out = String::new();
    out.push_str("# 评论\n\n");
    for (i, reply) in replies.iter().enumerate() {
        out.push_str(&render_comment(reply, 0, i + 1));
    }
    out
}

fn render_comment(reply: &ReplyInfo, depth: usize, index: usize) -> String {
    let indent = "  ".repeat(depth);
    let mut out = String::new();
    let time = reply.ctime.format("%Y-%m-%d %H:%M:%S");
    if depth == 0 {
        out.push_str(&format!(
            "{indent}### 评论 {index}  @{}（{}）\n\n",
            reply.uname, time
        ));
    } else {
        out.push_str(&format!(
            "{indent}- 回复 @{}（{}）\n",
            reply.uname, time
        ));
    }
    if !reply.content.is_empty() {
        out.push_str(&format!("{indent}{}\n\n", reply.content.replace('\n', &format!("\n{indent}"))));
    }
    if !reply.images.is_empty() {
        for (i, _) in reply.images.iter().enumerate() {
            out.push_str(&format!("{indent}![图片](comments/{}_{}.jpg)\n\n", reply.rpid, i + 1));
        }
    }
    for (i, sub) in reply.sub_replies.iter().enumerate() {
        out.push_str(&render_comment(sub, depth + 1, i + 1));
    }
    out
}
