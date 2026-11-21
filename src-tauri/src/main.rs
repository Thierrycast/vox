// Sem console no Windows em release: o app vive na bandeja.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod audio;
mod autostart;
mod bridge;
mod commands;
mod config;
mod dictation;
mod mic_id;
mod narration;
mod paste;
mod presence;
mod reading;
mod recordings;
mod sounds;
mod stt_stream;
mod texto_falado;
mod tray;

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::api::SpeechApi;
use crate::config::Settings;
use crate::dictation::Dictation;
use crate::reading::Reader;
use crate::sounds::SoundBank;

pub struct AppState {
    settings: Mutex<Settings>,
    dictation: Dictation,
    reader: Reader,
    api: Arc<SpeechApi>,
    sounds: Arc<SoundBank>,
    /// Onde a leitura está agora, para a extensão de navegador acompanhar.
    bridge: Arc<bridge::BridgeState>,
    /// A fila de narração de agentes de CLI, e o que acorda o trabalhador.
    narration: Arc<narration::Narration>,
    /// O que aconteceu ao registrar os atalhos, para o painel poder contar.
    shortcut_report: Mutex<tray::ShortcutReport>,
}

// ---------------------------------------------------------------- comandos

/// Níveis para o HUD desenhar. Chamado a cada 50 ms enquanto grava.
///
/// Devolve também a fase, para o front não precisar de uma segunda chamada só
/// para saber se ainda está gravando.
#[tauri::command]
fn audio_levels(state: State<'_, AppState>) -> serde_json::Value {
    let session = state.dictation.session();
    let session = session.lock();
    serde_json::json!({
        "levels": session.levels(),
        "phase": match session.phase() {
            dictation::Phase::Idle => "idle",
            dictation::Phase::Recording => "recording",
            dictation::Phase::Transcribing => "transcribing",
        },
        "barCount": session.bar_count(),
        // Texto reconhecido até agora. Vazio quando o streaming está desligado
        // ou ainda não produziu nada — o HUD trata isso como "só a onda".
        "liveText": session.live_text(),
    })
}

/// Tempos que o desenho do HUD precisa respeitar.
///
/// Ficam aqui e não duplicados no JavaScript: eram dois lugares para mudar o
/// mesmo número, e divergir silenciosamente é exatamente como a onda começaria
/// a piscar sem ninguém saber por quê.
#[tauri::command]
fn hud_timings() -> serde_json::Value {
    // Esta é a primeira chamada que o hud.js faz ao carregar. Se ela aparece no
    // log, o front está vivo — sem isso, uma janela sem console não dá nenhuma
    // pista de que o JavaScript sequer executou.
    tracing::info!("front carregado: o HUD pediu os tempos");
    serde_json::json!({
        "pollMs": dictation::LEVEL_POLL_MS,
        "successHoldMs": dictation::SUCCESS_HOLD_MS,
        "barsNormal": dictation::BARS_NORMAL,
        "barsPushToTalk": dictation::BARS_PUSH_TO_TALK,
    })
}

/// O widget pintou um quadro depois de aparecer. Ver `presence`.
#[tauri::command]
fn hud_presence(request: u64) {
    presence::confirm(request);
}

/// A página desenhou o estado que o backend pediu antes de aparecer. Ver
/// `revelar_quando_pronto` em `dictation.rs`.
#[tauri::command]
fn hud_ready(pedido: u64) {
    dictation::confirmar_hud_pronto(pedido);
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().clone()
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
) -> Result<(), String> {
    settings.sanitize();

    // As posições do widget são da janela, não do painel. O painel manda o
    // `Settings` que carregou ao abrir; arrastar a pílula com ele aberto e depois
    // mexer em qualquer opção gravava de volta a posição velha.
    let anterior = {
        let vigente = state.settings.lock();
        settings.hud_position_dictation = vigente.hud_position_dictation;
        settings.hud_position_reading = vigente.hud_position_reading;
        vigente.clone()
    };

    settings.save().map_err(|err| err.to_string())?;

    // Aplica no ato o que muda comportamento agora, sem esperar um restart.
    state.sounds.set_enabled(settings.sounds_enabled);
    state.sounds.set_volume(settings.sounds_volume);
    if let Some(directory) = &settings.external_sounds_directory {
        state.sounds.load_external_dictation_sounds(directory);
    }

    // As outras janelas precisam saber: a cor de destaque vale para o realce da
    // palavra na pílula, e quem acabou de escolhê-la está olhando para ela. A
    // âncora do widget, porque o HUD desliga o arraste no ato.
    let _ = app.emit("vox://settings-changed", serde_json::json!({
        "theme_accent": settings.theme_accent,
        "hud_placement_dictation": settings.hud_placement_dictation,
        "hud_placement_reading": settings.hud_placement_reading,
    }));

    reaplicar_ancora_do_widget(&app, &anterior, &settings);

    *state.settings.lock() = settings;
    Ok(())
}

/// Toca um aviso para a pessoa ouvir o volume que acabou de escolher.
///
/// Sem isto o controle de volume seria um número no escuro: só dá para ajustar
/// o que se ouve, e esperar o próximo erro acontecer para saber se ficou bom não
/// é ajuste, é adivinhação.
#[tauri::command]
fn preview_sound(state: State<'_, AppState>) {
    state.sounds.play(sounds::Cue::DictationSuccess);
}

/// Liga e desliga a legenda guiada, e ajusta a janela ao novo tamanho.
///
/// O front chama isto **antes** de abrir e **depois** de fechar: crescendo, a
/// janela precisa já caber o card que vai crescer dentro dela; encolhendo, ela
/// só pode encolher quando a animação terminou, senão o texto some cortado em
/// vez de recolher.
#[tauri::command]
fn set_reading_captions(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    {
        let mut settings = state.settings.lock();
        settings.reading_captions = enabled;
        if let Err(err) = settings.save() {
            tracing::warn!(?err, "não deu para guardar a preferência de legenda");
        }
    }

    dictation::shape_hud(&app, if enabled {
        dictation::HudShape::ColumnCaptions
    } else {
        dictation::HudShape::Column
    });
}

/// Reescreve um texto para a prévia do painel.
///
/// O painel precisa mostrar o efeito de um preset antes de a pessoa confiar nele
/// para o ditado do dia a dia — descrever "tira vício de linguagem" não é a
/// mesma coisa que ver a própria fala virar outra.
#[tauri::command]
async fn rewrite_preview(
    state: State<'_, AppState>,
    text: String,
    preset: String,
    intensity: u8,
) -> Result<serde_json::Value, String> {
    let modelo = state.settings.lock().stt_rewrite_model.clone();

    let resultado = state
        .api
        .rewrite_text(&text, &preset, intensity, modelo.as_deref())
        .await
        .map_err(|err| err.to_string())?;

    Ok(serde_json::json!({
        "text": resultado.text,
        "rewritten": {
            "applied": resultado.rewritten.applied,
            "elapsed_ms": resultado.rewritten.elapsed_ms,
            "error": resultado.rewritten.error,
            "skipped": resultado.rewritten.skipped,
        },
    }))
}

/// Os moldes de reescrita que o servidor conhece.
#[tauri::command]
async fn rewrite_presets(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.api.rewrite_presets().await.map_err(|err| err.to_string())
}

/// O catálogo de comandos com a combinação de cada um e o que falhou.
///
/// O painel monta a lista com isto em vez de guardar uma cópia: comando novo
/// aparece lá sem uma versão nova da interface, e a falha de registro aparece
/// junto do campo que a causou.
#[tauri::command]
fn command_catalog(state: State<'_, AppState>) -> Vec<serde_json::Value> {
    catalogo_atual(&state)
}

/// Liga e desliga a resposta do Vox aos comandos.
///
/// Desligado ele solta os atalhos globais e a ponte passa a recusar — e continua
/// na bandeja, que é o ponto: sumir do sistema quando se quer só um intervalo
/// obrigaria a procurar o app para tê-lo de volta.
#[tauri::command]
fn set_service_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    {
        let mut settings = state.settings.lock();
        if settings.service_enabled == enabled {
            return;
        }
        settings.service_enabled = enabled;
        if let Err(err) = settings.save() {
            tracing::warn!(?err, "não deu para guardar o estado do serviço");
        }
    }

    aplicar_estado_do_servico(&app, enabled);
}

