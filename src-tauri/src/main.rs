// Sem console no Windows em release: o app vive na bandeja.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod audio;
mod autostart;
mod bridge;
mod commands;
mod config;
mod dictation;
mod paste;
mod reading;
mod sounds;
mod stt_stream;
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
    {
        let vigente = state.settings.lock();
        settings.hud_position_dictation = vigente.hud_position_dictation;
        settings.hud_position_reading = vigente.hud_position_reading;
    }

    settings.save().map_err(|err| err.to_string())?;

    // Aplica no ato o que muda comportamento agora, sem esperar um restart.
    state.sounds.set_enabled(settings.sounds_enabled);
    state.sounds.set_volume(settings.sounds_volume);
    if let Some(directory) = &settings.external_sounds_directory {
        state.sounds.load_external_dictation_sounds(directory);
    }

    // As outras janelas precisam saber: a cor de destaque vale para o realce da
    // palavra na pílula, e quem acabou de escolhê-la está olhando para ela.
    let _ = app.emit("vox://settings-changed", serde_json::json!({
        "theme_accent": settings.theme_accent,
    }));

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
fn api_base_url() -> String {
    config::base_url()
}

/// Caminho do arquivo de preferências.
#[tauri::command]
fn settings_path() -> String {
    config::settings_path().to_string_lossy().to_string()
}

/// Esquece a posição guardada do widget e o devolve ao lugar de origem.
#[tauri::command]
fn reset_hud_position(app: AppHandle, state: State<'_, AppState>) {
    {
        let mut settings = state.settings.lock();
        settings.hud_position_dictation = None;
        settings.hud_position_reading = None;
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

/// Mostra o widget em repouso, sem iniciar ditado ou leitura.
#[tauri::command]
fn show_floating_widget(app: AppHandle) {
    dictation::show_idle_hud(&app);
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

#[tauri::command]
async fn api_health(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.api.health().await.map_err(|err| err.to_string())
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
    let _ = window.show();
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
        Command::ShowWidget => dictation::show_idle_hud(&app),
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
        // Deixa a mensagem na tela pelo tempo que o Raycast usa, depois some.
        tokio::time::sleep(std::time::Duration::from_millis(dictation::SUCCESS_HOLD_MS)).await;
        dictation::hide_hud(&app);
        dictation::hide_live_window(&app);
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

            let (voice, speed, prebuffer, normalizar, narrar_tabelas) = {
                let state = app.state::<AppState>();
                let settings = state.settings.lock();
                (
                    settings.voice.clone(),
                    settings.speed,
                    settings.prebuffer_ratio,
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
                state.reader.prepare(&texto, normalizar, narrar_tabelas).await
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
    let base_url = config::base_url();
    let credentials = config::credentials();

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
        shortcut_report: Mutex::new(tray::ShortcutReport::default()),
        api,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            audio_levels,
            hud_timings,
            get_settings,
            save_settings,
            list_input_devices,
            api_health,
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
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // A janela do HUD nasce aqui, escondida, e nunca é destruída.
            //
            // Criar uma webview custa 100–200 ms; fazer isso no momento do atalho
            // apareceria como atraso justamente onde a resposta precisa ser
            // instantânea. Criada na partida, mostrar depois custa ~1 frame.
            // Ponte da extensão de navegador. Falhar aqui não impede o app:
            // atalho e bandeja continuam funcionando, e a porta ocupada por
            // outro programa é o caso comum de erro.
            {
                let estado_do_app = app.state::<AppState>();
                let (ligada, porta, token) = {
                    let settings = estado_do_app.settings.lock();
                    (settings.bridge_enabled, settings.bridge_port, settings.bridge_token.clone())
                };
                if ligada {
                    let estado = estado_do_app.bridge.clone();
                    let acoes = std::sync::Arc::new(AcoesDaPonte { app: handle.clone() });
                    tauri::async_runtime::spawn(async move {
                        let config = bridge::Config { porta, token };
                        if let Err(err) = bridge::servir(config, estado, acoes).await {
                            tracing::error!(?err, porta, "a ponte da extensão não subiu");
                        }
                    });
                }
            }

            if let Some(hud) = app.get_webview_window("hud") {
                // Nasce escondido: o HUD so aparece quando ha o que mostrar.
                // Mesmo escondida a webview navega e carrega o front, entao o
                // primeiro ditado nao paga o custo de carregar a pagina.
                let _ = hud.hide();
            }

            // A preferência é a intenção; a chave do registro é o estado. Elas
            // divergem quando alguém limpa a inicialização com um utilitário por
            // fora, e a partida é o momento de reconciliar.
            {
                let estado = handle.state::<AppState>();
                let desejado = estado.settings.lock().start_with_windows;
                autostart::sync(desejado);
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
                    let campo = match papel {
                        dictation::HudRole::Dictation => &mut settings.hud_position_dictation,
                        dictation::HudRole::Reading => &mut settings.hud_position_reading,
                    };
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
                    let _ = window.hide();
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
                let _ = window.hide();

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
