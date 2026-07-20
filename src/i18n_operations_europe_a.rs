//! Detailed operational-log translations for the western European locales.
//!
//! Values in braces are protocol data and must remain byte-for-byte stable.

use crate::i18n::Locale;

pub fn template(locale: Locale, key: &str) -> Option<&'static str> {
    match locale {
        Locale::DeDe => german(key),
        Locale::FrFr => french(key),
        Locale::ItIt => italian(key),
        Locale::EsEs => spanish(key),
        _ => None,
    }
}

fn german(key: &str) -> Option<&'static str> {
    Some(match key {
        "log.effective_config" => "Wirksame vector-lsp-Konfiguration: {summary}",
        "log.watch_registration_prepare_failed" => {
            "Die Registrierung der Dateiüberwachung konnte nicht vorbereitet werden: {error}"
        }
        "log.file_watching_unavailable" => {
            "Die Dateiüberwachung für „{baseUri}“ ist nicht verfügbar: {error}"
        }
        "log.json_watch_retire_failed" => {
            "Veraltete JSON-Dateiüberwachungen konnten nicht entfernt werden; die nächste JSON-Bereichssynchronisierung wird erneut versuchen: {error}"
        }
        "log.workspace_scan_root_missing" => {
            "Arbeitsbereichssuche fehlgeschlagen: Bei der Initialisierung wurde keine Stamm-URI bereitgestellt."
        }
        "log.workspace_scan_root_not_file" => {
            "Arbeitsbereichssuche fehlgeschlagen: Die Stamm-URI ist kein Dateipfad: {rootUri}"
        }
        "log.workspace_scan_failed" => "Arbeitsbereichssuche fehlgeschlagen: {error}",
        "log.scan_path_uri_conversion_failed" => {
            "Der Suchpfad konnte nicht in eine URI umgewandelt werden."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "Der Referenzsuchpfad konnte nicht in eine URI umgewandelt werden."
        }
        "log.scan_reference_root_failed" => "Referenzsuche fehlgeschlagen: {error}",
        "log.scan_reference_root_not_file" => "Die Referenzstamm-URI ist kein Dateipfad: {rootUri}",
        "log.scan_task_failed" => {
            "Die Aufgabe zur Arbeitsbereichssuche ist fehlgeschlagen: {error}"
        }
        "log.workspace_scan_skipped" => "Arbeitsbereichssuche für „{path}“ übersprungen: {reason}",
        "log.workspace_indexed" => {
            "{count} Arbeitsbereichsdatei(en) wurden indiziert; {skippedCount} Pfad(e) übersprungen."
        }
        "log.scan_background_parse_failed" => {
            "Das Parsen während der Hintergrundsuche ist fehlgeschlagen: {error}"
        }
        "log.schema_task_stopped" => {
            "Das Schema konnte nicht geladen werden, weil seine Hintergrundaufgabe beendet wurde: {error}"
        }
        "log.reference_tables_loaded" => {
            "{count} ausgeblendete Referenztabelle(n) für Spielversion {version} geladen ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "Der gebündelte Referenz-Fallback ist deaktiviert: Keine explizite oder ableitbare Spielversion."
        }
        "log.reference_fallback_disabled" => {
            "Der gebündelte Referenz-Fallback ist für diese Sitzung deaktiviert: {error}"
        }
        "log.reference_data_load_failed" => {
            "Die gebündelten Referenzdaten konnten nicht geladen werden: {error}"
        }
        "log.did_close_restore_failed" => {
            "Die Wiederherstellung von der Festplatte bei didClose ist für {uri} fehlgeschlagen: {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Fehler: Der Modus single_shot benötigt workspace_path in der Konfiguration."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Fehler: Die Plugin-Laufzeit konnte nicht gestartet werden: {error}"
        }
        "cli.schema_task_panicked" => {
            "Fehler: Die Schemaaufgabe ist mit einem Panic abgebrochen: {error}"
        }
        "cli.plugins_loaded" => "{count} Plugin-Datei(en) geladen.",
        "cli.reference_fallback_disabled" => {
            "Warnung: Der gebündelte Referenz-Fallback ist für diesen Lauf deaktiviert: {error}"
        }
        "cli.reference_tables_loaded" => {
            "{count} ausgeblendete Referenztabelle(n) für Spielversion {gameVersion} geladen ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "Der gebündelte Referenz-Fallback ist für diesen Lauf deaktiviert."
        }
        "cli.workspace_read_failed" => {
            "Fehler: Das Arbeitsbereichsverzeichnis „{path}“ kann nicht gelesen werden: {error}"
        }
        "cli.path_skipped" => "Warnung: „{path}“ wird übersprungen: {error}",
        "cli.check_summary" => {
            "{errors} Fehler, {warnings} Warnung(en), {infos} Information(en) und {hints} Hinweisdiagnose(n) in {fileCount} Datei(en); {parsedFileCount} Datei(en) wurden geparst."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp: Die TCP-Plugin-Laufzeit konnte nicht gestartet werden: {error}"
        }
        _ => return None,
    })
}

