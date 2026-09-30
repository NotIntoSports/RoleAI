pub const CHUNKER_VERSION: &str = "resume-semantic-v1";

pub(crate) const MAX_CHUNK_RUNES: usize = 2000;
const HARD_SPLIT_LOOKBACK: usize = 400;
const MAX_CHUNKS: usize = 500;

const SECTION_HEADINGS: &[&str] = &[
    "工作经历",
    "工作经验",
    "实习经历",
    "项目经历",
    "项目经验",
    "教育背景",
    "教育经历",
    "专业技能",
    "技能特长",
    "自我评价",
    "个人总结",
    "experience",
    "work experience",
    "projects",
    "education",
    "skills",
    "summary",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialChunk {
    pub index: i64,
    pub content: String,
    pub section: String,
    pub start_char: usize,
    pub end_char: usize,
    pub size_estimate: i64,
}

struct Section {
    heading: String,
    body: String,
}

pub fn chunk_text(source_label: &str, text: &str) -> Vec<MaterialChunk> {
    let cleaned = clean_text(text);
    if cleaned.trim().is_empty() {
        return Vec::new();
    }

    let mut raw = Vec::new();
    let mut sections_for_raw = Vec::new();
    for section in split_sections(&cleaned) {
        let pieces = split_section(source_label, &section.heading, &section.body);
        sections_for_raw.extend(std::iter::repeat_n(section.heading.clone(), pieces.len()));
        raw.extend(pieces);
    }
    if raw.is_empty() {
        raw = split_oversized(&cleaned);
        sections_for_raw = vec![String::new(); raw.len()];
    }

    let mut chunks = Vec::new();
    for (content, section) in raw.into_iter().zip(sections_for_raw) {
        let content = content.trim().to_owned();
        if content.is_empty() {
            continue;
        }
        let (start_char, end_char) = locate_offsets(&cleaned, &content);
        chunks.push(MaterialChunk {
            index: chunks.len() as i64,
            size_estimate: content.chars().count() as i64,
            start_char,
            end_char,
            section,
            content,
        });
        if chunks.len() >= MAX_CHUNKS {
            break;
        }
    }
    chunks
}

/// 属性测试专用的清洗视图（真实清洗路径零拷贝转发），供 prop_tests 校验分块切片。
#[cfg(test)]
pub(crate) fn __prop_cleaned(text: &str) -> String {
    clean_text(text)
}

/// 属性测试专用：规范段头清单（分块会把段头规范化为这些形）。
#[cfg(test)]
pub(crate) fn __prop_section_headings() -> Vec<String> {
    SECTION_HEADINGS
        .iter()
        .map(|heading| heading.chars().filter(|c| !c.is_whitespace()).collect())
        .collect()
}

fn clean_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut kept = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.chars().count() < 2 {
            if line.trim().is_empty() {
                kept.push(String::new());
            }
            continue;
        }
        kept.push(line);
    }
    kept.join("\n").trim().to_owned()
}

fn split_sections(text: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current = Section {
        heading: String::new(),
        body: String::new(),
    };
    let flush = |current: &mut Section, sections: &mut Vec<Section>| {
        let body = current.body.trim();
        if body.is_empty() {
            return;
        }
        sections.push(Section {
            heading: current.heading.clone(),
            body: body.to_owned(),
        });
    };

    for line in text.lines() {
        if let Some(heading) = match_heading(line) {
            flush(&mut current, &mut sections);
            current = Section {
                heading,
                body: String::new(),
            };
            continue;
        }
        if !current.body.is_empty() {
            current.body.push('\n');
        }
        current.body.push_str(line);
    }
    flush(&mut current, &mut sections);
    if sections.is_empty() && !text.trim().is_empty() {
        return vec![Section {
            heading: String::new(),
            body: text.trim().to_owned(),
        }];
    }
    sections
}

fn match_heading(line: &str) -> Option<String> {
    let trimmed = line.trim().trim_matches(['：', ':']);
    let normalized = trimmed.to_lowercase();
    SECTION_HEADINGS
        .iter()
        .find(|heading| normalized == heading.to_lowercase())
        .map(|heading| (*heading).to_owned())
}

