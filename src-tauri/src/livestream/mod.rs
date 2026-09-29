//! 直播推流状态的 DTO 定义（serde + ts_rs 导出到 src/generated/bindings.ts）。
//! OBS 的实际控制逻辑在 crate::obs，这里只承载跨进程传输的数据结构。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LivestreamState {
    Draft,
    Ready,
    Playing,
    Paused,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LivestreamSegmentStatus {
    Draft,
    Ready,
    Playing,
    Played,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LivestreamOutputState {
    Idle,
    Synthesizing,
    Playing,
    Played,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivestreamVoiceSnapshot {
    pub provider_id: String,
    pub base_url: String,
    pub model_id: String,
    pub voice_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LivestreamSegment {
    pub id: String,
    pub title: String,
    pub text: String,
    pub estimated_seconds: u32,
    pub sources: Vec<String>,
    pub status: LivestreamSegmentStatus,
}

impl LivestreamSegment {
    pub fn draft(
        title: String,
        text: String,
        estimated_seconds: u32,
        sources: Vec<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            text,
            estimated_seconds: estimated_seconds.max(1),
            sources,
            status: LivestreamSegmentStatus::Draft,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LivestreamScript {
    pub id: String,
    pub title: String,
    pub segments: Vec<LivestreamSegment>,
    pub loop_enabled: bool,
    pub confirmed: bool,
    pub current_index: Option<usize>,
    pub state: LivestreamState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LivestreamError {
    #[error("Livestream script is invalid")]
    Invalid,
    #[error("Livestream script requires confirmation")]
    ConfirmationRequired,
    #[error("Livestream script has finished")]
    Finished,
    #[error("Livestream state does not allow this control")]
    StateInvalid,
}

impl LivestreamScript {
    pub fn draft(
        title: String,
        segments: Vec<LivestreamSegment>,
        loop_enabled: bool,
    ) -> Result<Self, LivestreamError> {
        if title.trim().is_empty()
            || segments.is_empty()
            || segments.len() > 100
            || segments.iter().any(|segment| {
                segment.title.trim().is_empty()
                    || segment.text.trim().is_empty()
                    || segment.text.len() > 16 * 1024
            })
        {
            return Err(LivestreamError::Invalid);
        }
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            segments,
            loop_enabled,
            confirmed: false,
            current_index: None,
            state: LivestreamState::Draft,
        })
    }

    pub fn confirm(&mut self) -> Result<(), LivestreamError> {
        if self.state != LivestreamState::Draft {
            return Err(LivestreamError::StateInvalid);
        }
        self.confirmed = true;
        self.state = LivestreamState::Ready;
        for segment in &mut self.segments {
            segment.status = LivestreamSegmentStatus::Ready;
        }
        Ok(())
    }

    pub fn start(&mut self) -> Result<&LivestreamSegment, LivestreamError> {
        if !self.confirmed {
            return Err(LivestreamError::ConfirmationRequired);
        }
        if !matches!(
            self.state,
            LivestreamState::Ready | LivestreamState::Finished
        ) {
            return Err(LivestreamError::StateInvalid);
        }
        self.select(0)
    }

    pub fn pause(&mut self) -> Result<(), LivestreamError> {
        if self.state != LivestreamState::Playing {
            return Err(LivestreamError::StateInvalid);
        }
        self.state = LivestreamState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<&LivestreamSegment, LivestreamError> {
        if self.state != LivestreamState::Paused {
            return Err(LivestreamError::StateInvalid);
        }
        let index = self.current_index.ok_or(LivestreamError::StateInvalid)?;
        self.select(index)
    }

    pub fn complete_current(&mut self) -> Result<(), LivestreamError> {
        let index = self.current_index.ok_or(LivestreamError::StateInvalid)?;
        if !matches!(
            self.state,
            LivestreamState::Playing | LivestreamState::Paused
        ) {
            return Err(LivestreamError::StateInvalid);
        }
        self.segments[index].status = LivestreamSegmentStatus::Played;
        Ok(())
    }

    #[allow(clippy::should_implement_trait)] // This is a user control action, not iterator consumption.
    pub fn next(&mut self) -> Result<&LivestreamSegment, LivestreamError> {
        let current = self.current_index.ok_or(LivestreamError::StateInvalid)?;
        if current + 1 < self.segments.len() {
            return self.select(current + 1);
        }
        if self.loop_enabled {
            return self.select(0);
        }
        self.state = LivestreamState::Finished;
        Err(LivestreamError::Finished)
    }

    pub fn previous(&mut self) -> Result<&LivestreamSegment, LivestreamError> {
        let current = self.current_index.ok_or(LivestreamError::StateInvalid)?;
        self.select(current.saturating_sub(1))
    }

    pub fn replay(&mut self) -> Result<&LivestreamSegment, LivestreamError> {
        let current = self.current_index.ok_or(LivestreamError::StateInvalid)?;
        self.select(current)
    }

    fn select(&mut self, index: usize) -> Result<&LivestreamSegment, LivestreamError> {
        let segment = self
            .segments
            .get_mut(index)
            .ok_or(LivestreamError::Invalid)?;
        segment.status = LivestreamSegmentStatus::Playing;
        self.current_index = Some(index);
        self.state = LivestreamState::Playing;
        Ok(segment)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LivestreamMediaKind {
    Image,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LivestreamStageState {
    pub product_title: String,
    pub current_subtitle: String,
    pub next_hint: String,
    pub state: LivestreamState,
    pub media_path: Option<String>,
    pub media_kind: Option<LivestreamMediaKind>,
    pub output_state: LivestreamOutputState,
    pub output_error_code: Option<String>,
}

pub fn render_stage_html(stage: &LivestreamStageState) -> String {
    let title = escape_html(&stage.product_title);
    let subtitle = escape_html(&stage.current_subtitle);
    let next = escape_html(&stage.next_hint);
    let status = match stage.output_state {
        LivestreamOutputState::Synthesizing => "思考中",
        LivestreamOutputState::Playing => "播报中",
        LivestreamOutputState::Played => "已播报",
        LivestreamOutputState::Cancelled => "已接管",
        LivestreamOutputState::Failed => "语音未输出",
        LivestreamOutputState::Idle => match stage.state {
            LivestreamState::Draft => "准备中",
            LivestreamState::Ready => "已就绪",
            LivestreamState::Playing => "等待播报",
            LivestreamState::Paused => "已暂停",
            LivestreamState::Finished => "已结束",
        },
    };
    let media = match (&stage.media_path, stage.media_kind) {
        (Some(path), Some(LivestreamMediaKind::Image)) => {
            format!(
                "<img class=\"media\" src=\"{}\" alt=\"\">",
                escape_html(path)
            )
        }
        (Some(path), Some(LivestreamMediaKind::Video)) => format!(
            "<video class=\"media\" src=\"{}\" autoplay muted loop playsinline></video>",
            escape_html(path)
        ),
        _ => "<div class=\"media placeholder\">AI</div>".into(),
    };
    format!(
        "<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>html,body{{width:100%;height:100%;margin:0;background:#0b1020;color:#fff;font-family:'Microsoft YaHei',sans-serif}}main{{height:100%;display:grid;grid-template-rows:1fr auto;padding:64px;box-sizing:border-box}}.media{{width:100%;height:100%;object-fit:contain;border-radius:28px}}.placeholder{{display:grid;place-items:center;font-size:180px;background:linear-gradient(135deg,#1d4ed8,#7c3aed)}}section{{padding:30px 38px;background:rgba(5,10,24,.86);border-radius:24px}}h1{{font-size:38px;margin:0 0 16px}}p{{font-size:48px;margin:0;line-height:1.35}}footer{{display:flex;justify-content:space-between;margin-top:18px;color:#b9c4df;font-size:24px}}</style><main>{media}<section><h1>{title}</h1><p>{subtitle}</p><footer><span>{next}</span><span>{status}</span></footer></section></main></html>"
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedScript {
    segments: Vec<GeneratedSegment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedSegment {
    title: String,
    text: String,
    estimated_seconds: u32,
    #[serde(default)]
    sources: Vec<String>,
}

pub fn parse_generated_segments(
    json_text: &str,
    max_segments: usize,
) -> Result<Vec<LivestreamSegment>, LivestreamError> {
    if !(1..=12).contains(&max_segments) || json_text.len() > 128 * 1024 {
        return Err(LivestreamError::Invalid);
    }
    let parsed: GeneratedScript =
        serde_json::from_str(json_text).map_err(|_| LivestreamError::Invalid)?;
    if parsed.segments.is_empty() || parsed.segments.len() > max_segments {
        return Err(LivestreamError::Invalid);
    }
    parsed
        .segments
        .into_iter()
        .map(|segment| {
            if segment.title.trim().is_empty()
                || segment.text.trim().is_empty()
                || segment.title.len() > 200
                || segment.text.len() > 16 * 1024
                || segment.sources.len() > 12
            {
                return Err(LivestreamError::Invalid);
            }
            Ok(LivestreamSegment::draft(
                segment.title.trim().into(),
                segment.text.trim().into(),
                segment.estimated_seconds.clamp(1, 900),
                segment
                    .sources
                    .into_iter()
                    .take(12)
                    .map(|source| source.chars().take(260).collect())
                    .collect(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> LivestreamScript {
        LivestreamScript::draft(
            "产品讲解".into(),
            vec![
                LivestreamSegment::draft(
                    "开场".into(),
                    "欢迎了解产品".into(),
                    8,
                    vec!["产品资料.pdf".into()],
                ),
                LivestreamSegment::draft(
                    "功能".into(),
                    "核心功能说明".into(),
                    12,
                    vec!["产品资料.pdf".into()],
                ),
            ],
            false,
        )
        .unwrap()
    }

    #[test]
    fn script_requires_confirmation_and_stops_after_the_last_segment() {
        let mut script = draft();
        assert_eq!(
            script.start().unwrap_err(),
            LivestreamError::ConfirmationRequired
        );
        script.confirm().unwrap();
        assert_eq!(script.start().unwrap().title, "开场");
        script.complete_current().unwrap();
        assert_eq!(script.next().unwrap().title, "功能");
        script.complete_current().unwrap();
        assert_eq!(script.next().unwrap_err(), LivestreamError::Finished);
        assert_eq!(script.state, LivestreamState::Finished);
    }

    #[test]
    fn looping_is_bounded_to_confirmed_segments() {
        let mut script = draft();
        script.loop_enabled = true;
        script.confirm().unwrap();
        script.start().unwrap();
        script.complete_current().unwrap();
        script.next().unwrap();
        script.complete_current().unwrap();
        assert_eq!(script.next().unwrap().title, "开场");
        assert_eq!(script.segments.len(), 2);
    }

    #[test]
    fn controls_pause_resume_previous_and_replay_without_generating_content() {
        let mut script = draft();
        script.confirm().unwrap();
        script.start().unwrap();
        script.pause().unwrap();
        assert_eq!(script.state, LivestreamState::Paused);
        script.resume().unwrap();
        script.complete_current().unwrap();
        script.next().unwrap();
        assert_eq!(script.previous().unwrap().title, "开场");
        assert_eq!(script.replay().unwrap().title, "开场");
        assert_eq!(script.segments.len(), 2);
    }

    #[test]
    fn stage_html_escapes_content_and_exposes_no_controls_or_internal_errors() {
        let stage = LivestreamStageState {
            product_title: "<新品>".into(),
            current_subtitle: "安全 & 稳定".into(),
            next_hint: "下一段：功能".into(),
            state: LivestreamState::Playing,
            media_path: None,
            media_kind: None,
            output_state: LivestreamOutputState::Playing,
            output_error_code: None,
        };
        let html = render_stage_html(&stage);
        assert!(html.contains("&lt;新品&gt;"));
        assert!(html.contains("安全 &amp; 稳定"));
        assert!(!html.contains("<新品>"));
        assert!(!html.contains("button"));
        assert!(!html.contains("apiKey"));
        assert!(!html.contains("errorCode"));
    }

    #[test]
    fn stage_exposes_truthful_output_states_without_internal_error_details() {
        let mut stage = LivestreamStageState {
            product_title: "产品".into(),
            current_subtitle: "正在合成".into(),
            next_hint: String::new(),
            state: LivestreamState::Playing,
            media_path: None,
            media_kind: None,
            output_state: LivestreamOutputState::Synthesizing,
            output_error_code: Some("SECRET_INTERNAL_DETAIL".into()),
        };
        assert!(render_stage_html(&stage).contains("思考中"));
        assert!(!render_stage_html(&stage).contains("SECRET_INTERNAL_DETAIL"));
        stage.output_state = LivestreamOutputState::Failed;
        assert!(render_stage_html(&stage).contains("语音未输出"));
    }

    #[test]
    fn parses_bounded_structured_model_script() {
        let value = r#"{"segments":[{"title":"开场","text":"欢迎","estimatedSeconds":8,"sources":["产品.pdf"]}]}"#;
        let segments = parse_generated_segments(value, 4).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].title, "开场");
        assert_eq!(segments[0].estimated_seconds, 8);
        assert!(parse_generated_segments(value, 0).is_err());
    }
}