fn french(key: &str) -> Option<&'static str> {
    Some(match key {
        "log.effective_config" => "Configuration vector-lsp effective : {summary}",
        "log.watch_registration_prepare_failed" => {
            "Impossible de préparer l’inscription à la surveillance des fichiers : {error}"
        }
        "log.file_watching_unavailable" => {
            "La surveillance des fichiers n’est pas disponible pour « {baseUri} » : {error}"
        }
        "log.json_watch_retire_failed" => {
            "Impossible de retirer les surveillances de fichiers JSON obsolètes ; la prochaine synchronisation de portée JSON réessaiera : {error}"
        }
        "log.workspace_scan_root_missing" => {
            "Analyse de l’espace de travail impossible : l’initialisation n’a fourni aucune URI racine."
        }
        "log.workspace_scan_root_not_file" => {
            "Analyse de l’espace de travail impossible : l’URI racine n’est pas un chemin de fichier : {rootUri}"
        }
        "log.workspace_scan_failed" => "Analyse de l’espace de travail impossible : {error}",
        "log.scan_path_uri_conversion_failed" => {
            "Impossible de convertir le chemin d’analyse en URI."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "Impossible de convertir le chemin d’analyse des références en URI."
        }
        "log.scan_reference_root_failed" => "Analyse des références impossible : {error}",
        "log.scan_reference_root_not_file" => {
            "L’URI racine des références n’est pas un chemin de fichier : {rootUri}"
        }
        "log.scan_task_failed" => "La tâche d’analyse de l’espace de travail a échoué : {error}",
        "log.workspace_scan_skipped" => {
            "Analyse de l’espace de travail ignorée pour « {path} » : {reason}"
        }
        "log.workspace_indexed" => {
            "{count} fichier(s) de l’espace de travail indexé(s) ; {skippedCount} chemin(s) ignoré(s)."
        }
        "log.scan_background_parse_failed" => {
            "L’analyse syntaxique pendant l’exploration en arrière-plan a échoué : {error}"
        }
        "log.schema_task_stopped" => {
            "Impossible de charger le schéma car sa tâche d’arrière-plan s’est arrêtée : {error}"
        }
        "log.reference_tables_loaded" => {
            "{count} table(s) de référence masquée(s) chargée(s) pour la version de jeu {version} ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "Le repli de références intégré est désactivé : aucune version de jeu explicite ou déductible."
        }
        "log.reference_fallback_disabled" => {
            "Le repli de références intégré est désactivé pour cette session : {error}"
        }
        "log.reference_data_load_failed" => {
            "Impossible de charger les données de référence intégrées : {error}"
        }
        "log.did_close_restore_failed" => {
            "La restauration depuis le disque lors de didClose a échoué pour {uri} : {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Erreur : le mode single_shot exige workspace_path dans la configuration."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Erreur : démarrage de l’environnement d’exécution des plugins impossible : {error}"
        }
        "cli.schema_task_panicked" => {
            "Erreur : la tâche de schéma a déclenché une panique : {error}"
        }
        "cli.plugins_loaded" => "{count} fichier(s) de plugin chargé(s).",
        "cli.reference_fallback_disabled" => {
            "Avertissement : le repli de références intégré est désactivé pour cette exécution : {error}"
        }
        "cli.reference_tables_loaded" => {
            "{count} table(s) de référence masquée(s) chargée(s) pour la version de jeu {gameVersion} ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "Le repli de références intégré est désactivé pour cette exécution."
        }
        "cli.workspace_read_failed" => {
            "Erreur : impossible de lire le répertoire de l’espace de travail « {path} » : {error}"
        }
        "cli.path_skipped" => "Avertissement : « {path} » est ignoré : {error}",
        "cli.check_summary" => {
            "{errors} erreur(s), {warnings} avertissement(s), {infos} information(s) et {hints} diagnostic(s) indicatif(s) dans {fileCount} fichier(s) ; {parsedFileCount} fichier(s) analysé(s)."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp : démarrage de l’environnement TCP des plugins impossible : {error}"
        }
        _ => return None,
    })
}