fn split_section(candidate: &str, heading: &str, body: &str) -> Vec<String> {
    if is_experience_heading(heading) {
        return chunk_blocks(candidate, heading, &split_experience_blocks(body));
    }
    if heading.is_empty() {
        let mut blocks = split_blank_paragraphs(body);
        if blocks.len() <= 1 {
            let items = split_bullet_items(body);
            if items.len() > 1 {
                blocks = items;
            }
        }
        return chunk_blocks(candidate, heading, &blocks);
    }
    let title = first_line_title(body);
    split_oversized(&with_prefix(candidate, heading, &title, body))
}

fn chunk_blocks(candidate: &str, heading: &str, blocks: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for block in blocks {
        let title = first_line_title(block);
        out.extend(split_oversized(&with_prefix(
            candidate, heading, &title, block,
        )));
    }
    out
}

fn split_blank_paragraphs(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let flush = |current: &mut Vec<&str>, blocks: &mut Vec<String>| {
        let text = current.join("\n").trim().to_owned();
        if !text.is_empty() {
            blocks.push(text);
        }
        current.clear();
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            flush(&mut current, &mut blocks);
            continue;
        }
        current.push(line);
    }
    flush(&mut current, &mut blocks);
    if blocks.is_empty() && !body.trim().is_empty() {
        return vec![body.trim().to_owned()];
    }
    blocks
}

fn is_experience_heading(heading: &str) -> bool {
    matches!(
        heading.to_lowercase().as_str(),
        "工作经历"
            | "工作经验"
            | "实习经历"
            | "项目经历"
            | "项目经验"
            | "experience"
            | "work experience"
            | "projects"
    )
}

fn split_experience_blocks(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let flush = |current: &mut Vec<&str>, blocks: &mut Vec<String>| {
        let text = current.join("\n").trim().to_owned();
        if !text.is_empty() {
            blocks.push(text);
        }
        current.clear();
    };
    for line in body.lines() {
        if is_date_line(line.trim()) && !current.is_empty() {
            flush(&mut current, &mut blocks);
        }
        current.push(line);
    }
    flush(&mut current, &mut blocks);
    if blocks.is_empty() && !body.trim().is_empty() {
        return vec![body.trim().to_owned()];
    }
    blocks
}

fn first_line_title(block: &str) -> String {
    let line = block.lines().next().unwrap_or("").trim();
    let runes: Vec<char> = line.chars().collect();
    if runes.len() > 80 {
        runes[..80].iter().collect()
    } else {
        line.to_owned()
    }
}

fn with_prefix(candidate: &str, heading: &str, title: &str, body: &str) -> String {
    let label = [candidate, heading, title]
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    if label.is_empty() {
        return body.trim().to_owned();
    }
    format!("[{label}]\n{}", body.trim())
}

fn split_oversized(text: &str) -> Vec<String> {
    if text.chars().count() <= MAX_CHUNK_RUNES {
        if text.trim().is_empty() {
            return Vec::new();
        }
        return vec![text.to_owned()];
    }
    let (prefix, body) = match text.split_once("]\n") {
        Some((head, rest)) if text.starts_with('[') => (format!("{head}]\n"), rest.to_owned()),
        _ => (String::new(), text.to_owned()),
    };
    let items = split_bullet_items(&body);
    if items.len() > 1 {
        return items
            .into_iter()
            .flat_map(|item| split_oversized(&format!("{prefix}{}", item.trim())))
            .collect();
    }
    hard_split(text, &prefix)
}

fn split_bullet_items(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = Vec::new();
    let mut found = false;
    let flush = |current: &mut Vec<&str>, items: &mut Vec<String>| {
        let text = current.join("\n").trim().to_owned();
        if !text.is_empty() {
            items.push(text);
        }
        current.clear();
    };
    for line in body.lines() {
        if is_bullet_line(line.trim()) && !current.is_empty() {
            found = true;
            flush(&mut current, &mut items);
        }
        current.push(line);
    }
    flush(&mut current, &mut items);
    if !found {
        return Vec::new();
    }
    items
}

