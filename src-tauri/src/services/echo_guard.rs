//! 文本级回声过滤：AI 播报声从扬声器被麦克风回收后，ASR 会把它转写成
//! 「用户发言」，模型再对着自己的话作答，形成自问自答循环。
//! 音频闸门（上行门控 / 回声抑制窗）是第一道防线；这里是最后一道——
//! 把定稿的用户转写与最近播报文本做归一化比对，命中即整轮丢弃。
//!
//! 归一化必须抹平 TTS 文本与 ASR 转写的系统性差异：TTS 念
//! 「现在是2026年9月28日，星期一。」，同一句话被 ASR 转写常是
//! 「现在是二零二六年九月二十八日星期一。」——数字与中文数字互换、
//! 标点增删。所以归一化统一转小写、全角折半角、丢弃标点空白，
//! 并把中文数字串换算成阿拉伯数字后再比对。

use std::time::{Duration, Instant};

/// 短于该长度（归一化后）的转写不参与判定：哼声/单字应答不允许被误吞。
const ECHO_MIN_CHARS: usize = 6;
/// 包含命中的最短长度：回声常是上句的截断片段（只听到半句）。
const CONTAIN_MIN_CHARS: usize = 8;
/// 字符 bigram Dice 系数命中阈值。取高不取低：漏掉一条回声的代价是
/// 自问自答循环（音频闸门会兜住绝大多数），误吞真人确认句的代价是
/// 用户被无视。实测「没错就是2026年9月28日星期一」对
/// 「现在是2026年9月28日星期一」约 0.84，必须放行；同句回声 ≥0.9。
const DICE_ECHO_THRESHOLD: f64 = 0.85;

/// 判定 `user_text` 是否为 `recent_assistant` 中某条最近播报的回声。
pub fn is_echo(user_text: &str, recent_assistant: &[String]) -> bool {
    let user = normalize(user_text);
    if user.chars().count() < ECHO_MIN_CHARS {
        return false;
    }
    let user_bigrams = char_bigrams(&user);
    for spoken in recent_assistant {
        let spoken = normalize(spoken);
        if spoken.chars().count() < ECHO_MIN_CHARS {
            continue;
        }
        let shorter = user.chars().count().min(spoken.chars().count());
        if shorter >= CONTAIN_MIN_CHARS
            && (spoken.contains(user.as_str()) || user.contains(spoken.as_str()))
        {
            return true;
        }
        let spoken_bigrams = char_bigrams(&spoken);
        if user_bigrams.is_empty() || spoken_bigrams.is_empty() {
            continue;
        }
        let intersection = user_bigrams.intersection(&spoken_bigrams).count();
        let dice = 2.0 * intersection as f64 / (user_bigrams.len() + spoken_bigrams.len()) as f64;
        if dice >= DICE_ECHO_THRESHOLD {
            return true;
        }
    }
    false
}

/// 归一化：小写、全角→半角、只保留字母数字与 CJK 表意字符、中文数字→阿拉伯数字。
pub fn normalize(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    for ch in text.chars() {
        // 全角 ASCII 区（！-～）折回半角。
        let ch = if (0xFF01..=0xFF5E).contains(&(ch as u32)) {
            char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch)
        } else {
            ch
        };
        for low in ch.to_lowercase() {
            if low.is_ascii_alphanumeric() || is_cjk_ideograph(low) {
                stripped.push(low);
            }
            // 标点、空白、符号一律丢弃。
        }
    }
    convert_chinese_numerals(&stripped)
}

fn is_cjk_ideograph(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF)
}

/// 把字符串里的连续中文数字串换算成阿拉伯数字。
/// 「二零二六」→2026（纯数位串按位拼接），「二十八」→28（含单位按位值合成）。
/// 「星期一」→「星期1」这类误换算无碍判定：两侧归一化路径一致。
fn convert_chinese_numerals(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run: Vec<char> = Vec::new();
    for ch in text.chars() {
        if numeral_digit(ch).is_some() || numeral_unit(ch).is_some() {
            run.push(ch);
        } else {
            flush_numeral_run(&mut run, &mut out);
            out.push(ch);
        }
    }
    flush_numeral_run(&mut run, &mut out);
    out
}

