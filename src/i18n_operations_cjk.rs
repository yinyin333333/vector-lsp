//! Detailed CJK wording for operational logs and command-line status.
//!
//! Values enclosed in braces are protocol arguments and must stay unchanged.

use crate::i18n::Locale;

/// Returns the complete locale-specific sentence for an operational message.
///
/// `None` deliberately means that this module does not own the locale or key.
pub fn template(locale: Locale, key: &str) -> Option<&'static str> {
    match locale {
        Locale::KoKr => match key {
            "log.effective_config" => Some("적용된 vector-lsp 구성: {summary}"),
            "log.watch_registration_prepare_failed" => {
                Some("감시 파일 등록을 준비하지 못했습니다: {error}")
            }
            "log.file_watching_unavailable" => {
                Some("'{baseUri}'에서 파일 감시를 사용할 수 없습니다: {error}")
            }
            "log.json_watch_retire_failed" => Some(
                "오래된 JSON 파일 감시를 해제하지 못했습니다. 다음 JSON 범위 동기화 때 다시 시도합니다: {error}",
            ),
            "log.workspace_scan_root_missing" => {
                Some("작업 영역을 검사하지 못했습니다. 초기화 요청에 루트 URI가 없습니다.")
            }
            "log.workspace_scan_root_not_file" => {
                Some("작업 영역을 검사하지 못했습니다. 루트 URI가 파일 경로가 아닙니다: {rootUri}")
            }
            "log.workspace_scan_failed" => Some("작업 영역 검사에 실패했습니다: {error}"),
            "log.scan_path_uri_conversion_failed" => Some("검사 경로를 URI로 변환하지 못했습니다."),
            "log.scan_reference_path_uri_conversion_failed" => {
                Some("참조 검사 경로를 URI로 변환하지 못했습니다.")
            }
            "log.scan_reference_root_failed" => Some("참조 검사를 수행하지 못했습니다: {error}"),
            "log.scan_reference_root_not_file" => {
                Some("참조 루트 URI가 파일 경로가 아닙니다: {rootUri}")
            }
            "log.scan_task_failed" => Some("작업 영역 검사 작업에 실패했습니다: {error}"),
            "log.workspace_scan_skipped" => {
                Some("작업 영역 검사에서 '{path}'을(를) 건너뛰었습니다: {reason}")
            }
            "log.workspace_indexed" => {
                Some("작업 영역 파일 {count}개를 색인했고 경로 {skippedCount}개를 건너뛰었습니다.")
            }
            "log.scan_background_parse_failed" => {
                Some("백그라운드 검사 파싱에 실패했습니다: {error}")
            }
            "log.schema_task_stopped" => {
                Some("백그라운드 작업이 중지되어 스키마를 불러오지 못했습니다: {error}")
            }
            "log.reference_tables_loaded" => {
                Some("게임 버전 {version}의 숨겨진 참조 테이블 {count}개를 불러왔습니다({digest}).")
            }
            "log.reference_fallback_no_version" => Some(
                "명시했거나 추론할 수 있는 게임 버전이 없어 내장 참조 대체 기능을 비활성화했습니다.",
            ),
            "log.reference_fallback_disabled" => {
                Some("이 세션에서 내장 참조 대체 기능을 비활성화했습니다: {error}")
            }
            "log.reference_data_load_failed" => {
                Some("내장 참조 데이터를 불러오지 못했습니다: {error}")
            }
            "log.did_close_restore_failed" => {
                Some("{uri}에 대해 didClose 디스크 복원에 실패했습니다: {error}")
            }
            "cli.single_shot_workspace_required" => {
                Some("오류: single_shot 모드에는 구성의 `workspace_path`가 필요합니다.")
            }
            "cli.plugin_runtime_startup_failed" => {
                Some("오류: 플러그인 런타임을 시작하지 못했습니다: {error}")
            }
            "cli.schema_task_panicked" => {
                Some("오류: 스키마 작업이 패닉으로 종료되었습니다: {error}")
            }
            "cli.plugins_loaded" => Some("플러그인 파일 {count}개를 불러왔습니다."),
            "cli.reference_fallback_disabled" => {
                Some("경고: 이번 실행에서 내장 참조 대체 기능을 비활성화했습니다: {error}")
            }
            "cli.reference_tables_loaded" => Some(
                "게임 버전 {gameVersion}의 숨겨진 참조 테이블 {count}개를 불러왔습니다({checksum}).",
            ),
            "cli.reference_fallback_disabled_for_run" => {
                Some("이번 실행에서는 내장 참조 대체 기능을 사용할 수 없습니다.")
            }
            "cli.workspace_read_failed" => {
                Some("오류: 작업 영역 디렉터리 '{path}'을(를) 읽지 못했습니다: {error}")
            }
            "cli.path_skipped" => Some("경고: '{path}'을(를) 건너뜁니다: {error}"),
            "cli.check_summary" => Some(
                "파일 {fileCount}개에서 오류 {errors}개, 경고 {warnings}개, 정보 {infos}개, 힌트 {hints}개를 발견했고 파일 {parsedFileCount}개를 파싱했습니다.",
            ),
            "cli.tcp_plugin_runtime_startup_failed" => {
                Some("vector-lsp: TCP 플러그인 런타임을 시작하지 못했습니다: {error}")
            }
            _ => None,
        },
        Locale::ZhCn => match key {
            "log.effective_config" => Some("生效的 vector-lsp 配置：{summary}"),
            "log.watch_registration_prepare_failed" => Some("无法准备受监视文件注册：{error}"),
            "log.file_watching_unavailable" => Some("'{baseUri}' 无法使用文件监视：{error}"),
            "log.json_watch_retire_failed" => {
                Some("无法移除过期的 JSON 文件监视器；将在下次 JSON 范围同步时重试：{error}")
            }
            "log.workspace_scan_root_missing" => Some("工作区扫描失败：初始化请求未提供根 URI。"),
            "log.workspace_scan_root_not_file" => {
                Some("工作区扫描失败：根 URI 不是文件路径：{rootUri}")
            }
            "log.workspace_scan_failed" => Some("工作区扫描失败：{error}"),
            "log.scan_path_uri_conversion_failed" => Some("无法将扫描路径转换为 URI。"),
            "log.scan_reference_path_uri_conversion_failed" => {
                Some("无法将参考扫描路径转换为 URI。")
            }
            "log.scan_reference_root_failed" => Some("参考扫描失败：{error}"),
            "log.scan_reference_root_not_file" => Some("参考根 URI 不是文件路径：{rootUri}"),
            "log.scan_task_failed" => Some("工作区扫描任务失败：{error}"),
            "log.workspace_scan_skipped" => Some("工作区扫描跳过了 '{path}'：{reason}"),
            "log.workspace_indexed" => {
                Some("已索引 {count} 个工作区文件；跳过了 {skippedCount} 个路径。")
            }
            "log.scan_background_parse_failed" => Some("后台扫描解析失败：{error}"),
            "log.schema_task_stopped" => Some("后台任务已停止，无法加载架构：{error}"),
            "log.reference_tables_loaded" => {
                Some("已加载游戏版本 {version} 的 {count} 个隐藏参考表（{digest}）。")
            }
            "log.reference_fallback_no_version" => {
                Some("没有明确指定或可推断的游戏版本，已禁用内置参考回退。")
            }
            "log.reference_fallback_disabled" => Some("已为此会话禁用内置参考回退：{error}"),
            "log.reference_data_load_failed" => Some("无法加载内置参考数据：{error}"),
            "log.did_close_restore_failed" => Some("无法为 {uri} 执行 didClose 磁盘还原：{error}"),
            "cli.single_shot_workspace_required" => {
                Some("错误：single_shot 模式要求配置中包含 `workspace_path`。")
            }
            "cli.plugin_runtime_startup_failed" => Some("错误：无法启动插件运行时：{error}"),
            "cli.schema_task_panicked" => Some("错误：架构任务发生恐慌：{error}"),
            "cli.plugins_loaded" => Some("已加载 {count} 个插件文件。"),
            "cli.reference_fallback_disabled" => {
                Some("警告：已为本次运行禁用内置参考回退：{error}")
            }
            "cli.reference_tables_loaded" => {
                Some("已加载游戏版本 {gameVersion} 的 {count} 个隐藏参考表（{checksum}）。")
            }
            "cli.reference_fallback_disabled_for_run" => Some("本次运行已禁用内置参考回退。"),
            "cli.workspace_read_failed" => Some("错误：无法读取工作区目录 '{path}'：{error}"),
            "cli.path_skipped" => Some("警告：跳过 '{path}'：{error}"),
            "cli.check_summary" => Some(
                "共检查 {fileCount} 个文件：{errors} 个错误、{warnings} 个警告、{infos} 条信息和 {hints} 个提示诊断；已解析 {parsedFileCount} 个文件。",
            ),
            "cli.tcp_plugin_runtime_startup_failed" => {
                Some("vector-lsp：TCP 插件运行时启动失败：{error}")
            }
            _ => None,
        },
        Locale::ZhTw => match key {
            "log.effective_config" => Some("生效的 vector-lsp 設定：{summary}"),
            "log.watch_registration_prepare_failed" => Some("無法準備監視檔案註冊：{error}"),
            "log.file_watching_unavailable" => Some("'{baseUri}' 無法使用檔案監視：{error}"),
            "log.json_watch_retire_failed" => {
                Some("無法移除過期的 JSON 檔案監視器；將在下次 JSON 範圍同步時重試：{error}")
            }
            "log.workspace_scan_root_missing" => Some("工作區掃描失敗：初始化要求未提供根 URI。"),
            "log.workspace_scan_root_not_file" => {
                Some("工作區掃描失敗：根 URI 不是檔案路徑：{rootUri}")
            }
            "log.workspace_scan_failed" => Some("工作區掃描失敗：{error}"),
            "log.scan_path_uri_conversion_failed" => Some("無法將掃描路徑轉換為 URI。"),
            "log.scan_reference_path_uri_conversion_failed" => {
                Some("無法將參照掃描路徑轉換為 URI。")
            }
            "log.scan_reference_root_failed" => Some("參照掃描失敗：{error}"),
            "log.scan_reference_root_not_file" => Some("參照根 URI 不是檔案路徑：{rootUri}"),
            "log.scan_task_failed" => Some("工作區掃描工作失敗：{error}"),
            "log.workspace_scan_skipped" => Some("工作區掃描略過了 '{path}'：{reason}"),
            "log.workspace_indexed" => {
                Some("已建立 {count} 個工作區檔案的索引；略過 {skippedCount} 個路徑。")
            }
            "log.scan_background_parse_failed" => Some("背景掃描剖析失敗：{error}"),
            "log.schema_task_stopped" => Some("背景工作已停止，無法載入結構描述：{error}"),
            "log.reference_tables_loaded" => {
                Some("已載入遊戲版本 {version} 的 {count} 個隱藏參照表（{digest}）。")
            }
            "log.reference_fallback_no_version" => {
                Some("沒有明確指定或可推斷的遊戲版本，已停用內建參照備援。")
            }
            "log.reference_fallback_disabled" => Some("已為此工作階段停用內建參照備援：{error}"),
            "log.reference_data_load_failed" => Some("無法載入內建參照資料：{error}"),
            "log.did_close_restore_failed" => Some("無法為 {uri} 執行 didClose 磁碟還原：{error}"),
            "cli.single_shot_workspace_required" => {
                Some("錯誤：single_shot 模式要求設定中包含 `workspace_path`。")
            }
            "cli.plugin_runtime_startup_failed" => Some("錯誤：無法啟動外掛程式執行階段：{error}"),
            "cli.schema_task_panicked" => Some("錯誤：結構描述工作發生恐慌：{error}"),
            "cli.plugins_loaded" => Some("已載入 {count} 個外掛程式檔案。"),
            "cli.reference_fallback_disabled" => {
                Some("警告：已為本次執行停用內建參照備援：{error}")
            }
            "cli.reference_tables_loaded" => {
                Some("已載入遊戲版本 {gameVersion} 的 {count} 個隱藏參照表（{checksum}）。")
            }
            "cli.reference_fallback_disabled_for_run" => Some("本次執行已停用內建參照備援。"),
            "cli.workspace_read_failed" => Some("錯誤：無法讀取工作區目錄 '{path}'：{error}"),
            "cli.path_skipped" => Some("警告：略過 '{path}'：{error}"),
            "cli.check_summary" => Some(
                "共檢查 {fileCount} 個檔案：{errors} 個錯誤、{warnings} 個警告、{infos} 則資訊及 {hints} 個提示診斷；已剖析 {parsedFileCount} 個檔案。",
            ),
            "cli.tcp_plugin_runtime_startup_failed" => {
                Some("vector-lsp：TCP 外掛程式執行階段啟動失敗：{error}")
            }
            _ => None,
        },
        Locale::JaJp => match key {
            "log.effective_config" => Some("有効な vector-lsp 設定: {summary}"),
            "log.watch_registration_prepare_failed" => {
                Some("監視対象ファイルの登録を準備できませんでした: {error}")
            }
            "log.file_watching_unavailable" => {
                Some("'{baseUri}' ではファイル監視を利用できません: {error}")
            }
            "log.json_watch_retire_failed" => Some(
                "古い JSON ファイル監視を解除できませんでした。次回の JSON スコープ同期で再試行します: {error}",
            ),
            "log.workspace_scan_root_missing" => Some(
                "ワークスペースのスキャンに失敗しました。初期化要求にルート URI がありません。",
            ),
            "log.workspace_scan_root_not_file" => Some(
                "ワークスペースのスキャンに失敗しました。ルート URI はファイルパスではありません: {rootUri}",
            ),
            "log.workspace_scan_failed" => Some("ワークスペースのスキャンに失敗しました: {error}"),
            "log.scan_path_uri_conversion_failed" => {
                Some("スキャンパスを URI に変換できませんでした。")
            }
            "log.scan_reference_path_uri_conversion_failed" => {
                Some("参照スキャンパスを URI に変換できませんでした。")
            }
            "log.scan_reference_root_failed" => Some("参照スキャンに失敗しました: {error}"),
            "log.scan_reference_root_not_file" => {
                Some("参照ルート URI はファイルパスではありません: {rootUri}")
            }
            "log.scan_task_failed" => Some("ワークスペーススキャンタスクに失敗しました: {error}"),
            "log.workspace_scan_skipped" => {
                Some("ワークスペースのスキャンで '{path}' をスキップしました: {reason}")
            }
            "log.workspace_indexed" => Some(
                "ワークスペースファイル {count} 件をインデックス化し、{skippedCount} 件のパスをスキップしました。",
            ),
            "log.scan_background_parse_failed" => {
                Some("バックグラウンドスキャンの解析に失敗しました: {error}")
            }
            "log.schema_task_stopped" => Some(
                "バックグラウンドタスクが停止したため、スキーマを読み込めませんでした: {error}",
            ),
            "log.reference_tables_loaded" => Some(
                "ゲームバージョン {version} の非表示参照テーブル {count} 件を読み込みました（{digest}）。",
            ),
            "log.reference_fallback_no_version" => Some(
                "明示または推測可能なゲームバージョンがないため、同梱の参照フォールバックを無効にしました。",
            ),
            "log.reference_fallback_disabled" => {
                Some("このセッションでは同梱の参照フォールバックを無効にしました: {error}")
            }
            "log.reference_data_load_failed" => {
                Some("同梱の参照データを読み込めませんでした: {error}")
            }
            "log.did_close_restore_failed" => {
                Some("{uri} の didClose ディスク復元に失敗しました: {error}")
            }
            "cli.single_shot_workspace_required" => {
                Some("エラー: single_shot モードには設定内の `workspace_path` が必要です。")
            }
            "cli.plugin_runtime_startup_failed" => {
                Some("エラー: プラグインランタイムを起動できませんでした: {error}")
            }
            "cli.schema_task_panicked" => {
                Some("エラー: スキーマタスクがパニックで終了しました: {error}")
            }
            "cli.plugins_loaded" => Some("プラグインファイル {count} 件を読み込みました。"),
            "cli.reference_fallback_disabled" => {
                Some("警告: 今回の実行では同梱の参照フォールバックを無効にしました: {error}")
            }
            "cli.reference_tables_loaded" => Some(
                "ゲームバージョン {gameVersion} の非表示参照テーブル {count} 件を読み込みました（{checksum}）。",
            ),
            "cli.reference_fallback_disabled_for_run" => {
                Some("今回の実行では同梱の参照フォールバックを無効にしています。")
            }
            "cli.workspace_read_failed" => {
                Some("エラー: ワークスペースディレクトリ '{path}' を読み取れませんでした: {error}")
            }
            "cli.path_skipped" => Some("警告: '{path}' をスキップします: {error}"),
            "cli.check_summary" => Some(
                "{fileCount} ファイルでエラー {errors} 件、警告 {warnings} 件、情報 {infos} 件、ヒント診断 {hints} 件を検出し、{parsedFileCount} ファイルを解析しました。",
            ),
            "cli.tcp_plugin_runtime_startup_failed" => {
                Some("vector-lsp: TCP プラグインランタイムの起動に失敗しました: {error}")
            }
            _ => None,
        },
        _ => None,
    }
}