/// Faz valer o interruptor: atalhos, o que estava em curso, e a bandeja.
fn aplicar_estado_do_servico(app: &AppHandle, enabled: bool) {
    if enabled {
        let report = register_shortcuts(app);
        *app.state::<AppState>().shortcut_report.lock() = report;
        tracing::info!("vox ativo");
    } else {
        if let Err(err) = app.global_shortcut().unregister_all() {
            tracing::warn!(?err, "não deu para soltar os atalhos");
        }

        // Desligar com a voz no ar deixaria o áudio tocando sem nenhum atalho
        // para pará-lo — o widget ainda tem o botão, mas quem desliga o serviço
        // não está olhando para ele.
        let state = app.state::<AppState>();
        state.reader.stop(app);
        state.bridge.clear();
        state.dictation.cancel(app);
        tracing::info!("vox em pausa: atalhos soltos e ponte recusando");
    }

    tray::refresh(app, enabled);
}

/// Liga e desliga a partida junto com o Windows.
#[tauri::command]
fn set_autostart(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    autostart::set(enabled)?;

    let mut settings = state.settings.lock();
    settings.start_with_windows = enabled;
    settings.save().map_err(|err| err.to_string())
}

/// Aplica os atalhos que estão gravados agora, sem reiniciar o app.
///
/// Solta tudo e registra de novo. Isso resolve duas coisas de uma vez: a
/// combinação nova passa a valer na hora, e o que estava tomado por outro
/// programa é testado outra vez — a disputa muda quando o outro app fecha, e
/// antes a única saída era reiniciar o Vox para descobrir.
#[tauri::command]
fn reapply_shortcuts(app: AppHandle, state: State<'_, AppState>) -> Vec<serde_json::Value> {
    if let Err(err) = app.global_shortcut().unregister_all() {
        tracing::warn!(?err, "não deu para soltar os atalhos antes de registrar de novo");
    }

    let report = register_shortcuts(&app);
    let falhas = report.commands.iter().filter(|info| info.failure.is_some()).count();
    tracing::info!(falhas, "atalhos reaplicados");

    *state.shortcut_report.lock() = report;
    catalogo_atual(&state)
}

fn catalogo_atual(state: &AppState) -> Vec<serde_json::Value> {
    let escolhas = state.settings.lock().shortcuts.clone();
    let falhas = state.shortcut_report.lock().clone();

    commands::Command::ALL
        .iter()
        .map(|comando| {
            let falha = falhas
                .commands
                .iter()
                .find(|info| info.id == comando.id())
                .and_then(|info| info.failure.clone());

            serde_json::json!({
                "id": comando.id(),
                "label": comando.label(),
                "hint": comando.hint(),
                "default_binding": comando.default_binding(),
                "binding": escolhas.get(comando.id()).cloned()
                    .unwrap_or_else(|| comando.default_binding().to_string()),
                "failure": falha,
            })
        })
        .collect()
}

/// Endereço da API, para o painel dizer para onde o áudio vai.
#[tauri::command]
fn api_base_url(state: State<'_, AppState>) -> String {
    config::base_url(&state.settings.lock())
}

/// Caminho do arquivo de preferências.
#[tauri::command]
fn settings_path() -> String {
    config::settings_path().to_string_lossy().to_string()
}

/// Esquece a posição guardada e a âncora do widget: volta ao padrão de origem.
#[tauri::command]
fn reset_hud_position(app: AppHandle, state: State<'_, AppState>) {
    {
        let mut settings = state.settings.lock();
        settings.hud_position_dictation = None;
        settings.hud_position_reading = None;
        settings.hud_placement_dictation = config::HudPlacement::Livre;
        settings.hud_placement_reading = config::HudPlacement::Livre;
        if let Err(err) = settings.save() {
            tracing::warn!(?err, "não deu para esquecer a posição do widget");
        }
    }

    // Só reposiciona se a janela estiver na tela: escondida, ela volta ao padrão
    // sozinha na próxima vez que aparecer. E reposiciona **no papel que está
    // exercendo** — mandar a barra do ditado para a posição da coluna seria o
    // mesmo defeito que este comando existe para desfazer.
    let visivel = app
        .get_webview_window("hud")
        .and_then(|janela| janela.is_visible().ok())
        .unwrap_or(false);
    if !visivel {
        return;
    }
    let forma = match dictation::current_role() {
        Some(dictation::HudRole::Dictation) => dictation::HudShape::Bar,
        Some(dictation::HudRole::Reading) => {
            if state.settings.lock().reading_captions {
                dictation::HudShape::ColumnCaptions
            } else {
                dictation::HudShape::Column
            }
        }
        None => return,
    };
    dictation::forget_role();
    dictation::shape_hud(&app, forma);
}

/// Abre o painel de preferências.
#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings(&app);
}

/// Reposiciona o widget na tela quando a âncora de um papel acabou de mudar.
///
/// Escondida, a janela nem precisa — `shape_hud` lê a preferência na próxima
/// vez que aparecer. Visível e parada no lugar antigo, o painel teria dito que
/// mudou e nada na tela teria acontecido.
fn reaplicar_ancora_do_widget(
    app: &AppHandle,
    anterior: &crate::config::Settings,
    nova: &crate::config::Settings,
) {
    if anterior.hud_placement_dictation == nova.hud_placement_dictation
        && anterior.hud_placement_reading == nova.hud_placement_reading
    {
        return;
    }
    let Some(window) = app.get_webview_window("hud") else { return };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    match dictation::current_role() {
        Some(dictation::HudRole::Dictation) => {
            dictation::shape_hud(app, dictation::HudShape::Bar);
        }
        Some(dictation::HudRole::Reading) => {
            let com_legenda = app.state::<AppState>().settings.lock().reading_captions;
            dictation::shape_hud(app, if com_legenda {
                dictation::HudShape::ColumnCaptions
            } else {
                dictation::HudShape::Column
            });
        }
        None => {}
    }
}

/// Mostra o widget em repouso, sem iniciar ditado ou leitura — ou esconde,
/// se ele já estava assim. É um alternador, e não um "sempre mostrar": o
/// mesmo atalho que traz o widget à tela precisa ser o que tira, senão ele
/// fica preso lá sem nenhuma forma de fechar.
///
/// Só alterna quando não há nada acontecendo de verdade. Com um ditado ou
/// uma leitura em curso — ou só concluída/falha, mas ainda na tela — o
/// widget já está lá por um motivo: apertar o atalho de novo não faz nada,
/// em vez de trocar a forma de quem está trabalhando por uma barra de
/// repouso.
///
/// Usada tanto pelo comando do painel (`show_floating_widget`) quanto pelo
/// atalho global `Ctrl+Alt+V` (`Command::ShowWidget`) — as duas entradas
/// precisam do mesmo alternador, senão uma delas continua com o bug antigo
/// de "trava na tela sem jeito de tirar".
fn toggle_floating_widget(app: &AppHandle) {
    let state = app.state::<AppState>();
    let ocupado = state.dictation.session().lock().phase() != dictation::Phase::Idle
        // Qualquer coisa diferente de `Idle` — incluindo `Complete` e
        // `Failed` — significa que a leitura ainda tem HUD próprio na tela;
        // só `stop()` (chamado à parte) volta o estado a `Idle` e esconde.
        || state.reader.state() != reading::ReadingState::Idle;

    // Com algo em curso o atalho não mexe em nada. O widget em curso já está
    // na tela por um motivo — e o caminho antigo chamava `show_idle_hud`,
    // que trocava a forma: a pílula da leitura virava uma barra de repouso
    // com a voz ainda falando, perdendo o transporte no meio da leitura.
    if ocupado {
        return;
    }

    let ocioso_visivel = app
        .get_webview_window("hud")
        .and_then(|janela| janela.is_visible().ok())
        .unwrap_or(false)
        && dictation::current_role() == Some(dictation::HudRole::Dictation);
    if ocioso_visivel {
        dictation::hide_hud(app, dictation::HudRole::Dictation);
        return;
    }

    dictation::show_idle_hud(app);
}

/// Mesma lógica de `toggle_floating_widget`, exposta ao painel de
/// preferências como comando Tauri.
#[tauri::command]
fn show_floating_widget(app: AppHandle) {
    toggle_floating_widget(&app);
}

