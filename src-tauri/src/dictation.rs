//! O ciclo do ditado.
//!
//! A ordem das operações aqui foi escolhida por latência percebida, e não é
//! arbitrária. Ao apertar o atalho, nesta ordem:
//!
//! 1. **o som toca** — retorno em poucos milissegundos, antes de qualquer coisa
//!    que possa falhar ou demorar;
//! 2. **o microfone abre** — o primeiro fonema não pode se perder;
//! 3. **o HUD aparece** — a janela já existe escondida desde a partida, então
//!    mostrar custa quase nada;
//! 4. **a conexão é pré-aquecida** — em segundo plano, enquanto o usuário fala.
//!
//! O passo 4 é o que tira 100–300 ms do fim: quando o usuário solta o atalho, o
//! handshake TLS já aconteceu e o upload começa direto.

use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::api::SpeechApi;
use crate::audio::Capture;
use crate::config::{OutputAction, Settings};
use crate::paste;
use crate::sounds::{Cue, SoundBank};

/// Intervalo de leitura dos níveis. Casado com a transição de 20 ms das barras:
/// cada valor assenta antes do próximo chegar, e a onda não pisca.
pub const LEVEL_POLL_MS: u64 = 50;

/// Quantas barras o HUD desenha em cada modo.
/// Quantas barras a onda desenha.
///
/// A referência usava 12 (e 22 em push-to-talk) numa faixa de 86px, e as barras
/// ocupavam só 46px dela — o resto era ar. Copiado para o nosso HUD, de 168px,
/// a onda virava um tufo no terço do meio.
///
/// O número aqui é derivado da geometria, não escolhido: 37 barras de 2px com
/// 2px de intervalo dão 146px, que é a largura útil do HUD. Mudou a largura,
/// muda este número — os dois andam juntos e estão anotados um no outro.
pub const BARS_NORMAL: usize = 37;

/// Igual ao normal. A referência dobrava a contagem em push-to-talk porque lá o
/// HUD escondia os botões e sobrava espaço; aqui os botões são absolutos e não
/// disputam espaço com a onda em modo nenhum, então a largura — e portanto a
/// contagem — é a mesma nos dois.
pub const BARS_PUSH_TO_TALK: usize = BARS_NORMAL;

/// Quanto tempo o HUD fica visível depois de uma mensagem de sucesso.
pub const SUCCESS_HOLD_MS: u64 = 2000;

/// De quanto em quanto tempo o áudio novo é empurrado para o streaming.
///
/// Casado com o bloco de 250 ms que o servidor reagrupa internamente: mandar
/// mais rápido só multiplica quadros sem antecipar nenhum resultado.
const LIVE_PUSH_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Recording,
    Transcribing,
}

pub struct DictationSession {
    capture: Option<Capture>,
    /// Sessão de transcrição incremental, quando ligada.
    live_stream: Option<Arc<crate::stt_stream::SttStream>>,
    phase: Phase,
    push_to_talk: bool,
    /// Texto capturado antes do cursor, lido no início e usado na colagem.
    text_before_caret: Option<String>,
}

impl Default for DictationSession {
    fn default() -> Self {
        Self {
            capture: None,
            live_stream: None,
            phase: Phase::Idle,
            push_to_talk: false,
            text_before_caret: None,
        }
    }
}

impl DictationSession {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn bar_count(&self) -> usize {
        if self.push_to_talk {
            BARS_PUSH_TO_TALK
        } else {
            BARS_NORMAL
        }
    }

    /// Texto que o streaming acumulou até agora, para o HUD mostrar.
    ///
    /// Vazio quando o streaming está desligado ou ainda não produziu nada — o
    /// HUD trata isso como "sem texto" e mantém só a onda.
    pub fn live_text(&self) -> String {
        self.live_stream
            .as_ref()
            .map(|stream| stream.display_text())
            .unwrap_or_default()
    }

