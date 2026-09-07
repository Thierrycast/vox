// Sem console no Windows em release: o app vive na bandeja.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod audio;
mod bridge;
mod config;
mod dictation;
mod paste;
mod reading;
mod sounds;
mod stt_stream;
mod tray;

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager, State};
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
fn save_settings(state: State<'_, AppState>, mut settings: Settings) -> Result<(), String> {
    settings.sanitize();
    settings.save().map_err(|err| err.to_string())?;

    // Aplica no ato o que muda comportamento agora, sem esperar um restart.
    state.sounds.set_enabled(settings.sounds_enabled);
    state.sounds.set_volume(settings.sounds_volume);
    if let Some(directory) = &settings.external_sounds_directory {
        state.sounds.load_external_dictation_sounds(directory);
    }

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

/// Abre o painel de preferências.
#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings(&app);
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
fn register_shortcuts(app: &AppHandle) -> tray::ShortcutReport {
    let mut report = tray::ShortcutReport::default();

    let (texto_ditado, texto_leitura) = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock();
        (settings.shortcut_dictate.clone(), settings.shortcut_read.clone())
    };

    let dictate = match parse_shortcut(&texto_ditado) {
        Some(atalho) => atalho,
        None => {
            report.dictate = Some(format!("combinação inválida: {texto_ditado}"));
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyD)
        }
    };
    let read = match parse_shortcut(&texto_leitura) {
        Some(atalho) => atalho,
        None => {
            report.read = Some(format!("combinação inválida: {texto_leitura}"));
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyL)
        }
    };

    report.dictate_label = texto_ditado;
    report.read_label = texto_leitura;

    let handle = app.clone();
    if let Err(err) = app.global_shortcut().on_shortcut(dictate, move |_app, _sc, event| {
        if event.state() != ShortcutState::Pressed {
            return;
        }
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move { toggle_dictation(handle).await });
    }) {
        tracing::error!(?err, "Ctrl+Shift+D indisponível");
        report.dictate = Some(motivo_curto(&err));
    }

    let handle = app.clone();
    if let Err(err) = app.global_shortcut().on_shortcut(read, move |_app, _sc, event| {
        if event.state() != ShortcutState::Pressed {
            return;
        }
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move { toggle_reading_shortcut(handle).await });
    }) {
        tracing::error!(?err, "Ctrl+Shift+S indisponível");
        report.read = Some(motivo_curto(&err));
    }

    report
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
    fn ler(&self, texto: String) -> Vec<String> {
        let state = self.app.state::<AppState>();
        let (voice, speed, prebuffer) = {
            let settings = state.settings.lock();
            (settings.voice.clone(), settings.speed, settings.prebuffer_ratio)
        };

        // Divide aqui e devolve a mesma lista que vai ser falada. A extensão
        // precisa da divisão idêntica para casar trecho com pedaço do DOM;
        // dividir dos dois lados daria listas diferentes na primeira
        // abreviação ou reticência.
        let segments = api::split_text(&texto);
        state.bridge.set_segments(segments.clone());

        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            let handle = app.clone();
            if let Err(err) = state
                .reader
                .speak(handle, texto, voice, speed, prebuffer)
                .await
            {
                tracing::error!(?err, "leitura pedida pela extensão falhou");
            }
        });

        segments
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

            let report = register_shortcuts(&handle);
            if report.has_failure() {
                tracing::warn!(
                    ditado = ?report.dictate,
                    leitura = ?report.read,
                    "algum atalho global não pôde ser registrado; \
                     as ações seguem disponíveis pelo menu da bandeja"
                );
            }

            // A bandeja é o único ponto de contato visível do app: sem ela não
            // haveria como sair nem como saber que ele está rodando.
            if let Err(err) = tray::build(&handle, &report) {
                tracing::error!(?err, "falha ao criar o ícone de bandeja");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
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
