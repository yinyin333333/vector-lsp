//! Detailed operational-log translations for Polish and Russian.
//!
//! Values in braces are protocol data and must remain byte-for-byte stable.

use crate::i18n::Locale;

pub fn template(locale: Locale, key: &str) -> Option<&'static str> {
    match locale {
        Locale::PlPl => polish(key),
        Locale::RuRu => russian(key),
        _ => None,
    }
}

fn polish(key: &str) -> Option<&'static str> {
    Some(match key {
        "log.effective_config" => "Obowiązująca konfiguracja vector-lsp: {summary}",
        "log.watch_registration_prepare_failed" => {
            "Nie udało się przygotować rejestracji obserwowania plików: {error}"
        }
        "log.file_watching_unavailable" => {
            "Obserwowanie plików jest niedostępne dla „{baseUri}”: {error}"
        }
        "log.json_watch_retire_failed" => {
            "Nie udało się usunąć nieaktualnych obserwatorów plików JSON; następna synchronizacja zakresu JSON ponowi próbę: {error}"
        }
        "log.workspace_scan_root_missing" => {
            "Skanowanie obszaru roboczego nie powiodło się: inicjalizacja nie podała głównego URI."
        }
        "log.workspace_scan_root_not_file" => {
            "Skanowanie obszaru roboczego nie powiodło się: główny URI nie jest ścieżką pliku: {rootUri}"
        }
        "log.workspace_scan_failed" => "Skanowanie obszaru roboczego nie powiodło się: {error}",
        "log.scan_path_uri_conversion_failed" => {
            "Nie udało się przekształcić ścieżki skanowania w URI."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "Nie udało się przekształcić ścieżki skanowania danych referencyjnych w URI."
        }
        "log.scan_reference_root_failed" => {
            "Skanowanie danych referencyjnych nie powiodło się: {error}"
        }
        "log.scan_reference_root_not_file" => {
            "Główny URI danych referencyjnych nie jest ścieżką pliku: {rootUri}"
        }
        "log.scan_task_failed" => "Zadanie skanowania obszaru roboczego nie powiodło się: {error}",
        "log.workspace_scan_skipped" => {
            "Pominięto skanowanie obszaru roboczego dla „{path}”: {reason}"
        }
        "log.workspace_indexed" => {
            "Zindeksowano {count} plików obszaru roboczego; pominięto {skippedCount} ścieżek."
        }
        "log.scan_background_parse_failed" => {
            "Analiza podczas skanowania w tle nie powiodła się: {error}"
        }
        "log.schema_task_stopped" => {
            "Nie udało się wczytać schematu, ponieważ jego zadanie w tle zostało zatrzymane: {error}"
        }
        "log.reference_tables_loaded" => {
            "Wczytano {count} ukrytych tabel referencyjnych dla wersji gry {version} ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "Dołączony awaryjny zestaw danych referencyjnych jest wyłączony: brak jawnej lub możliwej do wywnioskowania wersji gry."
        }
        "log.reference_fallback_disabled" => {
            "Dołączony awaryjny zestaw danych referencyjnych jest wyłączony w tej sesji: {error}"
        }
        "log.reference_data_load_failed" => {
            "Nie udało się wczytać dołączonych danych referencyjnych: {error}"
        }
        "log.did_close_restore_failed" => {
            "Przywrócenie pliku z dysku przy didClose nie powiodło się dla {uri}: {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Błąd: tryb single_shot wymaga workspace_path w konfiguracji."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Błąd: nie udało się uruchomić środowiska wykonawczego wtyczek: {error}"
        }
        "cli.schema_task_panicked" => "Błąd: zadanie schematu zakończyło się paniką: {error}",
        "cli.plugins_loaded" => "Wczytano {count} plików wtyczek.",
        "cli.reference_fallback_disabled" => {
            "Ostrzeżenie: dołączony awaryjny zestaw danych referencyjnych jest wyłączony dla tego uruchomienia: {error}"
        }
        "cli.reference_tables_loaded" => {
            "Wczytano {count} ukrytych tabel referencyjnych dla wersji gry {gameVersion} ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "Dołączony awaryjny zestaw danych referencyjnych jest wyłączony dla tego uruchomienia."
        }
        "cli.workspace_read_failed" => {
            "Błąd: nie można odczytać katalogu obszaru roboczego „{path}”: {error}"
        }
        "cli.path_skipped" => "Ostrzeżenie: pominięto „{path}”: {error}",
        "cli.check_summary" => {
            "{errors} błędów, {warnings} ostrzeżeń, {infos} informacji i {hints} diagnostyk podpowiedzi w {fileCount} plikach; przeanalizowano {parsedFileCount} plików."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp: nie udało się uruchomić środowiska wykonawczego wtyczek TCP: {error}"
        }
        _ => return None,
    })
}