/// Reinicia o processo do Vox.
///
/// Mesmo caminho do item "Reiniciar" da bandeja — existe aqui para o painel de
/// preferências oferecer o mesmo botão sem precisar sair da UI e ir até o
/// ícone do sistema.
#[tauri::command]
fn restart_app(app: AppHandle) {
    tracing::info!("reiniciando pelo painel de preferências");
    app.restart();
}

/// A bandeja alterna pelo mesmo caminho do painel.
pub fn alternar_servico(app: &AppHandle, enabled: bool) {
    {
        let state = app.state::<AppState>();
        let mut settings = state.settings.lock();
        settings.service_enabled = enabled;
        if let Err(err) = settings.save() {
            tracing::warn!(?err, "não deu para guardar o estado do serviço");
        }
    }
    aplicar_estado_do_servico(app, enabled);
}

/// A bandeja abre o mesmo painel; o comando existe para o front, este para ela.
pub fn open_settings_from_tray(app: &AppHandle) {
    show_settings(app);
}

fn show_settings(app: &AppHandle) {
    let Some(window) = app.get_webview_window("settings") else {
        tracing::error!("janela de preferências não encontrada");
        return;
    };
    raise(&window);
}

#[tauri::command]
fn list_input_devices() -> Result<Vec<audio::InputDevice>, String> {
    audio::list_input_devices().map_err(|err| err.to_string())
}

/// Backup local de áudio — ver `recordings.rs` pro porquê.
#[tauri::command]
fn list_recordings() -> Result<Vec<recordings::RecordingMeta>, String> {
    recordings::list().map_err(|err| err.to_string())
}

#[tauri::command]
async fn retry_recording(
    id: String,
    state: State<'_, AppState>,
) -> Result<recordings::RecordingMeta, String> {
    let (model, prompt) = {
        let settings = state.settings.lock();
        (settings.transcription_model.clone(), config::transcription_prompt(&settings))
    };
    recordings::retry(&id, &state.api, &model, &prompt)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn save_recording(id: String) -> Result<(), String> {
    recordings::mark_saved(&id).map_err(|err| err.to_string())
}

#[tauri::command]
fn delete_recording(id: String) -> Result<(), String> {
    recordings::delete(&id).map_err(|err| err.to_string())
}

#[tauri::command]
fn open_recordings_folder() -> Result<(), String> {
    recordings::open_folder().map_err(|err| err.to_string())
}

#[tauri::command]
async fn api_health(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.api.health().await.map_err(|err| err.to_string())
}

/// Testa um endereço de servidor sem tocar no cliente já em uso — é assim que
/// o painel confere o que está escrito no campo antes de salvar. Cliente
/// avulso, descartado no fim da chamada: o de verdade só troca com um
/// reinício (ver `restart_app`), porque `SpeechApi` é montado uma vez na
/// partida e o resto do app guarda um `Arc` para ele.
#[tauri::command]
async fn test_server_connection(
    base_url: String,
    user: String,
    password: String,
) -> Result<serde_json::Value, String> {
    let credentials = if user.trim().is_empty() {
        None
    } else {
        Some(api::Credentials { username: user, password })
    };
    let cliente = api::SpeechApi::new(&base_url, credentials).map_err(|err| err.to_string())?;
    cliente.health().await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn api_voices(state: State<'_, AppState>) -> Result<api::VoiceCatalog, String> {
    state.api.voices().await.map_err(|err| err.to_string())
}

#[tauri::command]
fn cancel_dictation(app: AppHandle, state: State<'_, AppState>) {
    state.dictation.cancel(&app);
}

/// Traz a janela de leitura para a frente.
///
/// Chamado ao clicar no tempo na pílula flutuante: o gesto natural de "quero
/// ver onde estou" é tocar no relógio.
#[tauri::command]
fn show_reader(app: AppHandle) {
    if let Some(window) = app.get_webview_window("reader") {
        raise(&window);
    }
}

/// Traz uma janela para a frente de verdade.
///
/// `show()` + `set_focus()` não bastam no Windows: um processo que não está em
/// primeiro plano não consegue roubar o foco, e o sistema troca isso por um
/// piscar na barra de tarefas. A janela abre — atrás de tudo. Foi por isso que a
/// leitura pareceu não ter destaque de texto: ele estava lá, numa janela que
/// ninguém viu.
///
/// O contorno é o de sempre: marcar como sempre-no-topo, mostrar, e desmarcar
/// em seguida. O sistema honra o `set_always_on_top` sem exigir foreground, e a
/// janela sobe. Desmarcar logo depois evita que ela fique por cima do trabalho
/// da pessoa para sempre.
fn raise(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(true);
    let _ = crate::presence::reveal(window);
    let _ = window.unminimize();
    let _ = window.set_focus();
    let _ = window.set_always_on_top(false);
}

/// O front informa onde a fala está dentro do trecho corrente.
///
/// Só a janela de leitura sabe isto: é ela que tem o elemento de áudio. O
/// backend guarda para a extensão de navegador poder desenhar o destaque na
/// página original — a mesma informação, dois lugares desenhando.
#[tauri::command]
fn reading_progress(state: State<'_, AppState>, index: usize, ratio: f32) {
    state.bridge.set_progress(index, ratio);
}

/// O front informa que a reprodução mudou de estado.
///
/// O nome vem como texto porque é o mesmo vocabulário que já viaja nos eventos
/// `vox://reading`; traduzir aqui mantém um dicionário só.
#[tauri::command]
fn reading_state(state: State<'_, AppState>, name: String) {
    use reading::ReadingState;

    let novo = match name.as_str() {
        "generating" => ReadingState::Generating,
        "playing" => ReadingState::Playing,
        "paused" => ReadingState::Paused,
        "complete" => ReadingState::Complete,
        "failed" => ReadingState::Failed,
        "idle" => ReadingState::Idle,
        outro => {
            tracing::warn!(estado = outro, "estado de leitura desconhecido");
            return;
        }
    };
    state.reader.sync_state(novo);
}

/// O front informa qual trecho entrou em reprodução.
#[tauri::command]
fn reading_cursor(state: State<'_, AppState>, index: usize) {
    state.reader.set_cursor(index);
}

#[tauri::command]
fn reading_finished(app: AppHandle, state: State<'_, AppState>) {
    state.reader.finished(&app);
    // Zera a ponte junto: é por `segments` voltar a zero que a extensão sabe
    // que acabou e pode apagar o destaque. Sem isto ela ficaria consultando a
    // posição para sempre, e a última frase ficaria pintada na página.
    state.bridge.clear();
}

#[tauri::command]
fn stop_reading(app: AppHandle, state: State<'_, AppState>) {
    state.reader.stop(&app);
    state.bridge.clear();
}

#[tauri::command]
fn toggle_reading(app: AppHandle, state: State<'_, AppState>) {
    match state.reader.state() {
        reading::ReadingState::Playing => state.reader.pause(&app),
        reading::ReadingState::Paused => state.reader.resume(&app),
        _ => {}
    }
}

/// Lê em voz alta o texto passado (ou o que estiver na área de transferência).
#[tauri::command]
async fn read_text(
    app: AppHandle,
    state: State<'_, AppState>,
    text: Option<String>,
) -> Result<(), String> {
    // Sem texto explícito, copia o que estiver selecionado no app em foco.
    // Numa thread de bloqueio: ver o comentário em `toggle_reading_shortcut`.
    let text = match text {
        Some(value) if !value.trim().is_empty() => value,
        _ => tauri::async_runtime::spawn_blocking(paste::copy_selection)
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| err.to_string())?,
    };

    if text.trim().is_empty() {
        return Err("Nada selecionado para ler.".into());
    }

    let (voice, speed, prebuffer) = {
        let settings = state.settings.lock();
        (settings.voice.clone(), settings.speed, settings.prebuffer_ratio)
    };

    state
        .reader
        .speak(app, text, voice, speed, prebuffer)
        .await
        .map_err(|err| err.to_string())
}

// ---------------------------------------------------------------- atalhos

/// Ctrl+Shift+D — ditado. Ctrl+Shift+S — ler a seleção.
///
/// Registra os dois de forma independente e **devolve o que falhou** em vez de
/// abortar no primeiro erro. Atalho global é recurso disputado: se outro app já
/// tomou a combinação, o registro falha em silêncio e a tecla simplesmente não
/// faz nada — o usuário conclui que o Vox está quebrado.
///
/// Aqui a falha vira uma linha no menu da bandeja, e o que funcionou continua
/// funcionando: perder a leitura não é motivo para perder o ditado também.
/// Registra tudo o que o catálogo declara, e devolve o que falhou.
///
/// Cada comando é registrado por conta própria: perder a leitura porque o
/// navegador tomou uma combinação não é motivo para perder o ditado junto. O que
/// falhar vira uma linha no menu da bandeja — atalho global que não registra é
/// silencioso por natureza, e sem isso a tecla simplesmente não faz nada e a
/// pessoa conclui que o app está quebrado.
fn register_shortcuts(app: &AppHandle) -> tray::ShortcutReport {
    let escolhas = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock();
        settings.shortcuts.clone()
    };

    let mut report = tray::ShortcutReport::default();
    let mut registradas: Vec<(String, &'static str)> = Vec::new();

    for comando in commands::Command::ALL {
        let texto = escolhas
            .get(comando.id())
            .cloned()
            .unwrap_or_else(|| comando.default_binding().to_string());

        let mut info = commands::CommandInfo {
            id: comando.id(),
            label: comando.label(),
            hint: comando.hint(),
            default_binding: comando.default_binding(),
            binding: texto.clone(),
            failure: None,
        };

        // Campo vazio é escolha, não erro: quem não quer o comando ocupando uma
        // combinação global do sistema apaga o campo no painel.
        if texto.trim().is_empty() {
            report.commands.push(info);
            continue;
        }

        // Duas vezes a mesma combinação: o segundo registro falharia com um erro
        // do sistema que não explica nada. Dizer qual comando já a tem explica.
        let normalizada = texto.trim().to_lowercase();
        if let Some((_, dono)) = registradas.iter().find(|(usada, _)| *usada == normalizada) {
            info.failure = Some(format!("a mesma combinação de \"{dono}\""));
            report.commands.push(info);
            continue;
        }

        let Some(atalho) = parse_shortcut(&texto) else {
            info.failure = Some(format!("combinação inválida: {texto}"));
            report.commands.push(info);
            continue;
        };

        let handle = app.clone();
        let resultado = app.global_shortcut().on_shortcut(atalho, move |_app, _sc, evento| {
            if evento.state() != ShortcutState::Pressed {
                return;
            }
            executar(handle.clone(), comando);
        });

        if let Err(err) = resultado {
            tracing::error!(?err, comando = comando.id(), atalho = %texto, "atalho indisponível");
            info.failure = Some(motivo_curto(&err));
        } else {
            registradas.push((normalizada, comando.label()));
        }

        report.commands.push(info);
    }

    report
}

/// O que cada comando faz.
///
/// Fica separado do registro porque a ação é do app e o registro é do sistema:
/// juntos, mexer numa das duas coisas obrigava a reler a outra.
fn executar(app: AppHandle, comando: commands::Command) {
    use commands::Command;

    match comando {
        Command::Dictate => {
            tauri::async_runtime::spawn(async move { toggle_dictation(app).await });
        }
        Command::CancelDictation => {
            let state = app.state::<AppState>();
            state.dictation.cancel(&app);
        }
        Command::ReadSelection => {
            tauri::async_runtime::spawn(async move { toggle_reading_shortcut(app).await });
        }
        Command::TogglePlayback => {
            // Diferente de `ReadSelection`: aqui nada começa. Quem quer só
            // pausar não pode correr o risco de iniciar uma leitura nova da
            // seleção que por acaso estava na tela.
            let state = app.state::<AppState>();
            match state.reader.state() {
                reading::ReadingState::Playing => state.reader.pause(&app),
                reading::ReadingState::Paused => state.reader.resume(&app),
                outro => tracing::debug!(estado = ?outro, "nada tocando; pausa ignorada"),
            }
        }
        Command::StopReading => {
            let state = app.state::<AppState>();
            state.reader.stop(&app);
            state.bridge.clear();
        }
        Command::ToggleCaptions => {
            let ligada = {
                let state = app.state::<AppState>();
                let atual = state.settings.lock().reading_captions;
                !atual
            };
            // O front é quem anima a abertura; mandar o evento em vez de mexer
            // na janela daqui mantém a animação e a preferência num caminho só.
            let _ = app.emit("vox://toggle-captions", serde_json::json!({ "enabled": ligada }));
        }
        Command::ShowWidget => toggle_floating_widget(&app),
        Command::OpenSettings => show_settings(&app),
    }
}

/// Interpreta `"Ctrl+Shift+D"` e devolve o atalho do Tauri.
///
/// Escrito à mão em vez de usar `FromStr`: a versão do plugin em uso não expõe
/// um parser estável, e a lista de teclas que faz sentido para atalho global é
/// curta. Devolve `None` para o chamador poder reportar a combinação inválida
/// em vez de o app subir sem o atalho e sem explicação.
fn parse_shortcut(texto: &str) -> Option<Shortcut> {
    let mut modificadores = Modifiers::empty();
    let mut tecla = None;

    for parte in texto.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match parte.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modificadores |= Modifiers::CONTROL,
            "shift" => modificadores |= Modifiers::SHIFT,
            "alt" => modificadores |= Modifiers::ALT,
            "super" | "win" | "meta" | "cmd" => modificadores |= Modifiers::SUPER,
            outra => tecla = code_from_str(outra),
        }
    }

    // Sem modificador, um atalho global roubaria a tecla do sistema inteiro.
    if modificadores.is_empty() {
        return None;
    }
    Some(Shortcut::new(Some(modificadores), tecla?))
}

