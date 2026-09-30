use std::{
    ffi::{OsStr, OsString},
    fs,
    path::PathBuf,
};

use thiserror::Error;

use crate::app_state::AppPaths;

pub const ISOLATION_SUPPORT_MARKER: &str = "AI_VIRTUAL_ASSISTANT_ISOLATION_V1";

#[derive(Debug, Clone)]
pub struct IsolatedStartup {
    pub root: PathBuf,
    pub paths: AppPaths,
    pub webview_data_directory: PathBuf,
    pub secret_namespace: String,
}

#[derive(Debug, Error)]
pub enum StartupError {
    #[error("--isolated-root may only be specified once")]
    DuplicateRoot,
    #[error("--isolated-root requires a directory path")]
    MissingRoot,
    #[error("the isolated root must be an absolute path")]
    RelativeRoot,
    #[error("the isolated root must be an existing empty directory")]
    InvalidRoot,
    #[error("the isolated root must not be a symlink or reparse point")]
    ReparseRoot,
    #[error("AI_VIRTUAL_ASSISTANT_CONFIG cannot be used with --isolated-root")]
    ConfigOverride,
    #[error("external WEBVIEW2 environment overrides cannot be used with --isolated-root")]
    WebviewOverride,
}

pub fn is_isolation_support_check(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .any(|argument| argument == OsStr::new("--check-isolation-support"))
}

pub fn parse_isolated_startup(
    args: &[OsString],
    config_override: Option<&OsStr>,
) -> Result<Option<IsolatedStartup>, StartupError> {
    const FLAG: &str = "--isolated-root";
    let mut root: Option<PathBuf> = None;
    let mut arguments = args.iter().skip(1);
    while let Some(argument) = arguments.next() {
        // 等号单段形式（`--isolated-root=<path>`）：WebDriver 启动器（msedgedriver）
        // 只会原样保留这种形式，见 startup.rs 测试 isolated_startup_accepts_inline_equals_form_for_webdriver_launchers。
        let inline_value = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix(FLAG))
            .and_then(|rest| rest.strip_prefix('='));
        if let Some(value) = inline_value {
            if root.is_some() {
                return Err(StartupError::DuplicateRoot);
            }
            if value.is_empty() {
                return Err(StartupError::MissingRoot);
            }
            root = Some(PathBuf::from(value));
            continue;
        }
        if argument != OsStr::new(FLAG) {
            continue;
        }
        if root.is_some() {
            return Err(StartupError::DuplicateRoot);
        }
        root = Some(arguments.next().ok_or(StartupError::MissingRoot)?.into());
    }
    let Some(root) = root else {
        return Ok(None);
    };
    if config_override.is_some()
        || args
            .iter()
            .skip(1)
            .any(|argument| argument == OsStr::new("--config"))
    {
        return Err(StartupError::ConfigOverride);
    }
    if !root.is_absolute() {
        return Err(StartupError::RelativeRoot);
    }

    let metadata = fs::symlink_metadata(&root).map_err(|_| StartupError::InvalidRoot)?;
    if metadata.file_type().is_symlink() || metadata_is_reparse_point(&metadata) {
        return Err(StartupError::ReparseRoot);
    }
    if !metadata.is_dir()
        || fs::read_dir(&root)
            .map_err(|_| StartupError::InvalidRoot)?
            .next()
            .is_some()
    {
        return Err(StartupError::InvalidRoot);
    }
    let root = root.canonicalize().map_err(|_| StartupError::InvalidRoot)?;
    let paths = AppPaths {
        data_directory: root.join("data"),
        logs_directory: root.join("logs"),
        config_path: root.join("config").join("local.json"),
        legacy_search_roots: Vec::new(),
    };
    Ok(Some(IsolatedStartup {
        root: root.clone(),
        paths,
        webview_data_directory: root.join("webview"),
        secret_namespace: format!("isolated-{}", uuid::Uuid::new_v4().simple()),
    }))
}