    pub fn levels(&self) -> Vec<f32> {
        match &self.capture {
            Some(capture) => capture.levels(self.bar_count()),
            None => vec![0.0; self.bar_count()],
        }
    }
}

pub struct Dictation {
    session: Arc<Mutex<DictationSession>>,
    sounds: Arc<SoundBank>,
    api: Arc<SpeechApi>,
}

impl Dictation {
    pub fn new(sounds: Arc<SoundBank>, api: Arc<SpeechApi>) -> Self {
        Self {
            session: Arc::new(Mutex::new(DictationSession::default())),
            sounds,
            api,
        }
    }

    pub fn session(&self) -> Arc<Mutex<DictationSession>> {
        self.session.clone()
    }

    /// Começa a gravar. Ver a ordem comentada no topo do módulo.
    pub fn start(&self, app: &AppHandle, settings: &Settings, push_to_talk: bool) -> Result<()> {
        {
            let session = self.session.lock();
            if session.phase != Phase::Idle {
                tracing::debug!("ditado já em curso; ignorando");
                return Ok(());
            }
        }

        // 1. retorno imediato
        self.sounds.play(Cue::DictationStart);

        // 2. microfone
        let capture = Capture::start(settings.input_device.as_deref())?;
        tracing::info!(fonte = %capture.source_description(), "ditado iniciado");

        {
            let mut session = self.session.lock();
            session.capture = Some(capture);
            session.phase = Phase::Recording;
            session.push_to_talk = push_to_talk;
            session.text_before_caret = None;
        }

        // 3. HUD (a janela já existe; isto só a torna visível)
        show_hud(app, push_to_talk);
        show_live_window(app, settings.live_transcription && settings.show_live_transcription);

        // 4. aquece a conexão enquanto o usuário fala
        let api = self.api.clone();
        tauri::async_runtime::spawn(async move { api.prewarm().await });

        // 5. transcrição incremental, se ligada
        //
        // Vem depois de tudo de propósito: se o WebSocket falhar, o ditado
        // continua funcionando pelo caminho em lote. Retorno visual é um
        // acréscimo, não um pré-requisito — perder a transcrição inteira porque
        // um enfeite não conectou seria péssimo.
        if settings.live_transcription {
            self.spawn_live_transcription(app.clone());
        }

        Ok(())
    }

    /// Abre a sessão de streaming e bombeia áudio enquanto a gravação dura.
    ///
    /// Roda numa tarefa própria e nunca propaga erro para o ditado: qualquer
    /// falha aqui apenas desliga o texto ao vivo e deixa o HUD com a onda, que
    /// é exatamente o comportamento de antes desta funcionalidade existir.
    fn spawn_live_transcription(&self, app: AppHandle) {
        let session = self.session.clone();
        let base_url = crate::config::base_url();
        let credentials = crate::config::credentials()
            .map(|creds| (creds.username, creds.password));

        tauri::async_runtime::spawn(async move {
            let stream = match crate::stt_stream::SttStream::connect(&base_url, credentials).await {
                Ok(stream) => Arc::new(stream),
                Err(err) => {
                    tracing::warn!(?err, "sem transcrição ao vivo; o ditado segue normal");
                    return;
                }
            };

            {
                let mut guarda = session.lock();
                if guarda.phase != Phase::Recording {
                    return;  // o usuário já soltou o atalho antes de conectarmos
                }
                guarda.live_stream = Some(stream.clone());
            }

            let mut ticker =
                tokio::time::interval(std::time::Duration::from_millis(LIVE_PUSH_MS));

            loop {
                ticker.tick().await;

                let (gravando, novas) = {
                    let guarda = session.lock();
                    let novas = guarda
                        .capture
                        .as_ref()
                        .map(|capture| capture.drain_new_samples())
                        .unwrap_or_default();
                    (guarda.phase == Phase::Recording, novas)
                };

                if !novas.is_empty() {
                    stream.push(&novas);
                }
                if !gravando {
                    break;
                }

                // Enquanto grava, publica o texto para o HUD desenhar.
                let texto = stream.display_text();
                if !texto.is_empty() {
                    let _ = app.emit("vox://live-text", serde_json::json!({ "text": texto }));
                }
            }
        });
    }