fn code_from_str(nome: &str) -> Option<Code> {
    Some(match nome {
        "a" => Code::KeyA, "b" => Code::KeyB, "c" => Code::KeyC, "d" => Code::KeyD,
        "e" => Code::KeyE, "f" => Code::KeyF, "g" => Code::KeyG, "h" => Code::KeyH,
        "i" => Code::KeyI, "j" => Code::KeyJ, "k" => Code::KeyK, "l" => Code::KeyL,
        "m" => Code::KeyM, "n" => Code::KeyN, "o" => Code::KeyO, "p" => Code::KeyP,
        "q" => Code::KeyQ, "r" => Code::KeyR, "s" => Code::KeyS, "t" => Code::KeyT,
        "u" => Code::KeyU, "v" => Code::KeyV, "w" => Code::KeyW, "x" => Code::KeyX,
        "y" => Code::KeyY, "z" => Code::KeyZ,
        "space" => Code::Space,
        "f1" => Code::F1, "f2" => Code::F2, "f3" => Code::F3, "f4" => Code::F4,
        "f5" => Code::F5, "f6" => Code::F6, "f7" => Code::F7, "f8" => Code::F8,
        "f9" => Code::F9, "f10" => Code::F10, "f11" => Code::F11, "f12" => Code::F12,
        _ => return None,
    })
}

/// Encurta o erro para caber num item de menu.
fn motivo_curto(err: &impl std::fmt::Display) -> String {
    let texto = err.to_string();
    // A causa quase sempre é outra aplicação já ter tomado a combinação; dizer
    // isso ajuda mais que repetir o código de erro do sistema.
    if texto.to_lowercase().contains("registered") || texto.contains("1409") {
        return "já em uso por outro app".to_string();
    }
    texto.chars().take(60).collect()
}

/// Lê em voz alta o que estiver na área de transferência.
///
/// Diferente do atalho, que copia a seleção: aqui o conteúdo já está lá. Serve
/// ao item de menu da bandeja e ao caso de o atalho global ter falhado.
pub async fn read_clipboard_aloud(app: AppHandle) -> anyhow::Result<()> {
    let texto = paste::Clipboard::new()?.text()?;
    if texto.trim().is_empty() {
        tracing::info!("área de transferência vazia; nada a ler");
        return Ok(());
    }

    let (voice, speed, prebuffer) = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock();
        (settings.voice.clone(), settings.speed, settings.prebuffer_ratio)
    };

    let handle = app.clone();
    let state = app.state::<AppState>();
    state.reader.speak(handle, texto, voice, speed, prebuffer).await
}