fn flush_numeral_run(run: &mut Vec<char>, out: &mut String) {
    if run.is_empty() {
        return;
    }
    let composed = if run.iter().all(|ch| numeral_digit(*ch).is_some()) {
        // 纯数位串（电话号码式报数）：按位拼接。
        run.iter()
            .filter_map(|ch| numeral_digit(*ch))
            .try_fold(0u64, |acc, digit| {
                acc.checked_mul(10).and_then(|acc| acc.checked_add(digit))
            })
    } else {
        compose_positional(run)
    };
    match composed {
        Some(value) => out.push_str(&value.to_string()),
        None => out.extend(run.iter()),
    }
    run.clear();
}

/// 含单位的中文数字按位值合成（标准算法）：十百千进位，万/亿开新段。
fn compose_positional(run: &[char]) -> Option<u64> {
    let mut total: u64 = 0;
    let mut section: u64 = 0;
    let mut digit: Option<u64> = None;
    for &ch in run {
        if let Some(value) = numeral_digit(ch) {
            digit = Some(value);
        } else {
            let unit = numeral_unit(ch)?;
            if unit >= 10_000 {
                section = (section + digit.take().unwrap_or(0)).checked_mul(unit)?;
                total += section;
                section = 0;
            } else {
                section += digit.take().unwrap_or(1).checked_mul(unit)?;
            }
        }
    }
    total.checked_add(section)?.checked_add(digit.unwrap_or(0))
}