    /// Encerra a gravação e devolve o áudio pronto para envio.
    ///
    /// Devolve `None` quando não havia sessão ativa, para o chamador poder
    /// ignorar um atalho repetido sem tratar erro.
    pub fn stop_recording(&self) -> Option<crate::audio::Recording> {
        let mut session = self.session.lock();
        if session.phase != Phase::Recording {
            return None;
        }
        session.phase = Phase::Transcribing;

        // Entrega ao streaming o que ficou desde o último bombeio, para o texto
        // ao vivo não parar no meio da última frase.
        if let (Some(capture), Some(stream)) = (&session.capture, &session.live_stream) {
            let resto = capture.drain_new_samples();
            if !resto.is_empty() {
                stream.push(&resto);
            }
        }

        session.capture.take().map(|capture| capture.finish())
    }

    pub fn reset(&self) {
        let mut session = self.session.lock();
        session.capture = None;
        session.live_stream = None;
        session.phase = Phase::Idle;
        session.push_to_talk = false;
        session.text_before_caret = None;
    }

    /// Descarta a sessão sem transcrever.
    ///
    /// O `reset` solta o `Arc` do streaming; a tarefa de rede percebe que a
    /// fase saiu de `Recording` no próximo tique e encerra sozinha. Não há o que
    /// abortar à força.
    pub fn cancel(&self, app: &AppHandle) {
        // Aborta o streaming: o usuário desistiu, e terminar de transcrever um
        // áudio que ninguém vai usar só ocuparia o servidor.
        if let Some(stream) = self.session.lock().live_stream.take() {
            stream.abort();
        }
        self.reset();
        hide_hud(app);
        hide_live_window(app);
    }

    /// Transcreve e entrega o texto conforme as preferências.
    pub async fn deliver(
        &self,
        app: AppHandle,
        settings: Settings,
        recording: crate::audio::Recording,
    ) -> Result<String> {
        tracing::info!(
            dispositivo = %recording.device_name,
            segundos = recording.duration_seconds(),
            pico = recording.peak,
            fala_detectada = recording.speech_detected,
            "gravação encerrada"
        );

        if !recording.speech_detected || recording.duration_seconds() < 0.25 {
            self.reset();
            emit_state(&app, "warning", Some("Nenhuma fala detectada"), None);
            self.sounds.play(Cue::DictationFailure);
            return Ok(String::new());
        }

        // Fecha o streaming e guarda o que ele conseguiu. Serve de **reserva do
        // streaming**: se o caminho em lote falhar (rede caiu, modelo remoto
        // fora do ar), é melhor entregar um texto imperfeito do Vosk do que
        // perder o ditado inteiro. Quem falou não tem como repetir do mesmo
        // jeito, e mandar "tente de novo" quando existe texto na mão é ruim.
        // O `take` sai num escopo próprio: segurar o guard do mutex durante o
        // `.await` abaixo tornaria este future não-`Send`, e o Tauri exige que
        // seja. Um lock de `parking_lot` atravessando ponto de espera é sempre
        // erro — aqui o compilador pegou, mas o motivo vale lembrar.
        let live_stream = self.session.lock().live_stream.take();

        let reserva = match live_stream {
            Some(stream) => match stream.finish().await {
                Ok(texto) => texto,
                Err(err) => {
                    // Fechar mal não é motivo para perder o que já foi
                    // reconhecido: o acumulado em memória continua válido.
                    tracing::debug!(?err, "streaming fechou com erro; usando o acumulado");
                    stream.display_text()
                }
            },
            None => String::new(),
        };

        let wav = recording.to_wav();
        tracing::debug!(bytes = wav.len(), "enviando para transcrição");

        let transcription = match self.api.transcribe(wav, &settings.transcription_model).await {
            Ok(response) => response.text,
            Err(err) if !reserva.trim().is_empty() => {
                tracing::warn!(
                    ?err,
                    chars = reserva.chars().count(),
                    "lote falhou; usando o texto do streaming"
                );
                emit_state(
                    &app,
                    "warning",
                    Some("Texto parcial"),
                    Some("A transcrição final falhou; este veio do reconhecimento ao vivo."),
                );
                reserva.clone()
            }
            Err(err) => {
                self.reset();
                tracing::error!(?err, "transcrição falhou");
                emit_state(&app, "error", Some("Transcrição falhou"), Some("Tente de novo"));
                self.sounds.play(Cue::DictationFailure);
                return Err(err);
            }
        };

        let text = transcription.trim().to_string();
        if text.is_empty() {
            self.reset();
            emit_state(&app, "warning", Some("Nenhuma fala detectada"), None);
            self.sounds.play(Cue::DictationFailure);
            return Ok(String::new());
        }

        let (text, should_submit) =
            paste::resolve_submit(&text, settings.submit_mode, &settings.submit_keyword);

        let before_caret = self.session.lock().text_before_caret.clone();
        let final_text = if settings.context_aware_paste {
            paste::fit_to_context(&text, before_caret.as_deref())
        } else {
            text.clone()
        };

        let outcome = match settings.output_action {
            OutputAction::Clipboard => paste::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(&final_text))
                .map(|_| "Copiado"),
            OutputAction::Paste => {
                let submit_key = should_submit.then_some(settings.submit_key);
                paste::paste(&final_text, submit_key).map(|_| "Colado")
            }
        };