/// O mesmo atalho serve para os três estados: começa, pausa e retoma.
///
/// Um atalho por verbo obrigaria a decorar três combinações para uma coisa só;
/// aqui a tecla significa "mexe na leitura" e o app sabe o que isso quer dizer
/// no momento.
async fn toggle_reading_shortcut(app: AppHandle) {
    use reading::ReadingState;

    let current = {
        let state = app.state::<AppState>();
        state.reader.state()
    };

    tracing::info!(estado = ?current, "atalho de leitura");

    match current {
        ReadingState::Playing => {
            let state = app.state::<AppState>();
            state.reader.pause(&app);
        }
        ReadingState::Paused => {
            let state = app.state::<AppState>();
            state.reader.resume(&app);
        }
        // Parada, terminada ou falhada: começa uma leitura nova da seleção.
        _ => {
            // Fora da thread do runtime: copiar a seleção solta modificadores,
            // manda teclas e espera a área de transferência mudar — até ~700 ms
            // de bloqueio. Feito aqui dentro, isso congelaria também a ponte da
            // extensão, que vive no mesmo runtime e é consultada a cada 120 ms.
            let copia = tauri::async_runtime::spawn_blocking(paste::copy_selection_with_source).await;

            let (selection, origem) = match copia {
                Ok(Ok((text, origem))) if !text.trim().is_empty() => (text, origem),
                Ok(Ok(_)) => {
                    // Nem seleção nem área de transferência: abre o leitor para
                    // a pessoa colar à mão, em vez de não fazer nada e parecer
                    // que o atalho não funcionou.
                    tracing::info!("nada para ler; abrindo o leitor vazio");
                    show_reader(app.clone());
                    return;
                }
                Ok(Err(err)) => {
                    tracing::error!(?err, "não foi possível ler a seleção");
                    show_reader(app.clone());
                    return;
                }
                Err(err) => {
                    tracing::error!(?err, "a tarefa de cópia não terminou");
                    show_reader(app.clone());
                    return;
                }
            };

            if origem == paste::SelectionSource::ClipboardFallback {
                // Terminais tratam Ctrl+C como interrupção, não cópia — a
                // seleção nunca chega à área de transferência. Dizer isso evita
                // o usuário achar que o app leu o texto errado por bug.
                tracing::info!(
                    "a cópia da seleção não trouxe nada;                      lendo o que já estava na área de transferência                      (terminais tratam Ctrl+C como interrupção)"
                );
            }

            let (voice, speed, prebuffer) = {
                let state = app.state::<AppState>();
                let settings = state.settings.lock();
                (settings.voice.clone(), settings.speed, settings.prebuffer_ratio)
            };

            let handle = app.clone();
            let state = app.state::<AppState>();
            if let Err(err) = state
                .reader
                .speak(handle, selection, voice, speed, prebuffer)
                .await
            {
                tracing::error!(?err, "leitura falhou");
            }
        }
    }
}

/// Um atalho só: aperta para começar, aperta de novo para finalizar.
async fn toggle_dictation(app: AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().clone();

    if state.dictation.session().lock().phase() == dictation::Phase::Recording {
        let Some(recording) = state.dictation.stop_recording() else { return };
        if let Err(err) = state.dictation.deliver(app.clone(), settings, recording).await {
            tracing::error!(?err, "ditado falhou");
        }
        // Deixa a mensagem na tela pelo tempo que o Raycast usa, depois some —
        // a não ser que o widget tenha sido pedido de novo nesse meio-tempo.
        let epoch = dictation::hud_epoch();
        tokio::time::sleep(std::time::Duration::from_millis(dictation::SUCCESS_HOLD_MS)).await;
        dictation::hide_hud_since(&app, dictation::HudRole::Dictation, epoch);
        // Mesma proteção do HUD: um novo ditado ou leitura começado durante a
        // espera já reabriu a janela de texto ao vivo por conta própria —
        // escondê-la agora, sem olhar o epoch, derrubaria essa sessão nova.
        if dictation::hud_epoch() == epoch {
            dictation::hide_live_window(&app);
        }
        return;
    }

    if let Err(err) = state.dictation.start(&app, &settings, false) {
        tracing::error!(?err, "não foi possível iniciar o ditado");
    }
}

// ------------------------------------------------------------------ ponte

/// O que a extensão pode pedir ao app.
///
/// Deliberadamente curto: ler um texto e parar. Nada aqui lê a área de
/// transferência, abre janela ou muda preferência — quanto menor a superfície,
/// menos importa quem conseguiu falar com a porta.
struct AcoesDaPonte {
    app: AppHandle,
}

impl bridge::Acoes for AcoesDaPonte {
    fn ler(&self, texto: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send>> {
        let app = self.app.clone();

        Box::pin(async move {
            // Pausado é pausado, venha o pedido de onde vier. Sem esta checagem a
            // extensão continuaria fazendo o computador falar com o Vox de folga.
            {
                let state = app.state::<AppState>();
                let ligado = state.settings.lock().service_enabled;
                if !ligado {
                    tracing::info!("pedido da extensão recusado: vox em pausa");
                    return Vec::new();
                }
            }

            let (voice, speed, prebuffer, limpar_com_modelo, normalizar, narrar_tabelas) = {
                let state = app.state::<AppState>();
                let settings = state.settings.lock();
                (
                    settings.voice.clone(),
                    settings.speed,
                    settings.prebuffer_ratio,
                    settings.reading_rewrite_enabled,
                    settings.normalize_before_reading,
                    settings.narrate_tables,
                )
            };

            // O texto é preparado aqui, e não dentro da fala, porque a divisão
            // precisa sair do texto limpo — e é esta lista que a extensão usa
            // para casar cada trecho com o pedaço do DOM que vai destacar.
            // Preparar de novo lá dentro poderia mudar os trechos debaixo de um
            // destaque já montado, e com a correção ligada custaria duas idas ao
            // modelo em vez de uma.
            let preparado = {
                let state = app.state::<AppState>();
                state
                    .reader
                    .prepare_to_read(&texto, limpar_com_modelo, normalizar, narrar_tabelas)
                    .await
            };

            // Divide aqui e devolve a mesma lista que vai ser falada. Dividir dos
            // dois lados daria listas diferentes na primeira abreviação ou
            // reticência.
            let segments = api::split_text(&preparado);
            {
                let state = app.state::<AppState>();
                state.bridge.set_segments(segments.clone());
            }

            let leitura = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = leitura.state::<AppState>();
                let handle = leitura.clone();
                if let Err(err) = state
                    .reader
                    .speak_prepared(handle, preparado, voice, speed, prebuffer)
                    .await
                {
                    tracing::error!(?err, "leitura pedida pela extensão falhou");
                }
            });

            segments
        })
    }

    fn servico_ligado(&self) -> bool {
        self.app.state::<AppState>().settings.lock().service_enabled
    }

    fn atalho_de_leitura(&self) -> String {
        let state = self.app.state::<AppState>();
        let settings = state.settings.lock();
        settings
            .shortcuts
            .get(commands::Command::ReadSelection.id())
            .cloned()
            .unwrap_or_else(|| commands::Command::ReadSelection.default_binding().to_string())
    }

    fn parar(&self) {
        let state = self.app.state::<AppState>();
        state.reader.stop(&self.app);
        state.bridge.clear();
    }

    fn pausar(&self) {
        self.app.state::<AppState>().reader.pause(&self.app);
    }

    fn retomar(&self) {
        self.app.state::<AppState>().reader.resume(&self.app);
    }

    fn preferencias(&self) -> serde_json::Value {
        let state = self.app.state::<AppState>();
        let settings = state.settings.lock();
        config::visao_da_ponte(&settings)
    }

    fn ajustar(&self, mudancas: serde_json::Value) -> Result<serde_json::Value, String> {
        let state = self.app.state::<AppState>();
        let (anterior, nova) = {
            let mut settings = state.settings.lock();
            let anterior = settings.clone();
            let nova = config::ajustar_pela_ponte(&anterior, &mudancas)?;
            if nova.start_with_windows != anterior.start_with_windows {
                autostart::set(nova.start_with_windows)?;
            }
            nova.save().map_err(|err| err.to_string())?;
            *settings = nova.clone();
            (anterior, nova)
        };

        state.sounds.set_enabled(nova.sounds_enabled);
        state.sounds.set_volume(nova.sounds_volume);

        let servico_mudou = nova.service_enabled != anterior.service_enabled;
        let atalhos_mudaram = nova.shortcuts != anterior.shortcuts && nova.service_enabled;
        if servico_mudou || atalhos_mudaram {
            let app = self.app.clone();
            let ligado = nova.service_enabled;
            let agendado = self.app.run_on_main_thread(move || {
                if servico_mudou {
                    aplicar_estado_do_servico(&app, ligado);
                    return;
                }
                if let Err(err) = app.global_shortcut().unregister_all() {
                    tracing::warn!(?err, "não deu para soltar os atalhos antes de registrar de novo");
                }
                let report = register_shortcuts(&app);
                *app.state::<AppState>().shortcut_report.lock() = report;
            });
            if let Err(err) = agendado {
                tracing::warn!(?err, "não deu para aplicar o ajuste da ponte na thread principal");
            }
        }

        let ancoras_mudaram = nova.hud_placement_dictation != anterior.hud_placement_dictation
            || nova.hud_placement_reading != anterior.hud_placement_reading;
        if ancoras_mudaram {
            let app = self.app.clone();
            let anterior_para_ancora = anterior.clone();
            let nova_para_ancora = nova.clone();
            if let Err(err) = self.app.run_on_main_thread(move || {
                reaplicar_ancora_do_widget(&app, &anterior_para_ancora, &nova_para_ancora);
            }) {
                tracing::warn!(?err, "não deu para reancorar o widget pela ponte na thread principal");
            }
        }

        let _ = self.app.emit("vox://settings-changed", serde_json::json!({
            "theme_accent": nova.theme_accent,
            "hud_placement_dictation": nova.hud_placement_dictation,
            "hud_placement_reading": nova.hud_placement_reading,
        }));
        let _ = self.app.emit("vox://preferencias-externas", ());
        tracing::info!(campos = ?mudancas.as_object().map(|mapa| mapa.keys().collect::<Vec<_>>()), "preferências ajustadas pela ponte");

        Ok(config::visao_da_ponte(&nova))
    }

    fn catalogo(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>> {
        let app = self.app.clone();
        Box::pin(async move {
            let api = app.state::<AppState>().api.clone();
            let vozes = match api.voices().await {
                Ok(catalogo) => serde_json::to_value(catalogo).unwrap_or_default(),
                Err(err) => serde_json::json!({ "error": err.to_string() }),
            };
            let presets = match api.rewrite_presets().await {
                Ok(lista) => lista,
                Err(err) => serde_json::json!({ "error": err.to_string() }),
            };
            let microfones = match tauri::async_runtime::spawn_blocking(audio::list_input_devices).await {
                Ok(Ok(lista)) => serde_json::to_value(lista).unwrap_or_default(),
                Ok(Err(err)) => serde_json::json!({ "error": err.to_string() }),
                Err(err) => serde_json::json!({ "error": err.to_string() }),
            };
            let comandos = catalogo_atual(&app.state::<AppState>());
            serde_json::json!({
                "voices": vozes,
                "rewrite_presets": presets,
                "input_devices": microfones,
                "commands": comandos,
                "transcription_models": [
                    { "value": "groq/whisper-large-v3-turbo", "label": "Whisper large v3 turbo" },
                    { "value": "vosk", "label": "Vosk — local" },
                ],
            })
        })
    }

    fn abrir_preferencias(&self) {
        let app = self.app.clone();
        if let Err(err) = self.app.run_on_main_thread(move || show_settings(&app)) {
            tracing::warn!(?err, "não deu para abrir as preferências pela ponte");
        }
    }

    fn tocar_aviso(&self) {
        self.app.state::<AppState>().sounds.play(sounds::Cue::DictationSuccess);
    }

    fn reiniciar(&self) {
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            tracing::info!("reiniciando a pedido da ponte");
            app.restart();
        });
    }
}