fn russian(key: &str) -> Option<&'static str> {
    Some(match key {
        "log.effective_config" => "Действующая конфигурация vector-lsp: {summary}",
        "log.watch_registration_prepare_failed" => {
            "Не удалось подготовить регистрацию отслеживания файлов: {error}"
        }
        "log.file_watching_unavailable" => {
            "Отслеживание файлов недоступно для «{baseUri}»: {error}"
        }
        "log.json_watch_retire_failed" => {
            "Не удалось отключить устаревшие наблюдатели JSON-файлов; следующая синхронизация области JSON повторит попытку: {error}"
        }
        "log.workspace_scan_root_missing" => {
            "Сканирование рабочей области не выполнено: при инициализации не передан корневой URI."
        }
        "log.workspace_scan_root_not_file" => {
            "Сканирование рабочей области не выполнено: корневой URI не является путём к файлу: {rootUri}"
        }
        "log.workspace_scan_failed" => "Сканирование рабочей области не выполнено: {error}",
        "log.scan_path_uri_conversion_failed" => {
            "Не удалось преобразовать путь сканирования в URI."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "Не удалось преобразовать путь сканирования справочных данных в URI."
        }
        "log.scan_reference_root_failed" => "Сканирование справочных данных не выполнено: {error}",
        "log.scan_reference_root_not_file" => {
            "Корневой URI справочных данных не является путём к файлу: {rootUri}"
        }
        "log.scan_task_failed" => "Задача сканирования рабочей области не выполнена: {error}",
        "log.workspace_scan_skipped" => {
            "Сканирование рабочей области пропущено для «{path}»: {reason}"
        }
        "log.workspace_indexed" => {
            "Проиндексировано файлов рабочей области: {count}; пропущено путей: {skippedCount}."
        }
        "log.scan_background_parse_failed" => {
            "Синтаксический разбор при фоновом сканировании не выполнен: {error}"
        }
        "log.schema_task_stopped" => {
            "Не удалось загрузить схему, поскольку её фоновая задача была остановлена: {error}"
        }
        "log.reference_tables_loaded" => {
            "Загружено скрытых справочных таблиц: {count}; версия игры {version} ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "Встроенный резервный набор справочных данных отключён: версия игры не задана и не может быть определена."
        }
        "log.reference_fallback_disabled" => {
            "Встроенный резервный набор справочных данных отключён для этого сеанса: {error}"
        }
        "log.reference_data_load_failed" => {
            "Не удалось загрузить встроенные справочные данные: {error}"
        }
        "log.did_close_restore_failed" => {
            "Восстановление с диска при didClose не выполнено для {uri}: {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Ошибка: для режима single_shot требуется workspace_path в конфигурации."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Ошибка: не удалось запустить среду выполнения плагинов: {error}"
        }
        "cli.schema_task_panicked" => "Ошибка: задача схемы завершилась паникой: {error}",
        "cli.plugins_loaded" => "Загружено файлов плагинов: {count}.",
        "cli.reference_fallback_disabled" => {
            "Предупреждение: встроенный резервный набор справочных данных отключён для этого запуска: {error}"
        }
        "cli.reference_tables_loaded" => {
            "Загружено скрытых справочных таблиц: {count}; версия игры {gameVersion} ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "Встроенный резервный набор справочных данных отключён для этого запуска."
        }
        "cli.workspace_read_failed" => {
            "Ошибка: не удалось прочитать каталог рабочей области «{path}»: {error}"
        }
        "cli.path_skipped" => "Предупреждение: путь «{path}» пропущен: {error}",
        "cli.check_summary" => {
            "Ошибок: {errors}, предупреждений: {warnings}, сведений: {infos}, диагностик-подсказок: {hints}; файлов: {fileCount}, разобрано файлов: {parsedFileCount}."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp: не удалось запустить среду выполнения TCP-плагинов: {error}"
        }
        _ => return None,
    })
}