fn hard_split(text: &str, prefix: &str) -> Vec<String> {
    let runes: Vec<char> = text.chars().collect();
    // 非首个块会前置段首上下文（prefix），切块预算必须为前缀留位，
    // 保证任何块（含前缀）都不超过 MAX_CHUNK_RUNES（proptest C51 回归）。
    let prefix_runes = prefix
        .chars()
        .count()
        .min(MAX_CHUNK_RUNES.saturating_sub(1));
    let mut out = Vec::new();
    let mut start = 0;
    while start < runes.len() {
        let remaining = &runes[start..];
        let budget = if start > 0 && !prefix.is_empty() {
            MAX_CHUNK_RUNES - prefix_runes
        } else {
            MAX_CHUNK_RUNES
        };
        if remaining.len() <= budget {
            let mut piece = remaining.iter().collect::<String>();
            piece = piece.trim().to_owned();
            if start > 0 && !prefix.is_empty() && !piece.starts_with('[') {
                let context: String = prefix.chars().take(prefix_runes).collect();
                piece = format!("{context}{piece}").trim().to_owned();
            }
            if !piece.is_empty() {
                out.push(piece);
            }
            break;
        }
        let window = &remaining[..budget];
        let mut cut = find_split(window);
        if cut == 0 {
            cut = budget;
        }
        let mut piece = remaining[..cut].iter().collect::<String>();
        piece = piece.trim().to_owned();
        if start > 0 && !prefix.is_empty() && !piece.starts_with('[') {
            let context: String = prefix.chars().take(prefix_runes).collect();
            piece = format!("{context}{piece}").trim().to_owned();
        }
        if !piece.is_empty() {
            out.push(piece);
        }
        start += cut;
    }
    out
}

fn find_split(window: &[char]) -> usize {
    if window.is_empty() {
        return 0;
    }
    let begin = window.len().saturating_sub(HARD_SPLIT_LOOKBACK);
    for i in (begin..window.len()).rev() {
        if window[i] == '\n' {
            if i + 1 < window.len() && window[i + 1] == '\n' {
                return i + 2;
            }
            return i + 1;
        }
    }
    for i in (begin..window.len()).rev() {
        if window[i] == '。' || window[i] == '！' || window[i] == '？' {
            if i > 0 && window[i - 1] == window[i] {
                continue;
            }
            return i + 1;
        }
    }
    0
}

fn locate_offsets(haystack: &str, content: &str) -> (usize, usize) {
    let needle = if content.starts_with('[') {
        content
            .split_once("]\n")
            .map(|(_, body)| body)
            .unwrap_or(content)
    } else {
        content
    };
    if let Some(byte) = haystack.find(needle) {
        let start = haystack[..byte].chars().count();
        return (start, start + needle.chars().count());
    }
    (0, content.chars().count())
}

fn is_date_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    if bytes.len() < 6 {
        return false;
    }
    let mut index = 0;
    if !take_digits(line, &mut index, 4, 4) {
        return false;
    }
    let Some(sep) = line[index..].chars().next() else {
        return false;
    };
    if !matches!(sep, '.' | '/' | '-' | '年') {
        return false;
    }
    index += sep.len_utf8();
    if !take_digits(line, &mut index, 1, 2) {
        return false;
    }
    if line[index..].starts_with('月') {
        index += '月'.len_utf8();
    }
    let rest = line[index..].trim_start();
    if rest.is_empty() {
        return true;
    }
    let rest = match rest
        .strip_prefix("–")
        .or_else(|| rest.strip_prefix("—"))
        .or_else(|| rest.strip_prefix('-'))
        .or_else(|| rest.strip_prefix("至"))
        .or_else(|| rest.strip_prefix("到"))
    {
        Some(rest) => rest.trim_start(),
        None => return true,
    };
    let lowered = rest.to_ascii_lowercase();
    if lowered.starts_with("至今") || lowered.starts_with("现在") || lowered.starts_with("present")
    {
        return true;
    }
    let mut end = 0;
    take_digits(rest, &mut end, 4, 4)
        && rest[end..]
            .chars()
            .next()
            .is_some_and(|sep| matches!(sep, '.' | '/' | '-' | '年'))
}

fn take_digits(text: &str, index: &mut usize, min: usize, max: usize) -> bool {
    let mut count = 0;
    let bytes = text.as_bytes();
    while *index < bytes.len() && bytes[*index].is_ascii_digit() && count < max {
        *index += 1;
        count += 1;
    }
    count >= min
}

