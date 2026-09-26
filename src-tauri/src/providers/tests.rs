use super::{merge_builtin_catalog, normalize_models_url, parse_model_catalog};

mod cascade {
    use super::super::{
        CascadeError, ChatMessage, ChatModel, OpenAiCompatibleCascade, ProviderEndpoint,
        SpeechToText, TextToSpeech, build_asr_multipart, build_llm_request, build_tts_request,
        json_body_too_large, normalize_chat_completions_url, normalize_speech_url,
        normalize_transcriptions_url, parse_chat_completion, parse_transcript, parse_tts_pcm,
        pcm_to_wav, tts_body_too_large,
    };

    fn le_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    fn le_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn pcm_to_wav_writes_mono_16bit_riff_header() {
        let pcm = [0x11_u8, 0x22, 0x33, 0x44];
        let wav = pcm_to_wav(&pcm, 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(le_u32(&wav, 16), 16);
        assert_eq!(le_u16(&wav, 20), 1);
        assert_eq!(le_u16(&wav, 22), 1);
        assert_eq!(le_u32(&wav, 24), 16_000);
        assert_eq!(le_u32(&wav, 28), 16_000 * 2);
        assert_eq!(le_u16(&wav, 32), 2);
        assert_eq!(le_u16(&wav, 34), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(le_u32(&wav, 40), 4);
        assert_eq!(&wav[44..], &pcm);
        assert_eq!(wav.len(), 48);
        assert_eq!(le_u32(&wav, 4), 40);
    }

    #[test]
    fn joins_openai_compatible_cascade_urls() {
        assert_eq!(
            normalize_transcriptions_url("https://example.test/v1")
                .unwrap()
                .as_str(),
            "https://example.test/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_transcriptions_url("https://example.test/v1/")
                .unwrap()
                .as_str(),
            "https://example.test/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_transcriptions_url("https://example.test/v1/audio/transcriptions")
                .unwrap()
                .as_str(),
            "https://example.test/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_chat_completions_url("https://example.test/v1")
                .unwrap()
                .as_str(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            normalize_speech_url("https://example.test/v1")
                .unwrap()
                .as_str(),
            "https://example.test/v1/audio/speech"
        );
        assert!(normalize_transcriptions_url("ftp://example.test/v1").is_err());
        assert!(normalize_chat_completions_url("https://user@example.test/v1").is_err());
        assert!(normalize_speech_url("https://example.test/v1?marker=synthetic").is_err());
        assert!(normalize_speech_url("https://example.test/v1#synthetic").is_err());
    }

    #[test]
    fn empty_transcript_and_llm_and_odd_pcm_fail_without_body() {
        let transcript =
            parse_transcript(br#"{"text":"","secret":"must-not-escape"}"#).unwrap_err();
        assert_eq!(transcript.code(), "ASR_RESPONSE_INVALID");
        assert!(!transcript.to_string().contains("must-not-escape"));

        let blank = parse_transcript(br#"{"text":"   "}"#).unwrap_err();
        assert_eq!(blank.code(), "ASR_RESPONSE_INVALID");

        let empty_llm = parse_chat_completion(
            br#"{"choices":[{"message":{"content":""}}],"marker":"must-not-escape"}"#,
        )
        .unwrap_err();
        assert_eq!(empty_llm.code(), "LLM_RESPONSE_EMPTY");
        assert!(!empty_llm.to_string().contains("must-not-escape"));

        let whitespace_llm =
            parse_chat_completion(br#"{"choices":[{"message":{"content":"  "}}]}"#).unwrap_err();
        assert_eq!(whitespace_llm.code(), "LLM_RESPONSE_EMPTY");

        let odd = parse_tts_pcm(&[0x00]).unwrap_err();
        assert_eq!(odd.code(), "TTS_PCM_INVALID");
        assert!(!odd.to_string().contains('\0'));
    }

    #[test]
    fn rejects_json_and_tts_bodies_over_caps() {
        assert!(!json_body_too_large(1024 * 1024));
        assert!(json_body_too_large(1024 * 1024 + 1));
        assert!(!tts_body_too_large(8 * 1024 * 1024));
        assert!(tts_body_too_large(8 * 1024 * 1024 + 1));
    }

    #[test]
    fn asr_multipart_contains_model_and_wav_without_credential() {
        let (body, content_type) =
            build_asr_multipart("qwen-asr", b"RIFFaudio", "voice-route-test");
        assert_eq!(
            content_type,
            "multipart/form-data; boundary=voice-route-test"
        );
        assert!(
            body.windows(b"qwen-asr".len())
                .any(|part| part == b"qwen-asr")
        );
        assert!(
            body.windows(b"RIFFaudio".len())
                .any(|part| part == b"RIFFaudio")
        );
        assert!(
            body.windows(b"audio.wav".len())
                .any(|part| part == b"audio.wav")
        );
        assert!(!body.windows(b"secret".len()).any(|part| part == b"secret"));
    }

    #[test]
    fn llm_and_tts_request_bodies_match_protocol() {
        let messages = [ChatMessage {
            role: "user".into(),
            content: "候选人回答".into(),
        }];
        assert_eq!(
            build_llm_request("qwen-plus", &messages),
            serde_json::json!({
                "model": "qwen-plus",
                "messages": [{"role": "user", "content": "候选人回答"}],
                "temperature": 0.3,
            })
        );
        assert_eq!(
            build_tts_request("qwen-tts", "Cherry", "你好"),
            serde_json::json!({
                "model": "qwen-tts",
                "input": "你好",
                "voice": "Cherry",
                "response_format": "pcm",
            })
        );
    }

    struct EmptyCascade;

    impl SpeechToText for EmptyCascade {
        fn transcribe(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[u8],
            _: u32,
        ) -> Result<String, CascadeError> {
            parse_transcript(br#"{"text":""}"#)
        }
    }

    impl ChatModel for EmptyCascade {
        fn complete(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[ChatMessage],
        ) -> Result<String, CascadeError> {
            parse_chat_completion(br#"{"choices":[{"message":{"content":""}}]}"#)
        }
    }

    impl TextToSpeech for EmptyCascade {
        fn synthesize(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, CascadeError> {
            parse_tts_pcm(&[0x01])
        }
    }

    #[test]
    fn failing_double_rejects_empty_transcript_llm_and_odd_pcm() {
        let endpoint = ProviderEndpoint {
            provider_id: "provider-1".into(),
            base_url: "https://example.test/v1".into(),
        };
        assert_eq!(
            EmptyCascade
                .transcribe(&endpoint, None, "asr", &[], 16_000)
                .unwrap_err()
                .code(),
            "ASR_RESPONSE_INVALID"
        );
        assert_eq!(
            EmptyCascade
                .complete(&endpoint, None, "llm", &[])
                .unwrap_err()
                .code(),
            "LLM_RESPONSE_EMPTY"
        );
        assert_eq!(
            EmptyCascade
                .synthesize(&endpoint, None, "tts", "alloy", "hi")
                .unwrap_err()
                .code(),
            "TTS_PCM_INVALID"
        );
        let _ = OpenAiCompatibleCascade::new();
    }
}

mod cascade_http {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver},
        thread,
        time::Duration,
    };

    use super::super::{
        ChatMessage, ChatModel, OpenAiCompatibleCascade, ProviderEndpoint, SpeechToText,
        TextToSpeech,
    };

    struct CapturedRequest {
        request_line: String,
        headers: String,
        body: Vec<u8>,
    }

    fn serve_once(response: Vec<u8>) -> (String, Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            stream.write_all(&response).unwrap();
            let _ = sender.send(request);
        });
        (format!("http://{address}/v1"), receiver)
    }

    fn read_request(stream: &mut TcpStream) -> CapturedRequest {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "request ended before headers completed");
            received.extend_from_slice(&buffer[..count]);
            if let Some(position) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8(received[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while received.len() - header_end < content_length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "request ended before body completed");
            received.extend_from_slice(&buffer[..count]);
        }
        CapturedRequest {
            request_line: headers.lines().next().unwrap().to_owned(),
            headers,
            body: received[header_end..header_end + content_length].to_vec(),
        }
    }

    fn response(status: &str, body: &[u8], content_type: &str, extra_headers: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\n{extra_headers}Connection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect()
    }

    fn endpoint(base_url: String) -> ProviderEndpoint {
        ProviderEndpoint {
            provider_id: "provider-1".into(),
            base_url,
        }
    }

    #[test]
    fn native_search_uses_selected_protocol_and_parses_sources() {
        use super::super::web_search::WebCapability;
        for capability in [
            WebCapability::OpenaiResponsesWebSearch,
            WebCapability::QwenResponsesWebSearch,
            WebCapability::QwenChatEnableSearch,
        ] {
            let is_chat = capability == WebCapability::QwenChatEnableSearch;
            let body = if is_chat {
                serde_json::json!({ "choices": [{ "message": { "content": "answer" } }], "search_info": { "search_results": [{ "url": "https://example.com/news", "title": "News" }] } })
            } else {
                serde_json::json!({ "status": "completed", "output": [{ "type": "message", "content": [{ "type": "output_text", "text": "answer" }] }, { "type": "web_search_call", "action": { "sources": [{ "url": "https://example.com/news", "title": "News" }] } }] })
            };
            let (url, captured) = serve_once(response(
                "200 OK",
                &serde_json::to_vec(&body).unwrap(),
                "application/json",
                "",
            ));
            let adapter = OpenAiCompatibleCascade::new()
                .unwrap()
                .with_web_capability(capability);
            assert_eq!(
                adapter
                    .complete(&endpoint(url), Some("synthetic"), "chosen-model", &[])
                    .unwrap(),
                "answer"
            );
            let request = captured.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(request.request_line.contains(if is_chat {
                "/v1/chat/completions"
            } else {
                "/v1/responses"
            }));
            let payload: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(payload["model"], "chosen-model");
            if is_chat {
                assert_eq!(payload["enable_search"], true);
            } else {
                assert_eq!(payload["tools"][0]["type"], "web_search");
            }
            assert_eq!(adapter.web_result().sources.len(), 1);
            assert!(!adapter.web_result().degraded);
        }
    }

    #[test]
    fn failed_search_falls_back_without_search_tools_or_fake_sources() {
        use super::super::web_search::WebCapability;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            assert!(read_request(&mut first).request_line.contains("/responses"));
            first
                .write_all(&response("400 Bad Request", b"{}", "application/json", ""))
                .unwrap();
            drop(first);
            let (mut second, _) = listener.accept().unwrap();
            let request = read_request(&mut second);
            let payload: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert!(payload.get("tools").is_none());
            assert!(payload.get("enable_search").is_none());
            second
                .write_all(&response(
                    "200 OK",
                    br#"{"choices":[{"message":{"content":"ordinary answer"}}]}"#,
                    "application/json",
                    "",
                ))
                .unwrap();
        });
        let adapter = OpenAiCompatibleCascade::new()
            .unwrap()
            .with_web_capability(WebCapability::OpenaiResponsesWebSearch);
        assert_eq!(
            adapter
                .complete(&endpoint(url), None, "model", &[])
                .unwrap(),
            "ordinary answer"
        );
        assert!(adapter.web_result().degraded);
        assert!(adapter.web_result().sources.is_empty());
        worker.join().unwrap();
    }

    #[test]
    fn transcribes_wav_multipart_and_returns_text() {
        let body = r#"{"text":"你好候选人"}"#.as_bytes();
        let (base_url, captured) = serve_once(response("200 OK", body, "application/json", ""));
        let adapter = OpenAiCompatibleCascade::new().unwrap();

        let text = adapter
            .transcribe(
                &endpoint(base_url),
                Some("synthetic-credential-marker"),
                "qwen-asr",
                &[0x11, 0x22],
                16_000,
            )
            .unwrap();

        assert_eq!(text, "你好候选人");
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            captured.request_line,
            "POST /v1/audio/transcriptions HTTP/1.1"
        );
        let headers = captured.headers.to_ascii_lowercase();
        assert!(headers.contains("authorization: bearer synthetic-credential-marker\r\n"));
        assert!(headers.contains("multipart/form-data; boundary="));
        assert!(
            captured
                .body
                .windows(b"qwen-asr".len())
                .any(|part| part == b"qwen-asr")
        );
        assert!(
            captured
                .body
                .windows(b"RIFF".len())
                .any(|part| part == b"RIFF")
        );
        assert!(
            !captured
                .body
                .windows(b"secret".len())
                .any(|part| part == b"secret")
        );
    }

    #[test]
    fn completes_chat_with_temperature_and_optional_auth() {
        let body = r#"{"choices":[{"message":{"content":"下一题"}}]}"#.as_bytes();
        let (base_url, captured) = serve_once(response("200 OK", body, "application/json", ""));
        let adapter = OpenAiCompatibleCascade::new().unwrap();
        let messages = [ChatMessage {
            role: "user".into(),
            content: "候选人回答".into(),
        }];

        let reply = adapter
            .complete(&endpoint(base_url), None, "qwen-plus", &messages)
            .unwrap();

        assert_eq!(reply, "下一题");
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(captured.request_line, "POST /v1/chat/completions HTTP/1.1");
        assert!(
            !captured
                .headers
                .to_ascii_lowercase()
                .contains("authorization:")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&captured.body).unwrap(),
            serde_json::json!({
                "model": "qwen-plus",
                "messages": [{"role": "user", "content": "候选人回答"}],
                "temperature": 0.3,
            })
        );
    }

