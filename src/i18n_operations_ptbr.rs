//! Detailed Brazilian Portuguese wording for operational logs and CLI status.
//!
//! Values enclosed in braces are protocol arguments and must stay unchanged.

use crate::i18n::Locale;

/// Returns the complete Brazilian Portuguese sentence for an operational key.
pub fn template(locale: Locale, key: &str) -> Option<&'static str> {
    if locale != Locale::PtBr {
        return None;
    }

    Some(match key {
        "log.effective_config" => "Configuração efetiva do vector-lsp: {summary}",
        "log.watch_registration_prepare_failed" => {
            "Não foi possível preparar o registro de monitoramento de arquivos: {error}"
        }
        "log.file_watching_unavailable" => {
            "O monitoramento de arquivos não está disponível para '{baseUri}': {error}"
        }
        "log.json_watch_retire_failed" => {
            "Não foi possível remover os monitores de arquivos JSON obsoletos; a próxima sincronização do escopo JSON tentará novamente: {error}"
        }
        "log.workspace_scan_root_missing" => {
            "A verificação do espaço de trabalho falhou: a inicialização não forneceu uma URI raiz."
        }
        "log.workspace_scan_root_not_file" => {
            "A verificação do espaço de trabalho falhou: a URI raiz não é um caminho de arquivo: {rootUri}"
        }
        "log.workspace_scan_failed" => "A verificação do espaço de trabalho falhou: {error}",
        "log.scan_path_uri_conversion_failed" => {
            "Não foi possível converter o caminho da verificação em uma URI."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "Não foi possível converter o caminho da verificação de referências em uma URI."
        }
        "log.scan_reference_root_failed" => "A verificação de referências falhou: {error}",
        "log.scan_reference_root_not_file" => {
            "A URI raiz das referências não é um caminho de arquivo: {rootUri}"
        }
        "log.scan_task_failed" => "A tarefa de verificação do espaço de trabalho falhou: {error}",
        "log.workspace_scan_skipped" => {
            "A verificação do espaço de trabalho ignorou '{path}': {reason}"
        }
        "log.workspace_indexed" => {
            "Foram indexados {count} arquivos do espaço de trabalho; {skippedCount} caminho(s) foram ignorados."
        }
        "log.scan_background_parse_failed" => {
            "A análise durante a verificação em segundo plano falhou: {error}"
        }
        "log.schema_task_stopped" => {
            "Não foi possível carregar o esquema porque a tarefa em segundo plano foi interrompida: {error}"
        }
        "log.reference_tables_loaded" => {
            "Foram carregadas {count} tabelas de referência ocultas para a versão do jogo {version} ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "O recurso de referência alternativo incluído está desativado: não há uma versão do jogo explícita nem inferível."
        }
        "log.reference_fallback_disabled" => {
            "O recurso de referência alternativo incluído está desativado nesta sessão: {error}"
        }
        "log.reference_data_load_failed" => {
            "Não foi possível carregar os dados de referência incluídos: {error}"
        }
        "log.did_close_restore_failed" => {
            "A restauração do disco em didClose falhou para {uri}: {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Erro: o modo single_shot exige `workspace_path` na configuração."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Erro: não foi possível iniciar o ambiente de execução dos plugins: {error}"
        }
        "cli.schema_task_panicked" => "Erro: a tarefa de esquema sofreu uma falha interna: {error}",
        "cli.plugins_loaded" => "Foram carregados {count} arquivos de plugin.",
        "cli.reference_fallback_disabled" => {
            "Aviso: o recurso de referência alternativo incluído está desativado nesta execução: {error}"
        }
        "cli.reference_tables_loaded" => {
            "Foram carregadas {count} tabelas de referência ocultas para a versão do jogo {gameVersion} ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "O recurso de referência alternativo incluído está desativado nesta execução."
        }
        "cli.workspace_read_failed" => {
            "Erro: não foi possível ler o diretório do espaço de trabalho '{path}': {error}"
        }
        "cli.path_skipped" => "Aviso: '{path}' foi ignorado: {error}",
        "cli.check_summary" => {
            "{errors} erro(s), {warnings} aviso(s), {infos} informação(ões) e {hints} diagnóstico(s) de dica em {fileCount} arquivo(s); {parsedFileCount} arquivo(s) foram analisados."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp: não foi possível iniciar o ambiente TCP dos plugins: {error}"
        }
        _ => return None,
    })
}