fn is_bullet_line(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    if matches!(first, '•' | '·' | '●' | '○' | '■' | '-' | '*' | '、') {
        return line[first.len_utf8()..].starts_with(|ch: char| ch.is_whitespace());
    }
    if first == '（' || first == '(' {
        let close = if first == '（' { '）' } else { ')' };
        let rest = &line[first.len_utf8()..];
        let digits = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        return rest[digits..].starts_with(close);
    }
    let digits = line.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    line[digits..].starts_with(['.', '、'])
}

#[cfg(test)]
mod tests {
    use super::{CHUNKER_VERSION, MAX_CHUNK_RUNES, chunk_text};

    #[test]
    fn chunker_version_is_stable() {
        assert_eq!(CHUNKER_VERSION, "resume-semantic-v1");
    }

    /// 回归（proptest C51 发现）：hard_split 给非首个块前置段首上下文时，
    /// 切块预算必须为前缀留位，否则块可达 2006 > 硬上限 2000。
    #[test]
    fn hard_split_with_prefix_respects_hard_cap() {
        let mut text = String::from("[会议纪要]\n");
        text.push_str(&"甲".repeat(MAX_CHUNK_RUNES * 2));
        let chunks = chunk_text("prop", &text);
        assert!(chunks.len() >= 2, "长文本必须被切分");
        for chunk in &chunks {
            assert!(
                chunk.size_estimate <= MAX_CHUNK_RUNES as i64,
                "块 {} 大小 {} 超过硬上限 {MAX_CHUNK_RUNES}",
                chunk.index,
                chunk.size_estimate
            );
        }
    }