    #[test]
    fn synthesizes_even_pcm_and_rejects_unauthorized_without_body() {
        let pcm = [0x00_u8, 0x01, 0x02, 0x03];
        let (base_url, captured) =
            serve_once(response("200 OK", &pcm, "application/octet-stream", ""));
        let adapter = OpenAiCompatibleCascade::new().unwrap();

        let out = adapter
            .synthesize(&endpoint(base_url), None, "qwen-tts", "Cherry", "你好")
            .unwrap();
        assert_eq!(out, pcm);
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(captured.request_line, "POST /v1/audio/speech HTTP/1.1");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&captured.body).unwrap(),
            serde_json::json!({
                "model": "qwen-tts",
                "input": "你好",
                "voice": "Cherry",
                "response_format": "pcm",
            })
        );

        let marker = br#"{"message":"synthetic-upstream-marker"}"#;
        let (base_url, _) =
            serve_once(response("401 Unauthorized", marker, "application/json", ""));
        let error = adapter
            .transcribe(&endpoint(base_url), None, "qwen-asr", &[0x00, 0x01], 16_000)
            .unwrap_err();
        assert_eq!(error.code(), "ASR_UNAUTHORIZED");
        assert!(!error.to_string().contains("synthetic-upstream-marker"));
    }

    #[test]
    fn rejects_redirect_oversize_json_and_odd_tts_pcm() {
        let adapter = OpenAiCompatibleCascade::new().unwrap();
        let (base_url, _) = serve_once(response(
            "302 Found",
            b"",
            "application/json",
            "Location: http://127.0.0.1:9/capture\r\n",
        ));
        assert_eq!(
            adapter
                .complete(&endpoint(base_url), None, "qwen-plus", &[])
                .unwrap_err()
                .code(),
            "LLM_REQUEST_FAILED"
        );

        let (base_url, _) = serve_once(response(
            "200 OK",
            &vec![b'x'; 1024 * 1024 + 1],
            "application/json",
            "",
        ));
        assert_eq!(
            adapter
                .complete(&endpoint(base_url), None, "qwen-plus", &[])
                .unwrap_err()
                .code(),
            "LLM_RESPONSE_TOO_LARGE"
        );

        let (base_url, _) = serve_once(response("200 OK", &[0x00], "application/octet-stream", ""));
        assert_eq!(
            adapter
                .synthesize(&endpoint(base_url), None, "qwen-tts", "alloy", "hi")
                .unwrap_err()
                .code(),
            "TTS_PCM_INVALID"
        );
    }

    #[test]
    fn rejects_tts_response_over_eight_mebibytes() {
        let adapter = OpenAiCompatibleCascade::new().unwrap();
        let (base_url, _) = serve_once(response(
            "200 OK",
            &vec![0_u8; 8 * 1024 * 1024 + 1],
            "application/octet-stream",
            "",
        ));
        assert_eq!(
            adapter
                .synthesize(&endpoint(base_url), None, "qwen-tts", "alloy", "hi")
                .unwrap_err()
                .code(),
            "TTS_RESPONSE_TOO_LARGE"
        );
    }
}

