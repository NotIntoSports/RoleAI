//! 训练客观指标（纯 Rust 计算，不发 LLM、不落库、不碰网络）。
//!
//! 输入是训练会话的逐条回答（文本 + 可选毫秒时间戳），输出结构化指标：
//! 回答时长、语速（中文字/分钟、英文词/分钟）、口头禅统计（词表可配置）、
//! 长停顿次数（仅当时间信息齐全时计算，否则标记不可用）、
//! 结构信号与 STAR 覆盖提示（启发式，报告里必须写明"仅供参考"）。

/// 单条回答的输入：用户回答文本 + 可选的起止毫秒时间戳。
/// 时间戳来自会话轮次的 `created_at`（RFC3339 由命令层换算成毫秒）；
/// 缺失时依赖时间的指标按"不可用"处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerInput {
    pub user_text: String,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

impl AnswerInput {
    pub fn new(user_text: impl Into<String>) -> Self {
        Self {
            user_text: user_text.into(),
            started_at_ms: None,
            ended_at_ms: None,
        }
    }

    pub fn with_timing(mut self, started_at_ms: i64, ended_at_ms: i64) -> Self {
        self.started_at_ms = Some(started_at_ms);
        self.ended_at_ms = Some(ended_at_ms);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FillerHit {
    pub word: String,
    pub count: usize,
}

/// 语速：中文字/分钟与英文词/分钟。仅当回答时长可计算且大于零时给出。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechRate {
    pub chinese_per_minute: f64,
    pub english_per_minute: f64,
}

/// 结构信号类型（启发式，只作为练习提示）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureKind {
    /// 开题（首先/第一/背景是）
    Opening,
    /// 条理展开（其次/然后/第二/第三）
    Enumeration,
    /// 收束（最后/总之/总结）
    Closing,
    /// 结果陈述（结果是/最终/提升了）
    Result,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureSignal {
    pub kind: StructureKind,
    pub marker: String,
}

/// STAR 结构覆盖提示（启发式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarCoverage {
    Situation,
    Task,
    Action,
    Result,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnswerMetrics {
    pub answer_index: usize,
    /// 回答时长（秒）；时间信息缺失或起止倒挂时为 None（不可用）。
    pub duration_seconds: Option<f64>,
    pub chinese_chars: usize,
    pub english_words: usize,
    pub speech_rate: Option<SpeechRate>,
    pub fillers: Vec<FillerHit>,
    pub structure_signals: Vec<StructureSignal>,
    pub star_coverage: Vec<StarCoverage>,
}

/// 训练整体指标。`long_pauses` 为 None 表示时间信息不全、不可用。
#[derive(Debug, Clone, PartialEq)]
pub struct PracticeMetrics {
    pub answers: Vec<AnswerMetrics>,
    pub total_duration_seconds: Option<f64>,
    pub average_answer_seconds: Option<f64>,
    /// 跨回答的长停顿次数（相邻回答之间的静默超过阈值）。
    pub long_pauses: Option<usize>,
    pub top_fillers: Vec<FillerHit>,
}

/// 口头禅与结构标记词表（可配置；默认词表来自训练需求）。
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsConfig {
    pub chinese_fillers: Vec<String>,
    pub english_fillers: Vec<String>,
    /// 相邻回答之间静默超过该秒数记一次长停顿。
    pub long_pause_seconds: f64,
    pub opening_markers: Vec<String>,
    pub enumeration_markers: Vec<String>,
    pub closing_markers: Vec<String>,
    pub result_markers: Vec<String>,
    pub star_situation_markers: Vec<String>,
    pub star_task_markers: Vec<String>,
    pub star_action_markers: Vec<String>,
    pub star_result_markers: Vec<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            chinese_fillers: ["嗯", "啊", "那个", "就是", "然后", "其实"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            english_fillers: ["um", "uh", "like", "you know"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            long_pause_seconds: 3.0,
            opening_markers: ["首先", "第一", "背景是"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            enumeration_markers: ["其次", "然后", "第二", "第三"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            closing_markers: ["最后", "总之", "总结"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            result_markers: ["结果是", "最终", "提升了", "完成了"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            star_situation_markers: ["当时", "背景", "之前在", "项目是"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            star_task_markers: ["负责", "任务是", "目标是"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            star_action_markers: ["我做了", "我采用", "我推动", "我用", "我组织"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            star_result_markers: ["结果是", "最终", "提升了", "减少了", "上线了"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// 统计一段文本中的 CJK 表意字符数（不含标点）。
pub fn count_chinese_chars(text: &str) -> usize {
    text.chars()
        .filter(|c| {
            matches!(
                *c,
                '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{20000}'..='\u{2A6DF}'
            )
        })
        .count()
}

/// 统计 ASCII 英文单词数（连续字母序列；"AI-powered" 记 2 个词）。
pub fn count_english_words(text: &str) -> usize {
    let mut words = 0usize;
    let mut in_word = false;
    for c in text.chars() {
        if c.is_ascii_alphabetic() {
            if !in_word {
                words += 1;
            }
            in_word = true;
        } else {
            in_word = false;
        }
    }
    words
}

fn count_substring_case_sensitive(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let matches = haystack.matches(needle).count();
    // 叠词内的重叠命中（如 "嗯嗯" 命中两次）符合口头禅直觉，直接返回次数。
    matches
}

fn count_english_filler(haystack_lower: &str, filler_lower: &str) -> usize {
    if filler_lower.is_empty() {
        return 0;
    }
    // 英文口头禅按整词/短语匹配，避免 "um" 命中 "gumption"。
    let mut count = 0usize;
    let mut start = 0usize;
    while let Some(position) = haystack_lower[start..].find(filler_lower) {
        let absolute = start + position;
        let before_ok = haystack_lower[..absolute]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphabetic());
        let after_index = absolute + filler_lower.len();
        let after_ok = haystack_lower[after_index..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphabetic());
        if before_ok && after_ok {
            count += 1;
        }
        start = after_index.max(start + 1);
    }
    count
}

fn coverage_from(text: &str, markers: &[String]) -> bool {
    markers
        .iter()
        .any(|marker| !marker.is_empty() && text.contains(marker.as_str()))
}

fn star_flags(text: &str, config: &MetricsConfig) -> Vec<StarCoverage> {
    let mut coverage = Vec::new();
    if coverage_from(text, &config.star_situation_markers) {
        coverage.push(StarCoverage::Situation);
    }
    if coverage_from(text, &config.star_task_markers) {
        coverage.push(StarCoverage::Task);
    }
    if coverage_from(text, &config.star_action_markers) {
        coverage.push(StarCoverage::Action);
    }
    if coverage_from(text, &config.star_result_markers) {
        coverage.push(StarCoverage::Result);
    }
    coverage
}

fn count_fillers(text: &str, config: &MetricsConfig) -> Vec<FillerHit> {
    let lower = text.to_lowercase();
    let mut hits = Vec::new();
    for filler in &config.chinese_fillers {
        let count = count_substring_case_sensitive(text, filler);
        if count > 0 {
            hits.push(FillerHit {
                word: filler.clone(),
                count,
            });
        }
    }
    for filler in &config.english_fillers {
        let count = count_english_filler(&lower, &filler.to_lowercase());
        if count > 0 {
            hits.push(FillerHit {
                word: filler.clone(),
                count,
            });
        }
    }
    hits
}

fn answer_duration(answer: &AnswerInput) -> Option<f64> {
    let started = answer.started_at_ms?;
    let ended = answer.ended_at_ms?;
    if ended < started {
        return None;
    }
    Some((ended - started) as f64 / 1000.0)
}

/// 计算全部客观指标。空输入返回空结构（全部 Option 为 None）。
pub fn compute_answers(inputs: &[AnswerInput], config: &MetricsConfig) -> PracticeMetrics {
    let mut answers = Vec::with_capacity(inputs.len());
    let mut durations = Vec::with_capacity(inputs.len());
    let mut filler_totals: Vec<FillerHit> = Vec::new();

    for (index, answer) in inputs.iter().enumerate() {
        let duration_seconds = answer_duration(answer);
        durations.push(duration_seconds);
        let chinese_chars = count_chinese_chars(&answer.user_text);
        let english_words = count_english_words(&answer.user_text);
        let speech_rate = duration_seconds.and_then(|seconds| {
            if seconds <= 0.0 {
                return None;
            }
            let minutes = seconds / 60.0;
            Some(SpeechRate {
                chinese_per_minute: chinese_chars as f64 / minutes,
                english_per_minute: english_words as f64 / minutes,
            })
        });
        let fillers = count_fillers(&answer.user_text, config);
        for hit in fillers.iter() {
            if let Some(existing) = filler_totals.iter_mut().find(|item| item.word == hit.word) {
                existing.count += hit.count;
            } else {
                filler_totals.push(hit.clone());
            }
        }
        let mut structure_signals = Vec::new();
        for marker in &config.opening_markers {
            if !marker.is_empty() && answer.user_text.contains(marker.as_str()) {
                structure_signals.push(StructureSignal {
                    kind: StructureKind::Opening,
                    marker: marker.clone(),
                });
            }
        }
        for marker in &config.enumeration_markers {
            if !marker.is_empty() && answer.user_text.contains(marker.as_str()) {
                structure_signals.push(StructureSignal {
                    kind: StructureKind::Enumeration,
                    marker: marker.clone(),
                });
            }
        }
        for marker in &config.closing_markers {
            if !marker.is_empty() && answer.user_text.contains(marker.as_str()) {
                structure_signals.push(StructureSignal {
                    kind: StructureKind::Closing,
                    marker: marker.clone(),
                });
            }
        }
        for marker in &config.result_markers {
            if !marker.is_empty() && answer.user_text.contains(marker.as_str()) {
                structure_signals.push(StructureSignal {
                    kind: StructureKind::Result,
                    marker: marker.clone(),
                });
            }
        }
        let star_coverage = star_flags(&answer.user_text, config);
        answers.push(AnswerMetrics {
            answer_index: index,
            duration_seconds,
            chinese_chars,
            english_words,
            speech_rate,
            fillers,
            structure_signals,
            star_coverage,
        });
    }

    // 总时长只在全部回答时长可计算时给出（否则任何"总/平均"都会失真）。
    let total_duration_seconds = if durations.iter().all(Option::is_some) && !durations.is_empty() {
        Some(durations.iter().map(|d| d.expect("checked all Some")).sum())
    } else {
        None
    };
    let average_answer_seconds = total_duration_seconds.map(|total| total / durations.len() as f64);

    // 长停顿：相邻回答之间的静默。所有回答时间信息齐全才可测；缺任何一处即整体不可用。
    let timing_complete = !inputs.is_empty()
        && inputs
            .iter()
            .all(|answer| answer.started_at_ms.is_some() && answer.ended_at_ms.is_some());
    let long_pauses = if timing_complete {
        let threshold_ms = (config.long_pause_seconds * 1000.0) as i64;
        let count = inputs
            .windows(2)
            .filter(|pair| {
                let gap = pair[1].started_at_ms.expect("checked above")
                    - pair[0].ended_at_ms.expect("checked above");
                gap > threshold_ms
            })
            .count();
        Some(count)
    } else {
        None
    };

    let mut top_fillers = filler_totals;
    top_fillers.sort_by(|a, b| b.count.cmp(&a.count).then(a.word.cmp(&b.word)));

    PracticeMetrics {
        answers,
        total_duration_seconds,
        average_answer_seconds,
        long_pauses,
        top_fillers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> MetricsConfig {
        MetricsConfig::default()
    }

    fn answer(text: &str) -> AnswerInput {
        AnswerInput::new(text)
    }

    // ---------- 回答时长 ----------

    #[test]
    fn duration_computes_from_timestamps() {
        let inputs = [answer("回答").with_timing(1_000, 61_000)];
        let metrics = compute_answers(&inputs, &config());
        let value = metrics.answers[0].duration_seconds.expect("duration");
        assert!((value - 60.0).abs() < f64::EPSILON);
        assert_eq!(metrics.total_duration_seconds, Some(60.0));
        assert_eq!(metrics.average_answer_seconds, Some(60.0));
    }

    #[test]
    fn duration_missing_timestamps_is_unavailable() {
        let inputs = [answer("没有时间戳的回答")];
        let metrics = compute_answers(&inputs, &config());
        assert_eq!(metrics.answers[0].duration_seconds, None);
        assert_eq!(metrics.total_duration_seconds, None);
        assert_eq!(metrics.average_answer_seconds, None);
    }

    #[test]
    fn duration_reversed_or_partial_timestamps_is_unavailable() {
        let reversed = [answer("倒挂").with_timing(61_000, 1_000)];
        let metrics = compute_answers(&reversed, &config());
        assert_eq!(metrics.answers[0].duration_seconds, None);

        let partial = [AnswerInput {
            user_text: "只有开始".into(),
            started_at_ms: Some(1_000),
            ended_at_ms: None,
        }];
        let metrics = compute_answers(&partial, &config());
        assert_eq!(metrics.answers[0].duration_seconds, None);
    }

    #[test]
    fn empty_input_yields_empty_metrics() {
        let metrics = compute_answers(&[], &config());
        assert!(metrics.answers.is_empty());
        assert_eq!(metrics.total_duration_seconds, None);
        assert_eq!(metrics.average_answer_seconds, None);
        assert_eq!(metrics.long_pauses, None);
        assert!(metrics.top_fillers.is_empty());
    }

    // ---------- 语速 ----------

    #[test]
    fn speech_rate_pure_chinese() {
        // 120 个中文字 / 60 秒 = 120 字/分钟。
        let text = "字".repeat(120);
        let inputs = [answer(&text).with_timing(0, 60_000)];
        let metrics = compute_answers(&inputs, &config());
        let rate = metrics.answers[0].speech_rate.expect("rate");
        assert!((rate.chinese_per_minute - 120.0).abs() < f64::EPSILON);
        assert!((rate.english_per_minute - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn speech_rate_mixed_chinese_english() {
        let text = "好的 we will ship the 功能 next week";
        let inputs = [answer(text).with_timing(0, 60_000)];
        let metrics = compute_answers(&inputs, &config());
        let answer = &metrics.answers[0];
        assert_eq!(answer.chinese_chars, 4); // 好、的、功、能（仅 CJK 表意字符）
        assert_eq!(answer.english_words, 6); // we will ship the next week
        let rate = answer.speech_rate.expect("rate");
        assert!((rate.chinese_per_minute - 4.0).abs() < f64::EPSILON);
        assert!((rate.english_per_minute - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn speech_rate_without_duration_is_unavailable() {
        let inputs = [answer("没有任何时间信息")];
        let metrics = compute_answers(&inputs, &config());
        assert_eq!(metrics.answers[0].speech_rate, None);
    }

    #[test]
    fn speech_rate_zero_duration_is_unavailable() {
        let inputs = [answer("零时长").with_timing(5_000, 5_000)];
        let metrics = compute_answers(&inputs, &config());
        assert_eq!(metrics.answers[0].duration_seconds, Some(0.0));
        assert_eq!(metrics.answers[0].speech_rate, None);
    }

    // ---------- 口头禅 ----------

    #[test]
    fn fillers_count_chinese_phrases() {
        let text = "嗯，那个项目其实就是我做的，然后那个项目上线了。嗯。";
        let inputs = [answer(text)];
        let metrics = compute_answers(&inputs, &config());
        let fillers = &metrics.answers[0].fillers;
        let find = |word: &str| {
            fillers
                .iter()
                .find(|hit| hit.word == word)
                .map(|hit| hit.count)
                .unwrap_or(0)
        };
        assert_eq!(find("嗯"), 2);
        assert_eq!(find("那个"), 2);
        assert_eq!(find("其实"), 1);
        assert_eq!(find("然后"), 1);
        // 口头禅也是中文字符，语速字数包含它们。
        assert!(metrics.answers[0].chinese_chars > 0);
    }

    #[test]
    fn fillers_count_english_whole_words_case_insensitive() {
        let text = "Um, I think, you know, the fix worked. UMAN would not match um. You Know?";
        let inputs = [answer(text)];
        let metrics = compute_answers(&inputs, &config());
        let fillers = &metrics.answers[0].fillers;
        let find = |word: &str| {
            fillers
                .iter()
                .find(|hit| hit.word == word)
                .map(|hit| hit.count)
                .unwrap_or(0)
        };
        assert_eq!(find("um"), 2); // 开头 "Um" 与句尾 "um"；"UMAN" 不算
        assert_eq!(find("you know"), 2); // 大小写不敏感
        assert_eq!(find("uh"), 0);
    }

    #[test]
    fn fillers_respect_configured_word_list_and_aggregate() {
        let mut config = config();
        config.chinese_fillers = vec!["就是说".into()];
        config.english_fillers = vec!["basically".into()];
        let inputs = [
            answer("就是说，就是说这个东西 basically works"),
            answer("basically 就是说 done"),
        ];
        let metrics = compute_answers(&inputs, &config);
        // 默认词表被替换：默认词"那个"不再统计。
        assert!(
            !metrics
                .answers
                .iter()
                .any(|a| a.fillers.iter().any(|hit| hit.word == "那个"))
        );
        let top = &metrics.top_fillers;
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].word, "就是说");
        assert_eq!(top[0].count, 3);
        assert_eq!(top[1].word, "basically");
        assert_eq!(top[1].count, 2);
    }

    #[test]
    fn fillers_absent_yield_empty_lists() {
        let inputs = [answer("干脆利落的回答，没有任何多余词")];
        let metrics = compute_answers(&inputs, &config());
        assert!(metrics.answers[0].fillers.is_empty());
        assert!(metrics.top_fillers.is_empty());
    }

    // ---------- 长停顿 ----------

    #[test]
    fn long_pauses_count_gaps_over_threshold() {
        let config = config(); // 阈值 3 秒
        let inputs = [
            answer("第一答").with_timing(0, 1_000),
            // 间隔 4 秒 → 记一次
            answer("第二答").with_timing(5_000, 6_000),
            // 间隔 1 秒 → 不记
            answer("第三答").with_timing(7_000, 8_000),
            // 间隔 10 秒 → 记一次
            answer("第四答").with_timing(18_000, 19_000),
        ];
        let metrics = compute_answers(&inputs, &config);
        assert_eq!(metrics.long_pauses, Some(2));
    }

    #[test]
    fn long_pauses_missing_timing_marks_unavailable() {
        let inputs = [
            answer("有时间的回答").with_timing(0, 1_000),
            answer("没有时间"),
        ];
        let metrics = compute_answers(&inputs, &config());
        assert_eq!(metrics.long_pauses, None);
    }

    #[test]
    fn long_pauses_threshold_configurable_and_single_answer_is_zero() {
        let mut config = config();
        config.long_pause_seconds = 1.0;
        let single = [answer("只有一条回答").with_timing(0, 500)];
        let metrics = compute_answers(&single, &config);
        assert_eq!(metrics.long_pauses, Some(0));

        let inputs = [
            answer("第一答").with_timing(0, 1_000),
            answer("第二答").with_timing(1_600, 2_000), // 间隔 0.6 秒
        ];
        let metrics = compute_answers(&inputs, &config);
        assert_eq!(metrics.long_pauses, Some(0));
    }

    // ---------- 结构信号与 STAR ----------

    #[test]
    fn structure_signals_detect_ordered_markers() {
        let text = "首先介绍背景，其次讲方案，然后落地，最后总结，结果是指标翻倍。";
        let inputs = [answer(text)];
        let signals = &compute_answers(&inputs, &config()).answers[0].structure_signals;
        let kinds: Vec<StructureKind> = signals.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&StructureKind::Opening));
        assert!(kinds.contains(&StructureKind::Enumeration));
        assert!(kinds.contains(&StructureKind::Closing));
        assert!(kinds.contains(&StructureKind::Result));
    }

    #[test]
    fn structure_signals_plain_answer_has_no_false_positives() {
        let inputs = [answer("我就直接把数据库分了库，把缓存加上了。")];
        let metrics = compute_answers(&inputs, &config());
        assert!(metrics.answers[0].structure_signals.is_empty());
        assert!(metrics.answers[0].star_coverage.is_empty());
    }

    #[test]
    fn star_coverage_detects_all_four_parts() {
        let text =
            "当时项目背景很紧，我负责网关模块，我用灰度发布推进，最终上线了，结果是可用性提升了。";
        let inputs = [answer(text)];
        let coverage = &compute_answers(&inputs, &config()).answers[0].star_coverage;
        assert_eq!(
            coverage,
            &[
                StarCoverage::Situation,
                StarCoverage::Task,
                StarCoverage::Action,
                StarCoverage::Result
            ]
        );
    }

    #[test]
    fn structure_and_star_work_on_mixed_language() {
        let text = "OK, 首先 I checked the logs, then 我用 perf 定位热点，最终 p99 下降了。";
        let inputs = [answer(text)];
        let answer_metrics = &compute_answers(&inputs, &config()).answers[0];
        assert!(
            answer_metrics
                .structure_signals
                .iter()
                .any(|signal| signal.kind == StructureKind::Opening)
        );
        assert!(answer_metrics.star_coverage.contains(&StarCoverage::Action));
        // 混合语料同时计入两套语速分母。
        assert!(answer_metrics.chinese_chars > 0 && answer_metrics.english_words > 0);
    }
}