/// WebDriver 自动化管线（tauri-driver → msedgedriver）的唯一自声明开关：
/// tauri-driver 启动 msedgedriver 时固定注入（tauri-driver 源码 webdriver.rs），
/// 正常用户与打包冒烟环境不会出现。出现时才允许官方驱动注入的两项 WebView2 覆盖。
fn declares_webview_automation(environment: &[(OsString, OsString)]) -> bool {
    environment.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("TAURI_WEBVIEW_AUTOMATION") && !value.is_empty()
    })
}

pub fn validate_isolated_webview_environment(
    isolated: &IsolatedStartup,
    environment: &[(OsString, OsString)],
) -> Result<(), StartupError> {
    let automation = declares_webview_automation(environment);
    for (key, value) in environment {
        let key = key.to_string_lossy().to_ascii_uppercase();
        if !key.starts_with("WEBVIEW2_") {
            continue;
        }
        if key == "WEBVIEW2_USER_DATA_FOLDER" {
            if equivalent_path_with_missing_leaf(
                &PathBuf::from(value),
                &isolated.webview_data_directory,
            ) {
                continue;
            }
            // msedgedriver 的自动化 profile（scoped_dir 临时目录）与隔离目录不同，
            // 只在显式自动化声明下放行；webview 数据仍落在启动方的临时目录里。
            if automation {
                continue;
            }
            return Err(StartupError::WebviewOverride);
        }
        // --enable-automation / --remote-debugging-port 等自动化浏览器参数，
        // 仅在显式自动化声明下放行；其余 WEBVIEW2_* 覆盖一律拒绝。
        if key == "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS" && automation {
            continue;
        }
        return Err(StartupError::WebviewOverride);
    }
    Ok(())
}