#[test]
fn normalizes_openai_compatible_models_url() {
    assert_eq!(
        normalize_models_url("https://example.test/v1")
            .unwrap()
            .as_str(),
        "https://example.test/v1/models"
    );
    assert_eq!(
        normalize_models_url("https://example.test/v1/")
            .unwrap()
            .as_str(),
        "https://example.test/v1/models"
    );
    assert!(normalize_models_url("ftp://example.test/v1").is_err());
}

#[test]
fn parses_deduplicated_sorted_model_ids() {
    let models =
        parse_model_catalog(br#"{"data":[{"id":"zeta"},{"id":"alpha"},{"id":"alpha"},{"id":""}]}"#)
            .unwrap();
    assert_eq!(
        models.into_iter().map(|model| model.id).collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
}

#[test]
fn merges_builtin_catalog_for_zhipu_models_endpoint() {
    let mut models =
        parse_model_catalog(br#"{"data":[{"id":"glm-5"},{"id":"glm-4.5"}]}"#).unwrap();
    let url = normalize_models_url("https://open.bigmodel.cn/api/paas/v4").unwrap();
    merge_builtin_catalog(&url, &mut models);
    let ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "embedding-2",
            "embedding-3",
            "glm-4.5",
            "glm-5",
            "glm-asr-2512",
            "glm-realtime",
            "glm-realtime-air",
            "glm-realtime-flash",
            "glm-tts",
        ]
    );
}

#[test]
fn builtin_catalog_dedupes_against_discovered_ids() {
    let mut models = parse_model_catalog(br#"{"data":[{"id":"glm-tts"}]}"#).unwrap();
    let url = normalize_models_url("https://open.bigmodel.cn/api/paas/v4").unwrap();
    merge_builtin_catalog(&url, &mut models);
    let ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    assert_eq!(ids.iter().filter(|id| *id == "glm-tts").count(), 1);
}

#[test]
fn builtin_catalog_skips_unknown_hosts() {
    let mut models = parse_model_catalog(br#"{"data":[{"id":"gpt-4o"}]}"#).unwrap();
    let url = normalize_models_url("https://api.openai.com/v1").unwrap();
    merge_builtin_catalog(&url, &mut models);
    let ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    assert_eq!(ids, vec!["gpt-4o"]);
}

#[test]
fn malformed_catalog_has_stable_error_without_body() {
    let error = parse_model_catalog(br#"{"upstreamSecret":"must-not-escape"}"#).unwrap_err();
    assert_eq!(error.code(), "PROVIDER_RESPONSE_INVALID");
    assert!(!error.to_string().contains("must-not-escape"));
}

mod embedding {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver},
        thread,
        time::Duration,
    };

    use super::super::{EmbeddingProbe, OpenAiCompatibleEmbeddingProbe, ProviderEndpoint};

    const TEST_INPUT: &str = "AI Virtual Assistant embedding connectivity test";

    struct CapturedRequest {
        request_line: String,
        headers: String,
        body: Vec<u8>,
    }

    fn serve_once(response: Vec<u8>) -> (String, Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            stream.write_all(&response).unwrap();
            let _ = sender.send(request);
        });
        (format!("http://{address}/v1"), receiver)
    }

    fn read_request(stream: &mut TcpStream) -> CapturedRequest {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "request ended before headers completed");
            received.extend_from_slice(&buffer[..count]);
            if let Some(position) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8(received[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while received.len() - header_end < content_length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "request ended before body completed");
            received.extend_from_slice(&buffer[..count]);
        }
        CapturedRequest {
            request_line: headers.lines().next().unwrap().to_owned(),
            headers,
            body: received[header_end..header_end + content_length].to_vec(),
        }
    }

    fn response(status: &str, body: &[u8], extra_headers: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n{extra_headers}Connection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect()
    }

    fn endpoint(base_url: String) -> ProviderEndpoint {
        ProviderEndpoint {
            provider_id: "provider-1".into(),
            base_url,
        }
    }

    #[test]
    fn posts_exact_embedding_request_with_optional_bearer_auth_and_returns_floats() {
        let body = br#"{"data":[{"embedding":[0.25,-1.5,3.0]}]}"#;
        let (base_url, captured) = serve_once(response("200 OK", body, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let vector = probe
            .embed(
                &endpoint(base_url),
                Some("synthetic-credential-marker"),
                "embed-model",
                3,
                TEST_INPUT,
            )
            .unwrap();

        assert_eq!(vector, vec![0.25, -1.5, 3.0]);
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(captured.request_line, "POST /v1/embeddings HTTP/1.1");
        assert!(
            captured
                .headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-credential-marker\r\n")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&captured.body).unwrap(),
            serde_json::json!({
                "input": "AI Virtual Assistant embedding connectivity test",
                "model": "embed-model",
                "dimensions": 3,
                "encoding_format": "float",
            })
        );
    }

    #[test]
    fn omits_authorization_for_configured_unauthenticated_endpoint() {
        let body = br#"{"data":[{"embedding":[1.0]}]}"#;
        let (base_url, captured) = serve_once(response("200 OK", body, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        assert_eq!(
            probe
                .embed(&endpoint(base_url), None, "local-model", 1, TEST_INPUT)
                .unwrap(),
            vec![1.0]
        );
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            !captured
                .headers
                .to_ascii_lowercase()
                .contains("authorization:")
        );
    }

    #[test]
    fn rejects_unauthorized_embedding_response_with_stable_code() {
        let marker = br#"{"message":"synthetic-upstream-marker"}"#;
        let (base_url, _) = serve_once(response("401 Unauthorized", marker, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(&endpoint(base_url), None, "embed-model", 3, TEST_INPUT)
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_UNAUTHORIZED");
        assert!(!error.to_string().contains("synthetic-upstream-marker"));
    }

    #[test]
    fn times_out_a_slow_embedding_response_with_stable_code() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            thread::sleep(Duration::from_secs(11));
        });
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(
                &endpoint(format!("http://{address}/v1")),
                None,
                "embed-model",
                3,
                TEST_INPUT,
            )
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_TIMEOUT");
    }

    #[test]
    fn rejects_redirect_without_following_it() {
        let (base_url, _) = serve_once(response(
            "302 Found",
            b"",
            "Location: http://127.0.0.1:9/capture\r\n",
        ));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(&endpoint(base_url), None, "embed-model", 3, TEST_INPUT)
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_REQUEST_FAILED");
    }

    #[test]
    fn rejects_embedding_response_over_one_mebibyte() {
        let body = vec![b'x'; 1024 * 1024 + 1];
        let (base_url, _) = serve_once(response("200 OK", &body, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(&endpoint(base_url), None, "embed-model", 3, TEST_INPUT)
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_RESPONSE_TOO_LARGE");
    }

    #[test]
    fn rejects_malformed_embedding_json_without_exposing_response() {
        let marker = br#"{"syntheticResponseMarker":"must-not-escape"}"#;
        let (base_url, _) = serve_once(response("200 OK", marker, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(&endpoint(base_url), None, "embed-model", 3, TEST_INPUT)
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_RESPONSE_INVALID");
        assert!(!error.to_string().contains("must-not-escape"));
    }

    #[test]
    fn rejects_values_that_overflow_f32() {
        let body = br#"{"data":[{"embedding":[3.5e38]}]}"#;
        let (base_url, _) = serve_once(response("200 OK", body, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(&endpoint(base_url), None, "embed-model", 1, TEST_INPUT)
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_NON_FINITE_VALUE");
    }

    #[test]
    fn rejects_embedding_vector_with_wrong_dimension() {
        let body = br#"{"data":[{"embedding":[1.0,2.0]}]}"#;
        let (base_url, _) = serve_once(response("200 OK", body, ""));
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();

        let error = probe
            .embed(&endpoint(base_url), None, "embed-model", 3, TEST_INPUT)
            .unwrap_err();

        assert_eq!(error.code(), "EMBEDDING_DIMENSION_MISMATCH");
    }

    #[test]
    fn rejects_embedding_endpoints_with_userinfo_query_or_fragment() {
        let probe = OpenAiCompatibleEmbeddingProbe::new().unwrap();
        for base_url in [
            "https://user@example.test/v1",
            "https://example.test/v1?marker=synthetic",
            "https://example.test/v1#synthetic",
        ] {
            assert_eq!(
                probe
                    .embed(
                        &endpoint(base_url.into()),
                        None,
                        "embed-model",
                        3,
                        TEST_INPUT,
                    )
                    .unwrap_err()
                    .code(),
                "EMBEDDING_ENDPOINT_INVALID"
            );
        }
    }
}

mod openai_realtime {
    use std::{collections::VecDeque, time::Duration};

    use serde_json::json;

    use super::super::{
        InputTranscriptAssembler, RealtimeDialectName, RealtimeError, RealtimeTransport,
        dialect_input_rate, realtime_dialect, realtime_url, session_update_event,
        wait_session_updated,
    };

    struct FakeSocket {
        inbound: VecDeque<Result<String, RealtimeError>>,
    }

    impl RealtimeTransport for FakeSocket {
        fn recv_text(&mut self, _timeout: Duration) -> Result<String, RealtimeError> {
            self.inbound
                .pop_front()
                .unwrap_or(Err(RealtimeError::SessionUpdateTimeout))
        }
    }

    #[test]
    fn maps_dashscope_compatible_mode_and_openai_dialects() {
        let dashscope = realtime_dialect("https://dashscope.aliyuncs.com/v1");
        assert_eq!(dashscope.name, RealtimeDialectName::Aliyun);
        assert_eq!(dashscope.audio_format, "pcm");
        assert_eq!(dashscope.default_voice, None);

        let compatible = realtime_dialect("https://gateway.example/compatible-mode/v1");
        assert_eq!(compatible.name, RealtimeDialectName::Aliyun);
        assert_eq!(compatible.audio_format, "pcm");
        assert_eq!(compatible.default_voice, None);

        let openai = realtime_dialect("https://api.openai.com/v1");
        assert_eq!(openai.name, RealtimeDialectName::Openai);
        assert_eq!(openai.audio_format, "pcm16");
        assert_eq!(openai.default_voice, Some("alloy"));
    }

    #[test]
    fn maps_bigmodel_to_16k_input_dialect() {
        // GLM 的 "pcm16" 表示 16kHz（与 OpenAI 的 24kHz 语义不同），VAD 也按 16kHz 解码；
        // 若按 OpenAI 方言重采样到 24kHz，服务端会听到变调音频导致转写失败。
        // 输出按官方文档只支持 "pcm"（固定 24kHz），音色用 GLM 自己的默认值。
        let glm = realtime_dialect("https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(glm.name, RealtimeDialectName::Bigmodel);
        assert_eq!(glm.audio_format, "pcm16");
        assert_eq!(dialect_input_rate(&glm), 16_000);
        assert_eq!(glm.output_audio_format, "pcm");
        assert_eq!(glm.default_voice, Some("tongtong"));
    }

    #[test]
    fn maps_http_and_https_realtime_urls() {
        assert_eq!(
            realtime_url("http://example.test/v1", "gpt-4o-realtime-preview")
                .unwrap()
                .as_str(),
            "ws://example.test/v1/realtime?model=gpt-4o-realtime-preview"
        );
        assert_eq!(
            realtime_url(
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
                "qwen-audio-3.0-realtime-plus"
            )
            .unwrap()
            .as_str(),
            "wss://dashscope.aliyuncs.com/api-ws/v1/realtime?model=qwen-audio-3.0-realtime-plus"
        );
        assert_eq!(
            realtime_url(
                "https://token-plan.cn-beijing.maas.aliyuncs.com/v1",
                "qwen-audio"
            )
            .unwrap()
            .as_str(),
            "wss://token-plan.cn-beijing.maas.aliyuncs.com/api-ws/v1/realtime?model=qwen-audio"
        );
        assert_eq!(
            realtime_url("wss://example.com/api-ws/v1/realtime", "qwen-audio")
                .unwrap()
                .as_str(),
            "wss://example.com/api-ws/v1/realtime?model=qwen-audio"
        );
    }

    #[test]
    fn rejects_ftp_userinfo_query_and_fragment_urls() {
        for base_url in [
            "ftp://example.test/v1",
            "https://user@example.test/v1",
            "https://example.test/v1?marker=synthetic",
            "https://example.test/v1#synthetic",
        ] {
            let error = realtime_url(base_url, "model").unwrap_err();
            assert_eq!(error.code(), "REALTIME_URL_INVALID");
            assert!(!error.to_string().contains("synthetic"));
            assert!(!error.to_string().contains("user"));
        }
    }

    #[test]
    fn assembler_streams_deltas_then_final_and_ignores_late_partial() {
        let mut assembler = InputTranscriptAssembler::new();
        assert_eq!(
            assembler.update(
                "input_transcript_delta",
                &json!({"item_id": "qwen-1", "text": "今天", "stash": "天气", "delta": ""})
            ),
            Some(("qwen-1".into(), "今天天气".into(), false))
        );
        assert_eq!(
            assembler.update(
                "input_transcript_delta",
                &json!({"item_id": "openai-1", "text": "", "stash": "", "delta": "你"})
            ),
            Some(("openai-1".into(), "你".into(), false))
        );
        assert_eq!(
            assembler.update(
                "input_transcript_delta",
                &json!({"item_id": "openai-1", "text": "", "stash": "", "delta": "好"})
            ),
            Some(("openai-1".into(), "你好".into(), false))
        );
        assert_eq!(
            assembler.update(
                "input_transcript_completed",
                &json!({"item_id": "openai-1", "transcript": "你好。"})
            ),
            Some(("openai-1".into(), "你好。".into(), true))
        );
        assembler.update(
            "input_transcript_completed",
            &json!({"item_id": "item-1", "transcript": "最终"}),
        );
        assert_eq!(
            assembler.update(
                "input_transcript_delta",
                &json!({"item_id": "item-1", "text": "旧", "stash": "", "delta": ""})
            ),
            None
        );
    }

    #[test]
    fn handshake_waits_for_session_updated_and_times_out() {
        let mut socket = FakeSocket {
            inbound: VecDeque::from([
                Ok(r#"{"type":"session.created"}"#.into()),
                Ok(r#"{"type":"session.updated"}"#.into()),
            ]),
        };
        wait_session_updated(&mut socket, Duration::from_secs(1)).unwrap();

        let mut silent = FakeSocket {
            inbound: VecDeque::new(),
        };
        let error = wait_session_updated(&mut silent, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.code(), "REALTIME_SESSION_UPDATE_TIMEOUT");
    }

    #[test]
    fn handshake_sanitizes_remote_error_codes() {
        let mut socket = FakeSocket {
            inbound: VecDeque::from([Ok(json!({
                "type": "error",
                "error": {
                    "code": "invalid_value",
                    "message": "bad field sk-secret-must-not-escape"
                }
            })
            .to_string())]),
        };
        let error = wait_session_updated(&mut socket, Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.code(), "invalid_value");
        assert!(!error.to_string().contains("sk-secret-must-not-escape"));
        assert!(!error.to_string().contains("bad field"));
    }

    #[test]
    fn session_update_uses_dialect_audio_and_voice_defaults() {
        let aliyun = session_update_event(
            "",
            "中文回复",
            &realtime_dialect("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        );
        let openai = session_update_event(
            "",
            "Reply briefly",
            &realtime_dialect("https://api.openai.com/v1"),
        );
        let custom = session_update_event(
            "custom-voice",
            "Reply briefly",
            &realtime_dialect("https://api.openai.com/v1"),
        );
        assert_eq!(aliyun["session"]["input_audio_format"], "pcm");
        assert_eq!(aliyun["session"]["output_audio_format"], "pcm");
        assert!(aliyun["session"].get("voice").is_none());
        assert_eq!(openai["session"]["input_audio_format"], "pcm16");
        assert_eq!(openai["session"]["voice"], "alloy");
        assert_eq!(custom["session"]["voice"], "custom-voice");
        assert!(aliyun["session"].get("input_audio_transcription").is_none());
        assert_eq!(aliyun["type"], "session.update");
        assert_eq!(
            dialect_input_rate(&realtime_dialect("https://api.openai.com/v1")),
            24_000
        );
        assert_eq!(
            dialect_input_rate(&realtime_dialect(
                "https://dashscope.aliyuncs.com/compatible-mode/v1"
            )),
            16_000
        );
    }
}

mod mock_http {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver},
        thread,
        time::Duration,
    };

    use super::super::ProviderEndpoint;

    pub struct CapturedRequest {
        pub request_line: String,
        pub headers: String,
        pub body: Vec<u8>,
    }

    pub fn serve_once(response: Vec<u8>) -> (String, Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            stream.write_all(&response).unwrap();
            let _ = sender.send(request);
        });
        (format!("http://{address}/v1"), receiver)
    }

    fn read_request(stream: &mut TcpStream) -> CapturedRequest {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "request ended before headers completed");
            received.extend_from_slice(&buffer[..count]);
            if let Some(position) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8(received[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while received.len() - header_end < content_length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "request ended before body completed");
            received.extend_from_slice(&buffer[..count]);
        }
        CapturedRequest {
            request_line: headers.lines().next().unwrap().to_owned(),
            headers,
            body: received[header_end..].to_vec(),
        }
    }

    pub fn response(status: &str, body: &[u8], extra_headers: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n{extra_headers}Connection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect()
    }

    pub fn endpoint(base_url: String) -> ProviderEndpoint {
        ProviderEndpoint {
            provider_id: "provider-1".into(),
            base_url,
        }
    }
}

mod voice_clone {
    use std::time::Duration;

    use super::super::VoiceCloneProbe;
    use super::mock_http::{endpoint, response, serve_once};

    const SAMPLE: &[u8] = b"RIFFsynthetic-voice-sample-bytes";

    #[test]
    fn uploads_sample_as_multipart_with_purpose_and_returns_file_id() {
        let (base_url, captured) = serve_once(response("200 OK", br#"{"id":"file_test_001"}"#, ""));
        let probe = VoiceCloneProbe::new().unwrap();

        let file_id = probe
            .upload_sample(
                &endpoint(base_url),
                Some("key-marker"),
                "sample.wav",
                "audio/wav",
                SAMPLE.to_vec(),
            )
            .unwrap();

        assert_eq!(file_id, "file_test_001");
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(captured.request_line, "POST /v1/files HTTP/1.1");
        let headers = captured.headers.to_ascii_lowercase();
        assert!(headers.contains("authorization: bearer key-marker\r\n"));
        assert!(headers.contains("content-type: multipart/form-data; boundary="));
        assert!(contains_bytes(&captured.body, b"voice-clone-input"));
        assert!(contains_bytes(&captured.body, SAMPLE));
    }

    #[test]
    fn clone_voice_posts_required_fields_with_transcript_and_returns_voice_id() {
        let body = br#"{"voice":"voice_clone_abc","file_id":"file_out","file_purpose":"voice-clone-output","request_id":"req-1"}"#;
        let (base_url, captured) = serve_once(response("200 OK", body, ""));
        let probe = VoiceCloneProbe::new().unwrap();

        let voice = probe
            .clone_voice(
                &endpoint(base_url),
                None,
                "roleai_abc123def",
                "你好，参考文字。",
                "file_in_123",
            )
            .unwrap();

        assert_eq!(voice, "voice_clone_abc");
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(captured.request_line, "POST /v1/voice/clone HTTP/1.1");
        assert!(!captured.headers.to_ascii_lowercase().contains("authorization:"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&captured.body).unwrap(),
            serde_json::json!({
                "model": "glm-tts-clone",
                "voice_name": "roleai_abc123def",
                "input": "你好，这是一段用于确认克隆音色效果的试听文本。",
                "file_id": "file_in_123",
                "text": "你好，参考文字。",
            })
        );
    }

    #[test]
    fn clone_voice_omits_blank_transcript_and_maps_unauthorized() {
        let marker = br#"{"message":"synthetic-upstream-marker"}"#;
        let (base_url, captured) = serve_once(response("401 Unauthorized", marker, ""));
        let probe = VoiceCloneProbe::new().unwrap();

        let error = probe
            .clone_voice(&endpoint(base_url), None, "roleai_abc123def", "   ", "file_in_123")
            .unwrap_err();

        assert_eq!(error.code(), "PROVIDER_UNAUTHORIZED");
        // 只透出供应商错误消息字段本身（限长），不是原始响应体。
        assert_eq!(
            error.provider_message.as_deref(),
            Some("synthetic-upstream-marker")
        );
        assert!(!error.to_string().contains("{\"message\""));
        let captured = captured.recv_timeout(Duration::from_secs(2)).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
        assert!(payload.get("text").is_none());
    }

    #[test]
    fn rejects_malformed_clone_response_without_exposing_it() {
        let marker = br#"{"syntheticResponseMarker":"must-not-escape"}"#;
        let (base_url, _) = serve_once(response("200 OK", marker, ""));
        let probe = VoiceCloneProbe::new().unwrap();

        let error = probe
            .clone_voice(&endpoint(base_url), None, "roleai_abc123def", "", "file_in_123")
            .unwrap_err();

        assert_eq!(error.code(), "PROVIDER_RESPONSE_INVALID");
        assert!(!error.to_string().contains("must-not-escape"));
    }

    #[test]
    fn rejects_oversize_upload_response_body() {
        let body = vec![b'x'; 1024 * 1024 + 1];
        let (base_url, _) = serve_once(response("200 OK", &body, ""));
        let probe = VoiceCloneProbe::new().unwrap();

        let error = probe
            .upload_sample(
                &endpoint(base_url),
                None,
                "sample.wav",
                "audio/wav",
                SAMPLE.to_vec(),
            )
            .unwrap_err();

        assert_eq!(error.code(), "PROVIDER_RESPONSE_TOO_LARGE");
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|part| part == needle)
    }
}