/// A ponte fala com a fila de narração por aqui.
///
/// Sem traço próprio, este módulo puxaria o app inteiro — e o teste da ponte,
/// que só precisa conferir o que foi pedido, teria que subir um `AppHandle`.
struct AcoesNarracaoDaPonte {
    app: AppHandle,
}

impl bridge::AcoesNarracao for AcoesNarracaoDaPonte {
    fn narrar(
        &self,
        pedido: bridge::PedidoNarracao,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, bridge::NarrarErro>> + Send>>
    {
        let app = self.app.clone();
        Box::pin(async move {
            let (limite, auto_speak, ligado) = {
                let state = app.state::<AppState>();
                let settings = state.settings.lock();
                (
                    settings.narration_queue_limit,
                    settings.narration_auto_speak,
                    settings.narration_enabled && settings.service_enabled,
                )
            };
            if !ligado {
                return Err(bridge::NarrarErro::Desligado);
            }

            // O modo do pedido vence; sem modo, vale o padrão do painel. É o
            // contrato que deixa o mod e o painel controlarem a mesma coisa
            // sem brigarem: o pedido carrega a escolha de quem chamou.
            let modo = narration::Modo::do_nome(pedido.mode.as_deref()).unwrap_or(if auto_speak {
                narration::Modo::Falar
            } else {
                narration::Modo::Segurar
            });

            let origem = pedido.source.unwrap_or_else(|| "agente".into());
            let state = app.state::<AppState>();
            let (id, posicao) = state
                .narration
                .enqueue(pedido.text, origem, modo, limite)
                .map_err(|_| bridge::NarrarErro::FilaCheia)?;
            tracing::info!(id, posicao, modo = modo.nome(), "narração enfileirada");
            Ok(serde_json::json!({
                "ok": true,
                "id": id,
                "position": posicao,
                "mode": modo.nome(),
            }))
        })
    }

    fn fila(&self) -> serde_json::Value {
        let state = self.app.state::<AppState>();
        let habilitada = state.settings.lock().narration_enabled;
        let corrente = state.narration.corrente().map(|item| item_json(&item));
        let pendentes: Vec<serde_json::Value> = state
            .narration
            .pendentes()
            .iter()
            .map(item_json)
            .collect();
        serde_json::json!({
            "enabled": habilitada,
            "service_enabled": state.settings.lock().service_enabled,
            "reading_state": format!("{:?}", state.reader.state()),
            "current": corrente,
            "items": pendentes,
        })
    }

    fn tocar(&self, id: u64) -> Option<serde_json::Value> {
        let state = self.app.state::<AppState>();
        match state.narration.request_play(id) {
            Ok(narration::Jogada::Tocando) => {
                Some(serde_json::json!({"ok": true, "action": "playing"}))
            }
            Ok(narration::Jogada::Promovido) => {
                Some(serde_json::json!({"ok": true, "action": "promoted"}))
            }
            Ok(narration::Jogada::JaTocando) => {
                Some(serde_json::json!({"ok": true, "action": "already"}))
            }
            Err(_) => None,
        }
    }

    fn cancelar(&self, id: u64) -> Option<serde_json::Value> {
        let state = self.app.state::<AppState>();
        match state.narration.cancel(id) {
            narration::Cancelamento::Removido => {
                Some(serde_json::json!({"ok": true, "was_current": false}))
            }
            narration::Cancelamento::Corrente => {
                // A fila tirou o item; parar a fala é a parte da ponte.
                state.reader.stop(&self.app);
                state.bridge.clear();
                Some(serde_json::json!({"ok": true, "was_current": true}))
            }
            narration::Cancelamento::Inexistente => None,
        }
    }

    fn pular(&self) -> bool {
        let state = self.app.state::<AppState>();
        let tinha = state.narration.skip();
        if tinha {
            state.reader.stop(&self.app);
            state.bridge.clear();
        }
        tinha
    }
}

fn item_json(item: &narration::Item) -> serde_json::Value {
    serde_json::json!({
        "id": item.id,
        "source": item.origem,
        "mode": item.modo.nome(),
        "state": item.estado,
        "preview": item.previa(),
    })
}

/// O trabalhador da fila de narração: um item por vez, do começo ao fim.
///
/// Dorme quando a fila está vazia e acorda com o `Notify` da própria fila.
/// Cada item: adapta o texto (preset `narracao` da speech-api), espera o play
/// quando o modo é segurar, fala pelo player da leitura, e só então libera o
/// topo para o próximo.
async fn rodar_narracao(app: AppHandle) {
    loop {
        let item = app.state::<AppState>().narration.begin_next();
        let Some(item) = item else {
            app.state::<AppState>().narration.wait().await;
            continue;
        };

        // A adaptação custa uma chamada de modelo, e é por isso que acontece
        // aqui e não no enqueue: cancelar um item que nunca chegou ao topo
        // não deve ter pago por ela.
        let fala = {
            let state = app.state::<AppState>();
            let modelo = state
                .settings
                .lock()
                .narration_model
                .clone()
                .filter(|valor| !valor.trim().is_empty());
            match state
                .api
                .rewrite_text(&item.original, "narracao", 2, modelo.as_deref())
                .await
            {
                Ok(resultado) if resultado.rewritten.applied => resultado.text,
                Ok(_) => {
                    tracing::info!("adaptação não aplicou; narrando o texto original");
                    item.original.clone()
                }
                Err(err) => {
                    tracing::warn!(?err, "adaptação falhou; narrando o texto original");
                    item.original.clone()
                }
            }
        };

        if fala.trim().is_empty() {
            tracing::info!(id = item.id, "nada para narrar");
            app.state::<AppState>().narration.finish(item.id);
            continue;
        }

        // Modo segurar: fica pronto e espera o comando de tocar — ou um
        // cancelamento, que tira o item do corrente e libera o topo.
        if item.modo == narration::Modo::Segurar {
            app.state::<AppState>().narration.mark(item.id, narration::Estado::Pronto);
            loop {
                if app.state::<AppState>().narration.take_play_request(item.id) {
                    break;
                }
                if !app.state::<AppState>().narration.is_corrente(item.id) {
                    break;
                }
                app.state::<AppState>().narration.wait().await;
            }
            if !app.state::<AppState>().narration.is_corrente(item.id) {
                continue;
            }
        }

        app.state::<AppState>().narration.mark(item.id, narration::Estado::Tocando);
        let (voz, velocidade, prebuffer) = {
            let state = app.state::<AppState>();
            let settings = state.settings.lock();
            (settings.voice.clone(), settings.speed, settings.prebuffer_ratio)
        };
        {
            let state = app.state::<AppState>();
            if let Err(err) = state
                .reader
                .speak(app.clone(), fala, voz, velocidade, prebuffer)
                .await
            {
                tracing::error!(?err, id = item.id, "narração falhou na síntese");
            }
        }

        // `speak` devolve quando o último trecho foi sintetizado, não quando
        // ele parou de tocar: quem toca é a janela de leitura. O topo só se
        // libera quando a fala de fato termina — completa, foi parada, ou
        // falhou — para que o item seguinte não comece por cima desta.
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let state = app.state::<AppState>();
            if !state.narration.is_corrente(item.id) {
                // Cancelado ou pulado por fora; quem parou a fala foi o pedido.
                break;
            }
            match state.reader.state() {
                reading::ReadingState::Complete
                | reading::ReadingState::Failed
                | reading::ReadingState::Idle => break,
                _ => {}
            }
        }
        app.state::<AppState>().narration.finish(item.id);
    }
}

