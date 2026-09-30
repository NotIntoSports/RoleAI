use std::{fs, path::PathBuf};

use serde_json::Value;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Explicit allow-list shape for a capability permission entry:
/// `^(core:default|allow-[a-z0-9-]+|<枚举插件>:<permission>)$`。
/// 插件命名空间逐一枚举（G07 引入 updater:default）；新增插件权限必须显式
/// 更新本契约，与安全面基线同一提交。无 regex 依赖。
fn is_plugin_permission(permission: &str) -> bool {
    // 官方 Tauri 插件权限形如 `<plugin>:<permission>`；只接受枚举过的插件名。
    match permission.split_once(':') {
        Some((plugin, rest)) => {
            matches!(plugin, "updater")
                && !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }
        None => false,
    }
}

fn matches_permission_shape(permission: &str) -> bool {
    if permission == "core:default" || is_plugin_permission(permission) {
        return true;
    }
    match permission.strip_prefix("allow-") {
        Some(rest) => {
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }
        None => false,
    }
}

#[test]
fn capability_is_explicit_and_has_no_generic_process_or_filesystem_access() {
    let text = fs::read_to_string(manifest_dir().join("capabilities/main.json")).unwrap();
    let capability: Value = serde_json::from_str(&text).unwrap();
    // (5) window scope pinned to exactly ["main"].
    assert_eq!(capability["windows"], serde_json::json!(["main"]));

    let permissions = capability["permissions"]
        .as_array()
        .expect("capability.permissions must be an array");
    let granted: Vec<&str> = permissions
        .iter()
        .map(|value| value.as_str().expect("permission must be a string"))
        .collect();

    // Phase 0-1 accepted baseline. Phase 3+ may ADD command permissions but must
    // never lose these, so assert containment (superset) rather than equality.
    let baseline = [
        "core:default",
        "allow-foundation-get-status",
        "allow-diagnostics-export",
        "allow-config-get-startup-state",
        "allow-config-restore-last-good",
        "allow-config-restore-defaults",
        "allow-open-app-directory",
    ];
    // (1) always includes core:default.
    assert!(
        granted.contains(&"core:default"),
        "capability must include core:default"
    );
    // (3) superset of the Phase 0-1 baseline.
    for required in baseline {
        assert!(
            granted.contains(&required),
            "capability must retain baseline permission: {required}"
        );
    }
    // (2) every entry matches the explicit allow-list shape.
    for permission in &granted {
        assert!(
            matches_permission_shape(permission),
            "unexpected capability permission shape: {permission}"
        );
    }
    // (4) no duplicate grants.
    let unique: std::collections::HashSet<&str> = granted.iter().copied().collect();
    assert_eq!(
        unique.len(),
        granted.len(),
        "capability.permissions must not contain duplicates"
    );

    let lower = text.to_ascii_lowercase();
    for forbidden in [
        "shell:allow-execute",
        "shell:allow-spawn",
        "fs:",
        "http://",
        "https://",
        "\"*\"",
    ] {
        assert!(
            !lower.contains(forbidden),
            "forbidden capability token: {forbidden}"
        );
    }
}

#[test]
fn csp_navigation_child_windows_and_single_instance_are_fail_closed() {
    let config: Value =
        serde_json::from_str(&fs::read_to_string(manifest_dir().join("tauri.conf.json")).unwrap())
            .unwrap();
    let csp = config["app"]["security"]["csp"].as_str().unwrap();
    assert!(!csp.contains("unsafe-eval"));
    assert!(!csp.contains("script-src http:"));
    assert!(!csp.contains("script-src https:"));
    assert!(!csp.contains("connect-src http:") && !csp.contains("connect-src https:"));

    let source = fs::read_to_string(manifest_dir().join("src/lib.rs")).unwrap();
    assert!(source.contains("tauri_plugin_single_instance::init"));
    assert!(source.contains("on_navigation(navigation_is_allowed)"));
    assert!(source.contains("on_new_window"));
    assert!(source.contains("NewWindowResponse::Deny"));
    assert!(source.contains("cfg!(debug_assertions)"));
}

#[test]
fn each_application_command_has_one_explicit_permission() {
    let lib = fs::read_to_string(manifest_dir().join("src/lib.rs")).unwrap();
    let handler = lib
        .split("tauri::generate_handler![")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("generate_handler list");
    let mut commands = handler
        .lines()
        .filter_map(|line| {
            line.trim()
                .trim_end_matches(',')
                .strip_prefix("commands::")
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    commands.sort();
    assert!(
        commands.windows(2).all(|pair| pair[0] != pair[1]),
        "commands must be registered once"
    );

    let permissions =
        fs::read_to_string(manifest_dir().join("permissions/application.toml")).unwrap();
    let mut allowed = Vec::new();
    for line in permissions.lines() {
        if let Some(rest) = line.trim().strip_prefix("commands.allow = [") {
            let name = rest
                .trim_start_matches('"')
                .trim_end_matches("\"]")
                .to_owned();
            allowed.push(name);
        }
    }
    allowed.sort();
    assert_eq!(
        commands, allowed,
        "each command must have exactly one permission stanza"
    );

    let capability: Value = serde_json::from_str(
        &fs::read_to_string(manifest_dir().join("capabilities/main.json")).unwrap(),
    )
    .unwrap();
    let granted = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|permission| *permission != "core:default")
        // 官方插件命名空间权限（如 updater:default）不属于命令映射，
        // 在下方按枚举清单单独校验。
        .filter(|permission| !is_plugin_permission(permission))
        .collect::<Vec<_>>();
    assert_eq!(granted.len(), allowed.len());
    for command in &commands {
        let identifier = format!("allow-{}", command.replace('_', "-"));
        assert!(
            granted.contains(&identifier.as_str()),
            "capability missing {identifier}"
        );
    }
    // 插件权限清单：新增插件权限必须先扩展 is_plugin_permission 的枚举。
    let plugin_expected = ["updater:default"];
    let all_granted = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    for expected in plugin_expected {
        assert!(
            all_granted.contains(&expected),
            "capability missing expected plugin permission {expected}"
        );
    }
}

#[test]
fn cascade_session_commands_are_registered_and_permitted() {
    let required = [
        "session_start",
        "session_stop",
        "session_set_mode",
        "session_export",
        "session_list",
        "session_get",
        "session_delete",
        "session_finalize_utterance",
        "session_agent_command",
        "runtime_get_status",
    ];
    let lib = fs::read_to_string(manifest_dir().join("src/lib.rs")).unwrap();
    let handler = lib
        .split("tauri::generate_handler![")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("generate_handler list");
    let permissions =
        fs::read_to_string(manifest_dir().join("permissions/application.toml")).unwrap();
    let capability: Value = serde_json::from_str(
        &fs::read_to_string(manifest_dir().join("capabilities/main.json")).unwrap(),
    )
    .unwrap();
    let granted = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    for command in required {
        assert!(
            handler.contains(&format!("commands::{command}")),
            "generate_handler missing {command}"
        );
        assert!(
            permissions.contains(&format!("commands.allow = [\"{command}\"]")),
            "permission missing for {command}"
        );
        let identifier = format!("allow-{}", command.replace('_', "-"));
        assert!(
            granted.contains(&identifier.as_str()),
            "capability missing {identifier}"
        );
    }
}