fn italian(key: &str) -> Option<&'static str> {
    Some(match key {
        "log.effective_config" => "Configurazione vector-lsp effettiva: {summary}",
        "log.watch_registration_prepare_failed" => {
            "Impossibile preparare la registrazione del controllo dei file: {error}"
        }
        "log.file_watching_unavailable" => {
            "Il controllo dei file non è disponibile per “{baseUri}”: {error}"
        }
        "log.json_watch_retire_failed" => {
            "Impossibile rimuovere i controlli obsoleti dei file JSON; la prossima sincronizzazione dell’ambito JSON riproverà: {error}"
        }
        "log.workspace_scan_root_missing" => {
            "Scansione dell’area di lavoro non riuscita: l’inizializzazione non ha fornito un URI radice."
        }
        "log.workspace_scan_root_not_file" => {
            "Scansione dell’area di lavoro non riuscita: l’URI radice non è un percorso di file: {rootUri}"
        }
        "log.workspace_scan_failed" => "Scansione dell’area di lavoro non riuscita: {error}",
        "log.scan_path_uri_conversion_failed" => {
            "Impossibile convertire il percorso di scansione in un URI."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "Impossibile convertire il percorso di scansione dei riferimenti in un URI."
        }
        "log.scan_reference_root_failed" => "Scansione dei riferimenti non riuscita: {error}",
        "log.scan_reference_root_not_file" => {
            "L’URI radice dei riferimenti non è un percorso di file: {rootUri}"
        }
        "log.scan_task_failed" => {
            "L’attività di scansione dell’area di lavoro non è riuscita: {error}"
        }
        "log.workspace_scan_skipped" => {
            "Scansione dell’area di lavoro saltata per “{path}”: {reason}"
        }
        "log.workspace_indexed" => {
            "Indicizzati {count} file dell’area di lavoro; saltati {skippedCount} percorsi."
        }
        "log.scan_background_parse_failed" => {
            "Analisi durante la scansione in background non riuscita: {error}"
        }
        "log.schema_task_stopped" => {
            "Impossibile caricare lo schema perché l’attività in background si è interrotta: {error}"
        }
        "log.reference_tables_loaded" => {
            "Caricate {count} tabelle di riferimento nascoste per la versione del gioco {version} ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "Il fallback di riferimento incluso è disattivato: nessuna versione del gioco esplicita o deducibile."
        }
        "log.reference_fallback_disabled" => {
            "Il fallback di riferimento incluso è disattivato per questa sessione: {error}"
        }
        "log.reference_data_load_failed" => {
            "Impossibile caricare i dati di riferimento inclusi: {error}"
        }
        "log.did_close_restore_failed" => {
            "Il ripristino dal disco durante didClose non è riuscito per {uri}: {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Errore: la modalità single_shot richiede workspace_path nella configurazione."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Errore: impossibile avviare il runtime dei plugin: {error}"
        }
        "cli.schema_task_panicked" => {
            "Errore: l’attività dello schema ha generato un panic: {error}"
        }
        "cli.plugins_loaded" => "Caricati {count} file di plugin.",
        "cli.reference_fallback_disabled" => {
            "Avviso: il fallback di riferimento incluso è disattivato per questa esecuzione: {error}"
        }
        "cli.reference_tables_loaded" => {
            "Caricate {count} tabelle di riferimento nascoste per la versione del gioco {gameVersion} ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "Il fallback di riferimento incluso è disattivato per questa esecuzione."
        }
        "cli.workspace_read_failed" => {
            "Errore: impossibile leggere la cartella dell’area di lavoro “{path}”: {error}"
        }
        "cli.path_skipped" => "Avviso: “{path}” viene saltato: {error}",
        "cli.check_summary" => {
            "{errors} errori, {warnings} avvisi, {infos} informazioni e {hints} diagnostiche di suggerimento in {fileCount} file; analizzati {parsedFileCount} file."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp: impossibile avviare il runtime TCP dei plugin: {error}"
        }
        _ => return None,
    })
}