// ---------------------------------------------------------------- partida

/// Acima disto o log vira histórico, e histórico atrapalha quem está lendo o
/// erro de agora. Um arquivo anterior é guardado; o resto se perde.
const LIMITE_DO_LOG: u64 = 2 * 1024 * 1024;

/// Abre o arquivo de log, girando o anterior se ele já ficou grande.
fn open_log_file() -> Option<std::fs::File> {
    let caminho = config::log_path();
    std::fs::create_dir_all(caminho.parent()?).ok()?;

    if matches!(std::fs::metadata(&caminho), Ok(dados) if dados.len() > LIMITE_DO_LOG) {
        let _ = std::fs::rename(&caminho, caminho.with_extension("log.old"));
    }

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&caminho)
        .ok()
}

/// Em release o processo não tem console: `windows_subsystem = "windows"` tira o
/// stdout, e um log que só existe na saída padrão desaparece justamente na
/// versão que roda no dia a dia. Por isso o arquivo é o destino principal, e a
/// saída padrão continua junto para quem roda em debug pelo terminal.
fn start_logging() {
    let filtro = tracing_subscriber::EnvFilter::try_from_env("VOX_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    match open_log_file() {
        Some(arquivo) => {
            let destino = Arc::new(arquivo).and(std::io::stdout);
            tracing_subscriber::fmt()
                .with_env_filter(filtro)
                .with_ansi(false)
                .with_writer(destino)
                .init();
        }
        None => {
            tracing_subscriber::fmt().with_env_filter(filtro).init();
            tracing::warn!(caminho = ?config::log_path(), "sem arquivo de log");
        }
    }
}