        self.reset();

        match outcome {
            Ok(label) => {
                emit_state(&app, "success", Some(label), None);
                self.sounds.play(Cue::DictationSuccess);
            }
            Err(err) => {
                // A colagem falhou, mas o texto não pode se perder: vai para a
                // área de transferência e o usuário é avisado do que aconteceu.
                tracing::error!(?err, "entrega falhou; caindo para a área de transferência");
                let saved = paste::Clipboard::new()
                    .and_then(|mut clipboard| clipboard.set_text(&final_text))
                    .is_ok();
                if saved {
                    emit_state(
                        &app,
                        "warning",
                        Some("Transcrição copiada"),
                        Some("Não deu para colar no app ativo."),
                    );
                } else {
                    emit_state(&app, "error", Some("Falha ao entregar o texto"), None);
                }
                self.sounds.play(Cue::DictationFailure);
            }
        }

        // O envio automático só existe no caminho de colagem; registrar quando
        // ele foi ignorado ajuda a entender relatos de "não enviou".
        if should_submit && settings.output_action == OutputAction::Clipboard {
            tracing::debug!("envio automático ignorado: a saída está em área de transferência");
        }

        Ok(final_text)
    }
}

/// Formatos que a janela flutuante assume.
///
/// São dois usos bem diferentes: o ditado quer uma barra curta perto de onde o
/// olho já está, e a leitura quer uma coluna estreita encostada na borda, que
/// não corta a linha do texto sendo lido.
#[derive(Debug, Clone, Copy)]
pub enum HudShape {
    /// Barra horizontal do ditado.
    Bar,
    /// Pílula vertical do player de leitura.
    Column,
}

impl HudShape {
    fn size(self) -> (f64, f64) {
        match self {
            // Folga em volta do desenho: a janela é transparente e a sombra
            // precisa de espaço, senão sai cortada na borda.
            HudShape::Bar => (360.0, 84.0),
            HudShape::Column => (84.0, 240.0),
        }
    }
}