fn numeral_digit(ch: char) -> Option<u64> {
    match ch {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

fn numeral_unit(ch: char) -> Option<u64> {
    match ch {
        '十' => Some(10),
        '百' => Some(100),
        '千' => Some(1_000),
        '万' => Some(10_000),
        '亿' => Some(100_000_000),
        _ => None,
    }
}

fn char_bigrams(text: &str) -> std::collections::HashSet<(char, char)> {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

// 音频闸门（上行门控）的纯计时计算，纯搬移自 services/realtime_pump.rs。
// 播放设备交互（播净回执、存活探测、缓冲清空）留在泵内，这里只做时刻运算。

/// 播净回执丢失时的短兜底尾窗；真实回执仍优先立即开门。
pub const GATE_DRAIN_FALLBACK_TAIL: Duration = Duration::from_millis(1500);
/// 关门安全阀：兜底链路全部失效时，关门最长这么久后强制重开，
/// 麦克风不被 20 秒安全阀无限闭锁后丢句。
const GATE_FORCE_OPEN_AFTER: Duration = Duration::from_secs(20);

/// 排水兜底与安全阀是否已到点（不含设备侧因素：播净回执/存活由泵另行判定）。
pub fn gate_timers_expired(
    drain_deadline: Option<Instant>,
    closed_at: Option<Instant>,
    now: Instant,
) -> bool {
    drain_deadline.is_some_and(|at| now >= at)
        || closed_at.is_some_and(|at| now.duration_since(at) > GATE_FORCE_OPEN_AFTER)
}

/// 已写入字节数 → 排水兜底截止时刻：24kHz×16bit（48 000 B/s）折算播放时长 + 尾窗。
pub fn gate_drain_deadline_from_bytes(now: Instant, bytes: usize) -> Instant {
    now + Duration::from_millis((bytes as u64 * 1000) / (24_000 * 2)) + GATE_DRAIN_FALLBACK_TAIL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_echo_detected_across_chinese_numeral_forms() {
        let spoken = vec!["现在是2026年9月28日，星期一。".to_string()];
        assert!(is_echo("现在是二零二六年九月二十八日星期一。", &spoken,));
        // 完全同形也要命中。
        assert!(is_echo("现在是2026年9月28日，星期一。", &spoken));
    }

    /// 真人确认/复述场景不得误吞：措辞有增量（没错/就是/问句）就放行。
    #[test]
    fn genuine_confirmation_is_not_echo() {
        let spoken = vec!["现在是2026年9月28日，星期一。".to_string()];
        assert!(!is_echo("没错，就是2026年9月28日，星期一。", &spoken));
        assert!(!is_echo("那明天呢", &spoken));
        assert!(!is_echo("帮我记一下今天日期", &spoken));
    }

    /// 回声的截断片段（只听到半句）靠包含关系命中。
    #[test]
    fn partial_fragment_echo_is_detected_by_containment() {
        let spoken = vec!["现在是2026年9月28日，星期一。".to_string()];
        assert!(is_echo("二零二六年九月二十八日", &spoken));
        assert!(!is_echo("星期一。", &spoken), "过短片段不得命中");
    }

    #[test]
    fn short_or_empty_input_is_never_echo() {
        let spoken = vec!["现在是2026年9月28日，星期一。".to_string()];
        assert!(!is_echo("好", &spoken));
        assert!(!is_echo("嗯嗯", &spoken));
        assert!(!is_echo("", &spoken));
    }

    #[test]
    fn unrelated_text_is_not_echo() {
        let spoken = vec!["现在是2026年9月28日，星期一。".to_string()];
        assert!(!is_echo("今天天气怎么样", &spoken));
        assert!(!is_echo("明天提醒我开会", &spoken));
    }

    /// 上一轮播报是回声轮的应答时（如「没错，就是……」），下一轮回声
    /// 要能对上更早那条原始播报——调用方保留最近 2~3 条即可覆盖。
    #[test]
    fn echo_matches_any_of_the_recent_spoken_texts() {
        let spoken = vec![
            "没错，就是2026年9月28日，星期一。".to_string(),
            "现在是2026年9月28日，星期一。".to_string(),
        ];
        assert!(is_echo("现在是二零二六年九月二十八日星期一。", &spoken));
    }

    #[test]
    fn normalization_unifies_width_punct_and_numerals() {
        assert_eq!(
            normalize("现在是2026年9月28日，星期一。"),
            "现在是2026年9月28日星期1"
        );
        assert_eq!(
            normalize("现在是二零二六年九月二十八日星期一。"),
            "现在是2026年9月28日星期1"
        );
        assert_eq!(normalize("ＡＢＣ１２３！"), "abc123");
        assert_eq!(normalize("二十八"), "28");
        assert_eq!(normalize("十"), "10");
        assert_eq!(normalize("一百零五"), "105");
        assert_eq!(normalize("三万"), "30000");
    }

    #[test]
    fn gate_timers_expire_on_drain_deadline_or_force_open() {
        let now = Instant::now();
        // 排水截止未到：不开。
        assert!(!gate_timers_expired(
            Some(now + Duration::from_secs(1)),
            Some(now),
            now
        ));
        // 排水截止已过：开。
        assert!(gate_timers_expired(
            Some(now - Duration::from_millis(1)),
            Some(now),
            now
        ));
        // 无排水截止，但关门已超 20s 安全阀：开。
        assert!(gate_timers_expired(
            None,
            Some(now - GATE_FORCE_OPEN_AFTER - Duration::from_millis(1)),
            now
        ));
        // 刚关门且无排水截止：不开。
        assert!(!gate_timers_expired(None, Some(now), now));
        // 无关门记录：不开。
        assert!(!gate_timers_expired(None, None, now));
    }

    #[test]
    fn gate_drain_deadline_scales_with_bytes_plus_tail() {
        let now = Instant::now();
        // 48 000 B/s：0.5s 音频（24 000 字节）+ 1.5s 尾窗。
        assert_eq!(
            gate_drain_deadline_from_bytes(now, 24_000),
            now + Duration::from_millis(500) + GATE_DRAIN_FALLBACK_TAIL
        );
        assert_eq!(
            gate_drain_deadline_from_bytes(now, 0),
            now + GATE_DRAIN_FALLBACK_TAIL
        );
    }
}