fn main() {
    start_logging();

    let settings = Settings::load();
    let base_url = config::base_url(&settings);
    let credentials = config::credentials(&settings);

    if credentials.is_none() {
        tracing::warn!(
            "sem VOX_API_USER/VOX_API_PASSWORD — se a API exigir basicAuth, virá 401"
        );
    }

    let api = Arc::new(
        SpeechApi::new(&base_url, credentials).expect("montar o cliente da speech-api"),
    );

    let sound_bank = Arc::new(SoundBank::new().expect("abrir a saída de áudio"));
    sound_bank.set_enabled(settings.sounds_enabled);
    sound_bank.set_volume(settings.sounds_volume);
    if let Some(directory) = &settings.external_sounds_directory {
        sound_bank.load_external_dictation_sounds(directory);
    }

    tracing::info!(%base_url, "vox iniciando");

    let state = AppState {
        dictation: Dictation::new(sound_bank.clone(), api.clone()),
        reader: Reader::new(api.clone(), sound_bank.clone()),
        settings: Mutex::new(settings),
        sounds: sound_bank,
        bridge: Arc::new(bridge::BridgeState::default()),
        narration: Arc::new(narration::Narration::new()),
        shortcut_report: Mutex::new(tray::ShortcutReport::default()),
        api,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            audio_levels,
            hud_timings,
            hud_presence,
            hud_ready,
            get_settings,
            save_settings,
            list_input_devices,
            api_health,
            test_server_connection,
            api_voices,
            cancel_dictation,
            show_reader,
            reading_cursor,
            reading_progress,
            reading_state,
            reading_finished,
            stop_reading,
            toggle_reading,
            read_text,
            preview_sound,
            set_reading_captions,
            open_settings,
            rewrite_preview,
            rewrite_presets,
            command_catalog,
            reapply_shortcuts,
            set_service_enabled,
            set_autostart,
            api_base_url,
            settings_path,
            reset_hud_position,
            show_floating_widget,
            restart_app,
            list_recordings,
            retry_recording,
            save_recording,
            delete_recording,
            open_recordings_folder,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // A janela do HUD nasce aqui, escondida, e nunca é destruída.
            //
            // Criar uma webview custa 100–200 ms; fazer isso no momento do atalho
            // apareceria como atraso justamente onde a resposta precisa ser
            // instantânea. Criada na partida, mostrar depois custa ~1 frame.
            // Ponte da extensão de navegador — e, pelo mesmo endereço de porta,
            // do mod do Claude Code. Falhar aqui não impede o app: atalho e
            // bandeja continuam funcionando, e a porta ocupada por outro
            // programa é o caso comum de erro.
            {
                let estado_do_app = app.state::<AppState>();
                let (ligada, porta, token, tailscale) = {
                    let settings = estado_do_app.settings.lock();
                    (
                        settings.bridge_enabled,
                        settings.bridge_port,
                        settings.bridge_token.clone(),
                        settings.bridge_tailscale,
                    )
                };
                if ligada {
                    let estado = estado_do_app.bridge.clone();
                    let acoes = std::sync::Arc::new(AcoesDaPonte { app: handle.clone() });
                    let acoes_narracao = std::sync::Arc::new(AcoesNarracaoDaPonte {
                        app: handle.clone(),
                    });
                    let token_loopback = token.clone();
                    tauri::async_runtime::spawn({
                        let acoes = acoes.clone();
                        let acoes_narracao = acoes_narracao.clone();
                        let estado_loopback = estado.clone();
                        async move {
                            let config = bridge::Config {
                                endereco: "127.0.0.1".into(),
                                porta,
                                token: token_loopback,
                            };
                            if let Err(err) =
                                bridge::servir(config, estado_loopback, acoes, acoes_narracao).await
                            {
                                tracing::error!(?err, porta, "a ponte da extensão não subiu");
                            }
                        }
                    });

                    // O segundo listener alcança os agentes que não estão
                    // nesta máquina — WSL em modo NAT e o homelab — pela
                    // tailnet, que já cifra o que passa por ela.
                    if tailscale {
                        if let Some(ip) = bridge::tailscale_ipv4() {
                            tracing::info!(%ip, "ponte da extensão também na tailnet");
                            tauri::async_runtime::spawn(async move {
                                let config = bridge::Config {
                                    endereco: ip.to_string(),
                                    porta,
                                    token,
                                };
                                if let Err(err) =
                                    bridge::servir(config, estado, acoes, acoes_narracao).await
                                {
                                    tracing::warn!(?err, porta, "a ponte da tailnet não subiu");
                                }
                            });
                        } else {
                            tracing::warn!(
                                "bridge_tailscale ligado e nenhum IP do Tailscale foi \
                                 encontrado; a ponte segue só no loopback"
                            );
                        }
                    }
                }
            }

            // O trabalhador da fila de narração: vive a vida inteira do app,
            // dormindo quando não há nada para narrar.
            tauri::async_runtime::spawn(rodar_narracao(handle.clone()));

            if let Some(hud) = app.get_webview_window("hud") {
                // Nasce escondido: o HUD so aparece quando ha o que mostrar.
                // Mesmo escondida a webview navega e carrega o front, entao o
                // primeiro ditado nao paga o custo de carregar a pagina.
                presence::conceal(&hud);
            }

            // Confere pro resto da vida do processo que o HUD, enquanto
            // visível, continua pintando — não só no instante em que aparece.
            // Ver o "O que essa confirmação ainda não cobria" em presence.rs.
            presence::watch_hud(handle.clone());

            // A preferência é a intenção; a chave do registro é o estado. Elas
            // divergem quando alguém limpa a inicialização com um utilitário por
            // fora, e a partida é o momento de reconciliar.
            {
                let estado = handle.state::<AppState>();
                let desejado = estado.settings.lock().start_with_windows;
                autostart::sync(desejado);
            }

            // O microfone preferido passou a ser guardado pelo ID estável do
            // WASAPI, não mais pelo nome cru. Quem tinha uma configuração de
            // antes dessa mudança continua funcionando (o casamento por nome
            // ainda existe como caminho de reserva), mas o painel mostraria
            // "não listado agora" pra um aparelho que está funcionando — só
            // porque o dropdown passou a comparar por ID. Migrar uma vez no
            // arranque evita esse falso alarme sem exigir que a pessoa
            // reabra as preferências e escolha o microfone de novo.
            {
                let estado = handle.state::<AppState>();
                let mut settings = estado.settings.lock();
                if let Some(atual) = settings.input_device.clone() {
                    if let Some(novo_id) = audio::migrar_id_se_precisar(&atual) {
                        tracing::info!(
                            antigo = %atual, novo = %novo_id,
                            "microfone preferido migrado para o ID estável do WASAPI"
                        );
                        settings.input_device = Some(novo_id);
                        if let Err(err) = settings.save() {
                            tracing::warn!(?err, "não deu para gravar a migração do microfone");
                        }
                    }
                }
            }

            // Primeiro uso: sem servidor configurado, ditado e leitura não têm
            // como funcionar, e uma bandeja muda não dá nenhuma pista disso.
            // O assistente só aparece até `onboarding_completed` virar `true`.
            let precisa_de_onboarding =
                !handle.state::<AppState>().settings.lock().onboarding_completed;
            if precisa_de_onboarding {
                if let Some(window) = handle.get_webview_window("onboarding") {
                    raise(&window);
                }
            }

            let ligado = handle.state::<AppState>().settings.lock().service_enabled;
            if !ligado {
                tracing::info!("vox sobe em pausa: sem atalhos até ser reativado na bandeja");
            }

            let report = if ligado {
                register_shortcuts(&handle)
            } else {
                tray::ShortcutReport::default()
            };
            for info in &report.commands {
                if let Some(motivo) = &info.failure {
                    tracing::warn!(
                        comando = info.id,
                        atalho = %info.binding,
                        motivo,
                        "atalho global não registrado; a ação segue pelo menu da bandeja"
                    );
                }
            }

            // Guardado para o painel mostrar a falha ao lado do campo que a
            // causou — sem isso, a única pista seria o menu da bandeja.
            *handle.state::<AppState>().shortcut_report.lock() = report.clone();

            // A bandeja é o único ponto de contato visível do app: sem ela não
            // haveria como sair nem como saber que ele está rodando.
            if let Err(err) = tray::build(&handle, &report) {
                tracing::error!(?err, "falha ao criar o ícone de bandeja");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "hud" {
                if let tauri::WindowEvent::Moved(position) = event {
                    // Movimento que o próprio app causou não é preferência de
                    // ninguém. Gravar isso era o que fazia o ditado nascer onde a
                    // leitura tinha acabado de ser posta.
                    if dictation::movement_is_ours() {
                        return;
                    }
                    // Janela escondida não é arrastada por ninguém.
                    if !window.is_visible().unwrap_or(false) {
                        return;
                    }
                    let Some(papel) = dictation::current_role() else { return };

                    let scale = window.scale_factor().unwrap_or(1.0);
                    let position = position.to_logical::<f64>(scale);
                    let largura = window
                        .outer_size()
                        .map(|tamanho| tamanho.to_logical::<f64>(scale).width)
                        .unwrap_or(0.0);

                    let guardar = match papel {
                        dictation::HudRole::Dictation => {
                            config::WindowPosition { x: position.x, y: position.y }
                        }
                        // A leitura é lembrada pela borda direita, que é o que
                        // não muda entre a pílula estreita e a aberta.
                        dictation::HudRole::Reading => {
                            config::WindowPosition { x: position.x + largura, y: position.y }
                        }
                    };

                    let app = window.app_handle();
                    let app_state = app.state::<AppState>();
                    let mut settings = app_state.settings.lock();
                    let ancorado = match papel {
                        dictation::HudRole::Dictation => {
                            settings.hud_placement_dictation != config::HudPlacement::Livre
                        }
                        dictation::HudRole::Reading => {
                            settings.hud_placement_reading != config::HudPlacement::Livre
                        }
                    };
                    let campo = match papel {
                        dictation::HudRole::Dictation => &mut settings.hud_position_dictation,
                        dictation::HudRole::Reading => &mut settings.hud_position_reading,
                    };
                    // Ancorado não guarda arraste: o ponto fixo manda, e a
                    // janela nem sai do lugar para o `Moved` chegar aqui. A
                    // guarda existe para o movimento que vier de fora do app —
                    // gravar um lugar que ninguém escolheu seria uma lembrança
                    // fantasma esperando a âncora voltar a ser "livre".
                    if ancorado {
                        return;
                    }
                    if *campo != Some(guardar) {
                        *campo = Some(guardar);
                        if let Err(error) = settings.save() {
                            tracing::warn!(?error, "não deu para guardar a posição do widget");
                        }
                    }
                }
                return;
            }
            // Fechar as preferências é só sair delas: destruir a webview
            // custaria recarregar a página inteira na próxima abertura, e o
            // painel é justamente o que se abre para mexer em duas coisas e
            // fechar de novo.
            if window.label() == "settings" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                        crate::presence::conceal(&webview);
                    }
                }
                return;
            }
            if window.label() != "reader" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Esconde em vez de destruir: a webview destruida nunca avisa
                // que a reprodução terminou, e o estado da leitura ficava preso
                // em `Playing`. Com a janela viva, o front sempre reporta.
                api.prevent_close();
                if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                    crate::presence::conceal(&webview);
                }

                // Fechar o leitor é dizer "terminei" — parar a fala junto é o
                // que a pessoa espera, e deixar a voz seguindo sem janela
                // nenhuma seria pior.
                let app = window.app_handle();
                let estado = app.state::<AppState>();
                estado.reader.stop(app);
                estado.bridge.clear();
            }
        })
        .build(tauri::generate_context!())
        .expect("erro ao montar o vox")
        .run(|_app, event| {
            // Fechar a janela de leitura não pode derrubar o app: ele vive na
            // bandeja, e o Tauri encerra por padrão quando a última janela
            // some. Sem isto, fechar o leitor mataria o ditado junto — e o
            // usuário não teria como saber por quê.
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