    #[test]
    fn keeps_whole_job_block() {
        let text = "工作经历\n2019.03-2021.06 阿里巴巴 高级工程师\n负责订单服务与 Kafka 链路，使用 Node.js 完成 2019.03 版本升级。\n日常维护 Vue.js 与 v1.2 配置，数值约 3.14。";
        let chunks = chunk_text("张三", text);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("Node.js"));
        assert!(chunks[0].content.contains("2019.03 版本"));
        assert!(
            chunks[0]
                .content
                .starts_with("[张三 | 工作经历 | 2019.03-2021.06 阿里巴巴 高级工程师]")
        );
        assert_eq!(chunks[0].section, "工作经历");
    }

    #[test]
    fn splits_on_date_lines_only() {
        let text = "工作经历\n2019.03-2021.06 阿里巴巴\n负责 Node.js 与 2019.03 内部版本。\n2021.07-至今 腾讯\n负责 Kafka 订单链路。";
        let chunks = chunk_text("李四", text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content.contains("阿里巴巴"));
        assert!(!chunks[0].content.contains("腾讯"));
        assert!(chunks[1].content.contains("腾讯"));
    }

    #[test]
    fn splits_oversized_by_bullets_with_repeated_prefix() {
        let body = "职责说明".repeat(280);
        let text = format!("工作经历\n2018.01-2019.01 某公司\n• {body}\n• {body}");
        let chunks = chunk_text("王五", &text);
        assert!(
            chunks.len() >= 2,
            "expected bullet split, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(
                chunk.content.chars().count() <= 2000,
                "chunk exceeded safety valve: {}",
                chunk.content.chars().count()
            );
            assert!(
                chunk
                    .content
                    .contains("[王五 | 工作经历 | 2018.01-2019.01 某公司]")
            );
        }
    }

    #[test]
    fn without_headings_uses_blank_lines() {
        let chunks = chunk_text("赵六", "第一段公司经历内容。\n\n第二段项目经历内容。");
        assert_eq!(chunks.len(), 2);
        let joined = chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains(&"第一段".repeat(2)));
    }

    /// 清洗边界：单字符行（哼声/散字）必须整行丢弃，两字符行必须保留，
    /// 空行保留为段落分隔——三者的判定窗口恰好卡在 chars().count() < 2。
    #[test]
    fn clean_text_drops_single_char_lines_but_keeps_two_char_lines() {
        let text = "首段内容\n甲\n乙乙\n\n次段内容";
        let chunks = chunk_text("钱八", &text);
        assert_eq!(chunks.len(), 2, "空行必须保住两个段落");
        assert!(
            chunks[0].content.contains("乙乙") && !chunks[0].content.contains('甲'),
            "两字符行必须保留、单字符行必须丢弃：{}",
            chunks[0].content
        );
    }

    /// 无段头分块的子弹回退只发生在"整段只有一个块"时：多段落正文保持
    /// 段落切块，不得被子弹项列表顶掉；且单个子弹项不构成回退条件。
    #[test]
    fn bullet_fallback_requires_single_block_and_multiple_items() {
        let multi_paragraph = "甲段说明\n\n• 乙项\n• 丙项\n• 丁项";
        let chunks = chunk_text("孙九", multi_paragraph);
        assert_eq!(chunks.len(), 2, "多段落正文必须按段落切，不得退化为逐子弹项");
        assert!(chunks[0].content.contains("甲段") && !chunks[0].content.contains('•'));

        let single_bullet = "甲段说明\n\n• 乙项";
        let chunks = chunk_text("孙九", single_bullet);
        assert_eq!(chunks.len(), 2);
        assert!(
            chunks[1].content.contains('•'),
            "单个子弹项必须留在原块内，不得顶掉段落块"
        );
    }

    /// 教育背景不走经历日期拆分：只有经历类段头按日期行切块，
    /// 其它段头保持"首行标题 + 整段"单块形态。
    #[test]
    fn education_section_is_not_split_by_date_lines() {
        let text = "教育背景\n2018.09-2022.06 某大学 本科\n主修计算机科学与技术";
        let chunks = chunk_text("吴十", &text);
        assert_eq!(chunks.len(), 1, "教育背景不得按日期行拆成多块");
        assert!(chunks[0].content.contains("主修计算机科学与技术"));
    }

    /// 硬切优先落在预算窗口内的空行边界（\n\n）上：首块止于空行前，
    /// 次块从空行后开始；非首块必须携带段首上下文且上下文恰好出现一次；
    /// 字符偏移必须重新定位到清洗文本的真实位置。
    #[test]
    fn hard_split_prefers_blank_line_boundary_in_lookback() {
        let text = format!("[会议纪要]\n{}\n\n{}", "甲".repeat(1900), "乙".repeat(1200));
        let chunks = chunk_text("周七", &text);
        assert_eq!(chunks.len(), 2, "甲/乙 两段必须切成两块");
        assert!(
            chunks[0].content.contains('甲') && !chunks[0].content.contains('乙'),
            "首块不得跨过空行边界"
        );
        assert!(
            chunks[1].content.contains('乙') && !chunks[1].content.contains('甲'),
            "次块必须从空行之后开始"
        );
        for chunk in &chunks {
            assert!(
                chunk.content.matches("[周七").count() == 1,
                "段首上下文必须恰好一次：{}",
                &chunk.content[..chunk.content.len().min(40)]
            );
        }
        assert_eq!(chunks[0].start_char, 0, "首块从清洗文本开头开始");
        assert!(
            chunks[1].start_char > 0 && chunks[1].end_char - chunks[1].start_char == 1200,
            "次块偏移必须定位到乙段真实位置（1200 字）"
        );
    }

    /// 无空行可切时回退到句读边界：回溯窗内从后往前找第一个 。！？；
    /// 叠标点（！！）只认收尾的那个；切点必须落在标点之后而不是标点前。
    #[test]
    fn hard_split_falls_back_to_sentence_break_and_skips_doubled_punctuation() {
        let text = format!("[会议纪要]\n{}完。成！！{}", "甲".repeat(1900), "乙".repeat(100));
        let chunks = chunk_text("周七", &text);
        assert_eq!(chunks.len(), 2, "长文必须切两块");
        assert!(
            chunks[0].content.contains("完。") && chunks[0].content.contains('！'),
            "首块必须包含第一句与第一个叹号"
        );
        assert!(
            !chunks[0].content.contains("！！") && !chunks[0].content.contains('乙'),
            "切点必须落在叠叹号之间，不得把乙段并进首块"
        );
        assert!(
            chunks[1].content.starts_with("[周七")
                && chunks[1].content.contains('乙')
                && !chunks[1].content.contains("完。"),
            "次块必须带上下文且从第二个叹号后开始"
        );
    }
}