fn equivalent_path_with_missing_leaf(left: &std::path::Path, right: &std::path::Path) -> bool {
    fn normalize(path: &std::path::Path) -> Option<PathBuf> {
        if !path.is_absolute() {
            return None;
        }
        path.canonicalize()
            .ok()
            .or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))
    }

    normalize(left).is_some_and(|left| normalize(right).is_some_and(|right| left == right))
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf, sync::Arc};

    use crate::{app_state::AppState, secrets::MemorySecretStore};

    use super::{parse_isolated_startup, validate_isolated_webview_environment};

    fn isolated_args(root: PathBuf) -> Vec<OsString> {
        vec![
            OsString::from("app"),
            OsString::from("--isolated-root"),
            root.into_os_string(),
        ]
    }

    #[test]
    fn isolated_startup_derives_every_path_without_migration_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();

        let isolated = parse_isolated_startup(&isolated_args(root.clone()), None)
            .unwrap()
            .unwrap();

        assert_eq!(isolated.root, root);
        assert_eq!(isolated.paths.data_directory, root.join("data"));
        assert_eq!(isolated.paths.logs_directory, root.join("logs"));
        assert_eq!(isolated.paths.config_path, root.join("config/local.json"));
        assert_eq!(isolated.webview_data_directory, root.join("webview"));
        assert!(isolated.paths.legacy_search_roots.is_empty());
        assert_ne!(isolated.secret_namespace, "default");
    }

    #[test]
    fn isolated_startup_rejects_ambiguous_or_unsafe_roots() {
        let relative = isolated_args(PathBuf::from("relative"));
        assert!(parse_isolated_startup(&relative, None).is_err());

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let duplicate = vec![
            OsString::from("app"),
            OsString::from("--isolated-root"),
            root.clone().into_os_string(),
            OsString::from("--isolated-root"),
            root.into_os_string(),
        ];
        assert!(parse_isolated_startup(&duplicate, None).is_err());
        assert!(
            parse_isolated_startup(
                &[OsString::from("app"), OsString::from("--isolated-root")],
                None,
            )
            .is_err()
        );

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("already-here"), b"data").unwrap();
        assert!(parse_isolated_startup(&isolated_args(directory.path().into()), None).is_err());

        let directory = tempfile::tempdir().unwrap();
        assert!(
            parse_isolated_startup(
                &isolated_args(directory.path().into()),
                Some(std::ffi::OsStr::new("outside/config.json")),
            )
            .is_err()
        );

        let directory = tempfile::tempdir().unwrap();
        let cli_override = vec![
            OsString::from("app"),
            OsString::from("--config"),
            directory.path().join("outside.json").into_os_string(),
            OsString::from("--isolated-root"),
            directory.path().to_path_buf().into_os_string(),
        ];
        assert!(parse_isolated_startup(&cli_override, None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn isolated_startup_rejects_reparse_root() {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("target");
        let link = parent.path().join("link");
        std::fs::create_dir(&target).unwrap();
        let status = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success(), "failed to create the test junction");

        assert!(parse_isolated_startup(&isolated_args(link), None).is_err());
    }

    #[test]
    fn isolated_app_state_confines_artifacts_and_does_not_alias_default_secrets() {
        let isolated_directory = tempfile::tempdir().unwrap();
        let isolated = parse_isolated_startup(
            &isolated_args(isolated_directory.path().canonicalize().unwrap()),
            None,
        )
        .unwrap()
        .unwrap();
        let shared_store = Arc::new(MemorySecretStore::default());
        let isolated_state = AppState::initialize_namespaced(
            isolated.paths.clone(),
            isolated.secret_namespace.clone(),
            shared_store.clone(),
        )
        .unwrap();

        let normal_directory = tempfile::tempdir().unwrap();
        let normal_paths = crate::app_state::AppPaths {
            data_directory: normal_directory.path().join("data"),
            logs_directory: normal_directory.path().join("logs"),
            config_path: normal_directory.path().join("config/local.json"),
            legacy_search_roots: Vec::new(),
        };
        let normal_state = AppState::initialize(normal_paths, shared_store).unwrap();
        isolated_state
            .secrets
            .set("provider/key", "isolated")
            .unwrap();
        normal_state.secrets.set("provider/key", "normal").unwrap();

        assert_eq!(
            isolated_state
                .secrets
                .read_internal("provider/key")
                .unwrap()
                .unwrap()
                .as_str(),
            "isolated"
        );
        assert_eq!(
            normal_state
                .secrets
                .read_internal("provider/key")
                .unwrap()
                .unwrap()
                .as_str(),
            "normal"
        );
        assert!(isolated.paths.config_path.is_file());
        assert!(isolated.paths.data_directory.join("app.sqlite3").is_file());
        assert!(isolated.paths.logs_directory.is_dir());
        for path in [
            &isolated.paths.config_path,
            &isolated.paths.data_directory,
            &isolated.paths.logs_directory,
        ] {
            assert!(
                path.starts_with(&isolated.root),
                "escaped isolated root: {}",
                path.display()
            );
        }
    }

    // 红灯先行（lane-I I02，配套 isolated_startup_accepts_inline_equals_form_for_webdriver_launchers）：
    // tauri-driver → msedgedriver 的官方自动化管线会为被测应用注入
    // WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS（--enable-automation/--remote-debugging-port 等）
    // 与 WEBVIEW2_USER_DATA_FOLDER（msedgedriver 临时 profile，实测见 e2e 探针）。
    // 只有 msedgedriver 会设置 TAURI_WEBVIEW_AUTOMATION（tauri-driver 源码 webdriver.rs），
    // 以它为开关放行这两项；非自动化环境维持原有的严格校验语义。
    #[test]
    fn isolated_startup_allows_official_webdriver_automation_webview_env() {
        let directory = tempfile::tempdir().unwrap();
        let isolated = parse_isolated_startup(
            &isolated_args(directory.path().canonicalize().unwrap()),
            None,
        )
        .unwrap()
        .unwrap();
        let automation = |extra: &[(OsString, OsString)]| {
            let mut environment = vec![(
                OsString::from("TAURI_WEBVIEW_AUTOMATION"),
                OsString::from("true"),
            )];
            environment.extend(extra.iter().cloned());
            environment
        };

        let injected = automation(&[
            (
                OsString::from("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"),
                OsString::from("--enable-automation --remote-debugging-port=0"),
            ),
            (
                OsString::from("WEBVIEW2_USER_DATA_FOLDER"),
                OsString::from("C:\\Windows\\Temp\\scoped_dir_msedgedriver"),
            ),
        ]);
        assert!(validate_isolated_webview_environment(&isolated, &injected).is_ok());

        // 自动化声明也不放行其余 WEBVIEW2_* 覆盖。
        let hostile = automation(&[(
            OsString::from("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER"),
            OsString::from("C:\\Windows\\Temp\\fake-runtime"),
        )]);
        assert!(validate_isolated_webview_environment(&isolated, &hostile).is_err());

        // 没有自动化声明时保持严格：同样两条注入必须拒绝。
        assert!(
            validate_isolated_webview_environment(
                &isolated,
                &injected.iter().skip(1).cloned().collect::<Vec<_>>()
            )
            .is_err()
        );
    }

    #[test]
    fn isolated_startup_rejects_webview_overrides_except_its_exact_data_folder() {
        let directory = tempfile::tempdir().unwrap();
        let requested_root = directory.path().to_path_buf();
        let isolated = parse_isolated_startup(&isolated_args(requested_root.clone()), None)
            .unwrap()
            .unwrap();
        let wrong_folder = vec![(
            OsString::from("WEBVIEW2_USER_DATA_FOLDER"),
            directory.path().join("outside").into_os_string(),
        )];
        assert!(validate_isolated_webview_environment(&isolated, &wrong_folder).is_err());

        let browser_arguments = vec![(
            OsString::from("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"),
            OsString::from("--remote-debugging-port=9222"),
        )];
        assert!(validate_isolated_webview_environment(&isolated, &browser_arguments).is_err());

        let exact_folder = vec![(
            OsString::from("WEBVIEW2_USER_DATA_FOLDER"),
            requested_root.join("webview").into_os_string(),
        )];
        assert!(validate_isolated_webview_environment(&isolated, &exact_folder).is_ok());
    }

    // 红灯先行（lane-I I02）：msedgedriver 官方 WebDriver 管线启动被测应用时，
    // 只会原样保留 `--flag=value` 单段参数；`--isolated-root <path>` 两段形式里
    // 的松散路径会被 Chromium 命令行解析改写（实测加 `--` 前缀、重排、小写）。
    // 因此隔离启动必须同时接受等号形式，桌面端 WebDriver e2e 才能建立隔离会话。
    #[test]
    fn isolated_startup_accepts_inline_equals_form_for_webdriver_launchers() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let inline = vec![
            OsString::from("app"),
            OsString::from(format!("--isolated-root={}", root.display())),
        ];
        let isolated = parse_isolated_startup(&inline, None).unwrap().unwrap();
        assert_eq!(isolated.root, root);
        assert_eq!(isolated.paths.config_path, root.join("config/local.json"));

        // 两种形式混用视为重复指定。
        let mixed = vec![
            OsString::from("app"),
            OsString::from(format!("--isolated-root={}", root.display())),
            OsString::from("--isolated-root"),
            root.into_os_string(),
        ];
        assert!(parse_isolated_startup(&mixed, None).is_err());

        // 空值与缺路径同样报错。
        let empty = vec![OsString::from("app"), OsString::from("--isolated-root=")];
        assert!(parse_isolated_startup(&empty, None).is_err());

        // 前缀撞车不是隔离参数（如 --isolated-rootx）。
        let prefix = vec![
            OsString::from("app"),
            OsString::from("--isolated-rootx"),
            OsString::from("value"),
        ];
        assert!(parse_isolated_startup(&prefix, None).unwrap().is_none());
    }
}
