use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum BiliError {
    #[error("response missing 'code' field, full response: {0}")]
    InvalidResponse(String),
    #[error("API returned error code {code}, full response: {response}")]
    ErrorResponse {
        code: i64,
        message: Option<String>,
        response: String,
    },
    #[error("risk control triggered by server, full response: {0}")]
    RiskControlOccurred(String),
    #[error("invalid HTTP response code {0}, reason: {1}")]
    InvalidStatusCode(u16, &'static str),
    #[error("no video streams available (may indicate risk control)")]
    VideoStreamsEmpty,
}

impl BiliError {
    pub fn is_risk_control_related(&self) -> bool {
        matches!(
            self,
            BiliError::RiskControlOccurred(_) | BiliError::VideoStreamsEmpty | BiliError::InvalidStatusCode(_, _)
        )
    }

    pub fn is_common_error(&self) -> bool {
        if let BiliError::ErrorResponse { code, message, .. } = self {
            for pair in [(-503, "服务暂不可用"), (-504, "服务调用超时")] {
                if *code == pair.0 && message.as_ref().is_some_and(|m| m == pair.1) {
                    return true;
                }
            }
        }
        false
    }

    /// 稿件已删除/不存在：-404 为旧版返回码，62012 为新版返回码
    /// 注意 62002「稿件不可见」（审核中/锁定/退回）不算删除，调用方应继续跳过
    pub fn is_video_not_found(&self) -> bool {
        matches!(self, BiliError::ErrorResponse { code, .. } if *code == -404 || *code == 62012)
    }
}