/// Redimensiona e reposiciona a janela flutuante.
///
/// A barra do ditado fica no centro inferior, com folga para não encostar na
/// barra de tarefas: durante o ditado o olho não está no texto, e o centro é
/// onde ele acha o indicador sem procurar. A coluna da leitura encosta na borda
/// direita, centralizada na vertical — é onde ela cobre menos texto.
pub fn shape_hud(app: &AppHandle, shape: HudShape) {
    let Some(window) = app.get_webview_window("hud") else { return };

    let (width, height) = shape.size();
    let _ = window.set_size(tauri::LogicalSize::new(width, height));

    // Janela escondida nem sempre tem monitor associado no Windows. Quando isso
    // acontece o HUD fica onde estava — o que já pareceu "o HUD não abriu",
    // sendo que ele abriu fora da vista.
    let monitor = match window.current_monitor() {
        Ok(Some(monitor)) => monitor,
        outro => {
            tracing::warn!(
                ?outro,
                "sem monitor para posicionar o HUD; ele fica na posição anterior"
            );
            return;
        }
    };
    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);

    let (x, y) = match shape {
        HudShape::Bar => ((screen.width - width) / 2.0, screen.height - height - 96.0),
        HudShape::Column => (screen.width - width - 12.0, (screen.height - height) / 2.0),
    };
    tracing::debug!(
        ?shape, largura = width, altura = height,
        tela_l = screen.width, tela_a = screen.height, escala = scale,
        x, y, "posicionando o HUD"
    );
    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
}

pub fn show_hud(app: &AppHandle, push_to_talk: bool) {
    let Some(window) = app.get_webview_window("hud") else {
        tracing::error!("janela do HUD não encontrada");
        return;
    };
    shape_hud(app, HudShape::Bar);
    let _ = app.emit("vox://hud", serde_json::json!({
        "state": "recording",
        "pushToTalk": push_to_talk,
    }));
    if let Err(err) = window.show() {
        tracing::error!(?err, "não deu para mostrar o HUD");
        return;
    }
    tracing::debug!(
        visivel = ?window.is_visible(),
        posicao = ?window.outer_position(),
        tamanho = ?window.outer_size(),
        "HUD mostrado"
    );
}

pub fn hide_hud(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("hud") {
        let _ = window.hide();
    }
}

/// Mostra a janelinha do texto ao vivo, se a preferência estiver ligada.
///
/// Ela é uma janela separada, e não uma faixa dentro do HUD, porque as duas
/// coisas têm ritmos diferentes: a onda responde à voz em tempo real e precisa
/// ficar quieta no lugar; o texto cresce e reflui. Juntos, o HUD pulava de
/// tamanho a cada palavra reconhecida.
pub fn show_live_window(app: &AppHandle, ligado: bool) {
    let Some(window) = app.get_webview_window("live") else { return };
    if !ligado {
        let _ = window.hide();
        return;
    }

    let _ = app.emit("vox://live-text", serde_json::json!({ "text": "" }));

    if let Ok(Some(monitor)) = window.current_monitor() {
        let scale = monitor.scale_factor();
        let screen = monitor.size().to_logical::<f64>(scale);
        let (width, _) = LIVE_SIZE;
        // Canto superior direito, com a mesma folga dos dois lados.
        let _ = window.set_position(tauri::LogicalPosition::new(
            screen.width - width - 24.0,
            24.0,
        ));
    }
    let _ = window.show();
}

pub fn hide_live_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("live") {
        let _ = window.hide();
    }
}

/// Precisa bater com o que está em `tauri.conf.json`: o posicionamento é feito
/// aqui e depende da largura real da janela.
const LIVE_SIZE: (f64, f64) = (360.0, 132.0);

fn emit_state(app: &AppHandle, state: &str, title: Option<&str>, message: Option<&str>) {
    let _ = app.emit("vox://hud", serde_json::json!({
        "state": state,
        "title": title,
        "message": message,
    }));
}