fn spanish(key: &str) -> Option<&'static str> {
    Some(match key {
        "log.effective_config" => "Configuración efectiva de vector-lsp: {summary}",
        "log.watch_registration_prepare_failed" => {
            "No se pudo preparar el registro de vigilancia de archivos: {error}"
        }
        "log.file_watching_unavailable" => {
            "La vigilancia de archivos no está disponible para «{baseUri}»: {error}"
        }
        "log.json_watch_retire_failed" => {
            "No se pudieron retirar las vigilancias de archivos JSON obsoletas; la próxima sincronización del ámbito JSON volverá a intentarlo: {error}"
        }
        "log.workspace_scan_root_missing" => {
            "Error al examinar el espacio de trabajo: la inicialización no proporcionó una URI raíz."
        }
        "log.workspace_scan_root_not_file" => {
            "Error al examinar el espacio de trabajo: la URI raíz no es una ruta de archivo: {rootUri}"
        }
        "log.workspace_scan_failed" => "Error al examinar el espacio de trabajo: {error}",
        "log.scan_path_uri_conversion_failed" => {
            "No se pudo convertir la ruta de exploración en una URI."
        }
        "log.scan_reference_path_uri_conversion_failed" => {
            "No se pudo convertir la ruta de exploración de referencias en una URI."
        }
        "log.scan_reference_root_failed" => "Error al examinar las referencias: {error}",
        "log.scan_reference_root_not_file" => {
            "La URI raíz de referencias no es una ruta de archivo: {rootUri}"
        }
        "log.scan_task_failed" => "La tarea de exploración del espacio de trabajo falló: {error}",
        "log.workspace_scan_skipped" => {
            "Se omitió la exploración del espacio de trabajo para «{path}»: {reason}"
        }
        "log.workspace_indexed" => {
            "Se indexaron {count} archivos del espacio de trabajo; se omitieron {skippedCount} rutas."
        }
        "log.scan_background_parse_failed" => {
            "Error de análisis durante la exploración en segundo plano: {error}"
        }
        "log.schema_task_stopped" => {
            "No se pudo cargar el esquema porque se detuvo su tarea en segundo plano: {error}"
        }
        "log.reference_tables_loaded" => {
            "Se cargaron {count} tablas de referencia ocultas para la versión del juego {version} ({digest})."
        }
        "log.reference_fallback_no_version" => {
            "La referencia alternativa incluida está deshabilitada: no hay una versión del juego explícita ni deducible."
        }
        "log.reference_fallback_disabled" => {
            "La referencia alternativa incluida está deshabilitada para esta sesión: {error}"
        }
        "log.reference_data_load_failed" => {
            "No se pudieron cargar los datos de referencia incluidos: {error}"
        }
        "log.did_close_restore_failed" => {
            "La restauración desde disco al cerrar el documento falló para {uri}: {error}"
        }
        "cli.single_shot_workspace_required" => {
            "Error: el modo single_shot requiere workspace_path en la configuración."
        }
        "cli.plugin_runtime_startup_failed" => {
            "Error: no se pudo iniciar el entorno de ejecución de complementos: {error}"
        }
        "cli.schema_task_panicked" => {
            "Error: la tarea de esquema provocó un fallo interno: {error}"
        }
        "cli.plugins_loaded" => "Se cargaron {count} archivos de complementos.",
        "cli.reference_fallback_disabled" => {
            "Advertencia: la referencia alternativa incluida está deshabilitada para esta ejecución: {error}"
        }
        "cli.reference_tables_loaded" => {
            "Se cargaron {count} tablas de referencia ocultas para la versión del juego {gameVersion} ({checksum})."
        }
        "cli.reference_fallback_disabled_for_run" => {
            "La referencia alternativa incluida está deshabilitada para esta ejecución."
        }
        "cli.workspace_read_failed" => {
            "Error: no se puede leer el directorio del espacio de trabajo «{path}»: {error}"
        }
        "cli.path_skipped" => "Advertencia: se omite «{path}»: {error}",
        "cli.check_summary" => {
            "{errors} errores, {warnings} advertencias, {infos} informaciones y {hints} diagnósticos informativos en {fileCount} archivos; se analizaron {parsedFileCount} archivos."
        }
        "cli.tcp_plugin_runtime_startup_failed" => {
            "vector-lsp: no se pudo iniciar el entorno TCP de complementos: {error}"
        }
        _ => return None,
    })
}
