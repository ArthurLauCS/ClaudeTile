//! Display codexreset.vercel.app's public predictions; no local inference or account access.
use crate::{config::tr, json::Json, quota, time};

pub const TRACKERS: [&str; 2] = ["codex", "claude"];
pub const EMBEDS: [&str; 2] = [
    "https://codexreset.vercel.app/zh/embed/codex",
    "https://codexreset.vercel.app/zh/embed/claude",
];
pub type Feeds = [Result<Prediction, String>; 2];

#[derive(Debug)]
pub struct Prediction {
    pub status: String,
    pub polled: i64,
}

impl Prediction {
    fn parse(text: &str, tracker: &str) -> Option<Self> {
        let j = Json::parse(text)?;
        if j.get("tracker").str()? != tracker { return None; }
        let status = j.get("status");
        let state = status.get("status").str()?;
        if !matches!(state, "yes" | "no" | "soon") { return None; }
        Some(Self {
            status: state.into(),
            polled: time::parse_iso(j.get("lastPolledAt").str()?)?,
        })
    }

    pub fn label(&self) -> &'static str {
        match self.status.as_str() {
            "yes" => tr("已重置", "Reset"),
            "soon" => tr("可能即将重置", "Reset may be coming"),
            _ => tr("未重置", "Not reset"),
        }
    }
}

pub fn fetch(proxy: Option<&str>) -> Feeds {
    TRACKERS.map(|tracker| {
        let path = format!("/api/status?tracker={tracker}");
        match quota::http_get("codexreset.vercel.app", &path, "Accept: application/json\r\n", proxy) {
            Ok((200, body)) => Prediction::parse(&body, tracker).ok_or_else(|| tr("预测接口格式有变化", "Prediction format changed").into()),
            Ok((code, _)) => Err(format!("HTTP {code}")),
            Err(_) => Err(tr("预测暂不可用，稍后重试", "Prediction unavailable; retrying later").into()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const FEED: &str = r#"{"tracker":"claude","status":{"status":"no","lastResetAt":null},"prediction":{"probability":0.46,"nextExpectedAt":"2026-10-19T17:24:52Z"},"lastPolledAt":"2026-10-20T07:00:19.97Z"}"#;

    #[test]
    fn only_uses_upstream_status_not_prediction_date() {
        let p = Prediction::parse(FEED, "claude").unwrap();
        assert_eq!(p.status, "no");
        assert_eq!(p.polled, time::parse_iso("2026-10-20T07:00:19Z").unwrap());
        assert_eq!(Prediction::parse(&FEED.replace("\"no\"", "\"soon\""), "claude").unwrap().status, "soon");
    }

    #[test]
    fn rejects_mismatched_or_malformed_feed() {
        for s in [FEED.replace("\"no\"", "\"unknown\""), FEED.replace("2026-10-20T07:00:19.97Z", "bad-date"), "{}".into()] {
            assert!(Prediction::parse(&s, "claude").is_none());
        }
        assert!(Prediction::parse(FEED, "codex").is_none());
    }

    #[test]
    #[ignore = "Live public service; set CT_TEST_PROXY if needed"]
    fn live_public_feeds() {
        for result in fetch(std::env::var("CT_TEST_PROXY").ok().as_deref()) {
            assert!(result.is_ok(), "{result:?}");
        }
    }
}
