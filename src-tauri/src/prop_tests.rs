//! 属性测试（lane-C C51，proptest）：分块、PCM 重采样、协议解析、
//! 会话导出与配置反序列化的不变量。随机 + 自动缩减，种子可复现。

use proptest::prelude::*;

use crate::audio::pcm::{ASR_SAMPLE_RATE, CAPTURE_SAMPLE_RATE, resample_pcm16_mono};
use crate::materials::chunk::{MAX_CHUNK_RUNES, chunk_text};
use crate::providers::parse_server_event;
use crate::sessions::{SessionExportFormat, SessionStore};

// ------------------------------------------------------------ 策略 ----

/// 任意 UTF-8 文本：含 CJK、emoji、换行、空白与可见字符的组合。
fn arb_text() -> impl Strategy<Value = String> {
    r"[^\x00]{0,6000}"
}

fn arb_pcm_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..=20_000)
}

fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop::option::of(r"[\x20-\x7e一-鿿]{0,40}")
        .prop_map(|s| serde_json::Value::String(s.unwrap_or_default()));
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(serde_json::Value::Array),
            proptest::collection::hash_map(r"[a-zA-Z_]{0,12}", inner, 0..8)
                .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
            any::<bool>().prop_map(serde_json::Value::Bool),
            any::<f64>().prop_map(|v| serde_json::json!(v)),
        ]
    })
}

// ------------------------------------------------------------ 分块 ----

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn chunk_text_bounds_and_no_panic(text in arb_text()) {
        let chunks = chunk_text("prop", &text);
        prop_assert!(chunks.len() <= 500, "块数超过单篇上限");
        for chunk in &chunks {
            prop_assert!(!chunk.content.trim().is_empty(), "不允许空块");
            prop_assert!(
                chunk.size_estimate <= MAX_CHUNK_RUNES as i64,
                "块大小 {} 超过硬上限 {}",
                chunk.size_estimate,
                MAX_CHUNK_RUNES
            );
            prop_assert_eq!(chunk.size_estimate, chunk.content.chars().count() as i64);
            prop_assert_eq!(
                chunk.index,
                chunks.iter().position(|c| c.index == chunk.index).unwrap() as i64
            );
        }
    }

    // ------------------------------------------------------------ PCM ----

    #[test]
    fn resample_48k_to_16k_length_is_third_of_samples(pcm in arb_pcm_bytes()) {
        let out = resample_pcm16_mono(&pcm, CAPTURE_SAMPLE_RATE, ASR_SAMPLE_RATE);
        let samples = pcm.len() / 2;
        prop_assert_eq!(out.len(), samples / 3 * 2, "48k→16k 输出必须是输入样本数的 1/3");
    }

    #[test]
    fn resample_never_panics_and_respects_output_bound(
        pcm in arb_pcm_bytes(),
        from in 1u32..=192_000,
        to in 1u32..=192_000,
    ) {
        let out = resample_pcm16_mono(&pcm, from, to);
        let samples = pcm.len() / 2;
        let expected = (samples as u64 * to as u64 / from as u64) as usize;
        prop_assert!(out.len() <= expected * 2 + 2, "输出样本数不得超过按比例推算的上限");
        // 输出必须是偶数字节（完整 16-bit 样本）。
        prop_assert_eq!(out.len() % 2, 0);
    }

    // ------------------------------------------------------------ 配置反序列化 ----

    #[test]
    fn config_from_json_never_panics_and_keeps_error_prefix(json in r"[sS]{0,4000}") {
        // 任意输入：不 panic；错误码一律带 CONFIG_ 前缀（稳定契约）。
        if let Err(error) = crate::config::AppConfigV1::from_json(&json) {
            prop_assert!(error.code().starts_with("CONFIG_"), "意外错误码 {}", error.code());
        }
    }

    // ------------------------------------------------------------ 协议解析 ----

    #[test]
    fn parse_server_event_never_panics_on_arbitrary_json(value in arb_json_value()) {
        // 任意 JSON：要么解析出事件，要么返回 Err——都不允许 panic。
        let _ = parse_server_event(&serde_json::to_string(&value).unwrap_or_default());
    }

    #[test]
    fn parse_server_event_error_codes_keep_contract(value in arb_json_value()) {
        if let Err(error) = parse_server_event(&serde_json::to_string(&value).unwrap_or_default()) {
            let code = error.code();
            prop_assert!(!code.is_empty(), "错误码不得为空");
            // 两条稳定契约：本crate产生的错误码带 REALTIME_ 前缀；
            // Remote 变体按设计透传服务端给出的错误码（可能是任意非空串）。
            prop_assert!(
                code.starts_with("REALTIME_") || matches!(error, crate::providers::RealtimeError::Remote(_)),
                "错误码契约被破坏：{code}"
            );
        }
    }

    // ------------------------------------------------------------ 会话导出 ----

    #[test]
    fn json_export_round_trips_arbitrary_turn_text(
        user_text in arb_text(),
        assistant_text in arb_text(),
    ) {
        // 任意文本进导出：文件必须是合法 JSON 且原文逐字保留（JSON 转义可逆）。
        let directory = tempfile::tempdir().expect("tempdir");
        let database =
            crate::database::Database::open(directory.path().join("prop.sqlite3")).expect("open");
        database.migrate().expect("migrate");
        let store = SessionStore::new(&database);
        store
            .insert_session(crate::sessions::NewSession {
                id: "prop-session",
                status: "listening",
                role_profile_id: "role-1",
                voice_route_id: "route-1",
                transport_mode: "direct",
            })
            .expect("insert session");
        store
            .insert_turn(crate::sessions::NewTurn {
                id: "prop-turn",
                session_id: "prop-session",
                turn_index: 0,
                user_text: &user_text,
                assistant_text: &assistant_text,
                materials_used: false,
            })
            .expect("insert turn");
        let export_root = directory.path().join("exports");
        let path = crate::sessions::export_session(
            &store,
            "prop-session",
            SessionExportFormat::Json,
            &export_root,
        )
        .expect("export");
        let body = std::fs::read_to_string(path).expect("read export");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("导出必须是合法 JSON");
        let turns = parsed["turns"].as_array().expect("turns array");
        prop_assert_eq!(turns.len(), 1);
        prop_assert_eq!(turns[0]["userText"].as_str().unwrap_or(""), user_text);
        prop_assert_eq!(turns[0]["assistantText"].as_str().unwrap_or(""), assistant_text);
    }
}
