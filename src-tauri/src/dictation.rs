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

use std::sync::atomic::{AtomicU8, Ordering};
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

    /// Passa o texto ditado por um modelo, se a preferência estiver ligada.
    ///
    /// Devolve sempre um texto utilizável. Falha de rede, modelo fora do ar,
    /// resposta com tamanho suspeito — em todos os casos o que volta é o que foi
    /// ditado. A pessoa acabou de falar e está esperando para colar; entregar
    /// nada seria muito pior do que entregar sem o ajuste.
    async fn rewrite(&self, text: &str, settings: &Settings) -> String {
        let comecou = std::time::Instant::now();

        let resultado = self
            .api
            .rewrite_text(
                text,
                &settings.stt_rewrite_preset,
                settings.stt_rewrite_intensity,
                settings.stt_rewrite_model.as_deref(),
            )
            .await;

        match resultado {
            Ok(saida) => {
                if let Some(motivo) = saida.rewritten.error.or(saida.rewritten.skipped) {
                    tracing::warn!(motivo, "reescrita não foi aplicada; texto original mantido");
                } else {
                    tracing::info!(
                        preset = %settings.stt_rewrite_preset,
                        intensidade = settings.stt_rewrite_intensity,
                        modelo_ms = saida.rewritten.elapsed_ms,
                        total_ms = comecou.elapsed().as_millis(),
                        "texto reescrito"
                    );
                }
                saida.text
            }
            Err(err) => {
                tracing::warn!(?err, "reescrita falhou; entregando o texto ditado");
                text.to_string()
            }
        }
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

        // Vocabulário e instruções viram o `prompt` da transcrição — é assim que
        // se ensina um nome próprio ao modelo. Antes eles eram guardados e nunca
        // enviados, porque a API não tinha onde recebê-los; desde a 2.6.0 tem.
        let prompt = crate::config::transcription_prompt(&settings);

        let transcription = match self
            .api
            .transcribe(wav, &settings.transcription_model, &prompt)
            .await
        {
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

        // A reescrita vem antes da palavra-chave de envio, e não depois: se ela
        // rodasse por último, o modelo receberia "manda ver" no fim do texto e
        // trataria como conteúdo — ou, pior, o reescreveria e a palavra deixaria
        // de ser reconhecida.
        let (text, should_submit) =
            paste::resolve_submit(&text, settings.submit_mode, &settings.submit_keyword);

        let text = if settings.stt_rewrite_enabled {
            emit_state(&app, "rewriting", Some("Ajustando"), None);
            self.rewrite(&text, &settings).await
        } else {
            text
        };

        // Sem chave para ligar e desligar: `fit_to_context` só faz alguma coisa
        // quando sabe o que está antes do cursor, e hoje ninguém sabe — ler isso
        // exige UI Automation, que ainda não existe aqui. A preferência que
        // existia oferecia escolha entre duas coisas idênticas; quando a leitura
        // do cursor chegar, o ajuste passa a valer sozinho.
        let before_caret = self.session.lock().text_before_caret.clone();
        let final_text = paste::fit_to_context(&text, before_caret.as_deref());

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
    /// A mesma pílula com a legenda guiada aberta ao lado.
    ColumnCaptions,
}

/// Para que o widget está sendo usado agora.
///
/// A janela é uma só, mas os papéis não se confundem: cada um lembra a própria
/// posição. Sem essa separação, a barra do ditado nascia onde a coluna da leitura
/// tinha ficado, e vice-versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudRole {
    Dictation,
    Reading,
}

impl HudShape {
    pub fn role(self) -> HudRole {
        match self {
            HudShape::Bar => HudRole::Dictation,
            HudShape::Column | HudShape::ColumnCaptions => HudRole::Reading,
        }
    }

    fn size(self) -> (f64, f64) {
        match self {
            // Folga em volta do desenho: a janela é transparente e a sombra
            // precisa de espaço, senão sai cortada na borda.
            HudShape::Bar => (360.0, 136.0),
            // A altura é a mesma das duas formas da leitura de propósito: abrir
            // a legenda muda só a largura, e a pílula não pula na vertical
            // debaixo do cursor que acabou de clicar.
            HudShape::Column => (136.0, 320.0),
            // A janela cresce junto com o card, e não antes dele: uma janela
            // transparente maior que o desenho continua capturando o clique na
            // área vazia, e o usuário fica sem entender por que a página atrás
            // parou de responder num retângulo invisível.
            HudShape::ColumnCaptions => (504.0, 320.0),
        }
    }
}

/// O papel que a janela está exercendo agora. Zero é nenhum ainda.
static PAPEL_ATUAL: AtomicU8 = AtomicU8::new(0);

/// Até quando um `Moved` da janela é consequência de um movimento **nosso**.
///
/// O Windows avisa a mudança de posição do mesmo jeito quando a pessoa arrasta e
/// quando o próprio app chama `set_position`. Sem distinguir, cada troca de forma
/// era gravada como se tivesse sido um arraste — e o lugar onde a leitura tinha
/// sido posta virava a preferência do ditado.
static MOVIMENTO_NOSSO_ATE: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// Quanto dura o silêncio depois de um movimento programado. Os eventos chegam
/// assíncronos, e numa máquina ocupada chegam atrasados; 450 ms cobre os dois sem
/// engolir um arraste que a pessoa comece logo em seguida.
const SILENCIO_APOS_MOVER: std::time::Duration = std::time::Duration::from_millis(450);

pub fn current_role() -> Option<HudRole> {
    match PAPEL_ATUAL.load(Ordering::Relaxed) {
        1 => Some(HudRole::Dictation),
        2 => Some(HudRole::Reading),
        _ => None,
    }
}

/// Esquece o papel atual, para a próxima forma não herdar a posição da janela.
///
/// Usado ao voltar ao padrão: sem isto, `shape_hud` veria a janela visível no
/// mesmo papel e manteria exatamente o lugar de onde a pessoa pediu para sair.
pub fn forget_role() {
    PAPEL_ATUAL.store(0, Ordering::Relaxed);
}

fn set_current_role(papel: HudRole) {
    let valor = match papel {
        HudRole::Dictation => 1,
        HudRole::Reading => 2,
    };
    PAPEL_ATUAL.store(valor, Ordering::Relaxed);
}

fn marcar_movimento_programado() {
    if let Ok(mut ate) = MOVIMENTO_NOSSO_ATE.lock() {
        *ate = Some(std::time::Instant::now() + SILENCIO_APOS_MOVER);
    }
}

/// O `Moved` que acabou de chegar foi o app que causou?
pub fn movement_is_ours() -> bool {
    MOVIMENTO_NOSSO_ATE
        .lock()
        .ok()
        .and_then(|ate| *ate)
        .is_some_and(|limite| std::time::Instant::now() < limite)
}

/// O ponto está dentro de algum monitor ligado agora?
///
/// Posição guardada num monitor que foi desconectado deixaria o widget num lugar
/// que nenhuma tela mostra — aberto, respondendo, e impossível de achar. Nesse
/// caso vale mais o lugar padrão do que a lembrança.
fn ponto_visivel(window: &tauri::WebviewWindow, x: f64, y: f64) -> bool {
    let Ok(monitores) = window.available_monitors() else { return true };
    if monitores.is_empty() {
        return true;
    }
    monitores.iter().any(|monitor| {
        let escala = monitor.scale_factor();
        let origem = monitor.position().to_logical::<f64>(escala);
        let tamanho = monitor.size().to_logical::<f64>(escala);
        x >= origem.x && x < origem.x + tamanho.width && y >= origem.y && y < origem.y + tamanho.height
    })
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

    // Reafirma o "sempre por cima" a cada mudança de forma.
    //
    // A janela nasce com `alwaysOnTop` no tauri.conf.json, mas isso é o estado
    // inicial e não uma garantia: no Windows o atributo se perde quando outra
    // janela topmost sobe (instalador, UAC, um jogo em tela cheia) e nada
    // devolve. O HUD então continua visível — atrás de tudo, que é o mesmo que
    // não estar. Reafirmar é barato e idempotente.
    let _ = window.set_always_on_top(true);

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
            // Sem para onde posicionar, ao menos o tamanho é aplicado.
            let _ = window.set_size(tauri::LogicalSize::new(width, height));
            return;
        }
    };
    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);

    let current_size = window
        .outer_size()
        .ok()
        .map(|size| size.to_logical::<f64>(scale));
    let current_position = window
        .outer_position()
        .ok()
        .map(|position| position.to_logical::<f64>(scale));
    let papel = shape.role();
    let mesmo_papel = current_role() == Some(papel);
    let visivel = window.is_visible().unwrap_or(false);

    let guardada = {
        let state = app.state::<crate::AppState>();
        let settings = state.settings.lock();
        match papel {
            HudRole::Dictation => settings.hud_position_dictation,
            HudRole::Reading => settings.hud_position_reading,
        }
    };

    /* De onde vem a posição, em ordem.
     *
     * 1. **A janela já está na tela, no mesmo papel** — a posição atual manda.
     *    É o caso de abrir e fechar a legenda: a pílula não pode voltar ao lugar
     *    de origem só porque mudou de largura. A borda direita fica parada,
     *    porque é para a esquerda que a legenda cresce.
     *
     * 2. **Mudou de papel, ou estava escondida** — a posição guardada **daquele
     *    papel**. A regra antiga olhava só "está visível?", e uma leitura na
     *    tela passava a posição dela para o ditado que começasse em seguida.
     *
     * 3. **Nunca foi arrastado para lá**, ou o monitor sumiu — o padrão do papel. */
    let (x, y) = match (visivel && mesmo_papel, current_size, current_position) {
        (true, Some(size), Some(position)) if (width - size.width).abs() > f64::EPSILON => {
            (position.x + size.width - width, position.y)
        }
        (true, _, Some(position)) => (position.x, position.y),
        _ => {
            let lembrada = guardada.and_then(|posicao| {
                let (x, y) = match papel {
                    HudRole::Dictation => (posicao.x, posicao.y),
                    // Guardada pela borda direita: vale para a pílula estreita e
                    // para a aberta.
                    HudRole::Reading => (posicao.x - width, posicao.y),
                };
                // Confere o meio da janela, e não o canto: um canto um pixel fora
                // da tela ainda deixa o widget inteiro à vista.
                ponto_visivel(&window, x + width / 2.0, y + height / 2.0).then_some((x, y))
            });

            lembrada.unwrap_or_else(|| match papel {
                HudRole::Dictation => ((screen.width - width) / 2.0, screen.height - height - 96.0),
                HudRole::Reading => (screen.width - width - 12.0, (screen.height - height) / 2.0),
            })
        }
    };

    set_current_role(papel);
    tracing::debug!(
        ?shape, largura = width, altura = height,
        tela_l = screen.width, tela_a = screen.height, escala = scale,
        x, y, "posicionando o HUD"
    );

    /* A ordem entre mover e redimensionar depende do sentido.
     *
     * As duas chamadas são independentes e o sistema desenha entre elas. Ao
     * encolher, redimensionar primeiro deixa a janela um quadro estreita na
     * posição da janela larga — a pílula, que é ancorada à direita, aparece
     * quase quatrocentos pixels mais à esquerda e volta. Era esse o piscar no
     * fim do fechamento da legenda.
     *
     * A regra é sempre a mesma: primeiro a operação que não deixa a janela
     * ocupando espaço que ela não deveria. Encolhendo, mover; crescendo,
     * redimensionar. */
    let atual = current_size.map(|size| size.width).unwrap_or(width);

    // Tudo o que o sistema avisar daqui a pouco é consequência desta chamada, e
    // não um arraste. Ver `movement_is_ours`.
    marcar_movimento_programado();

    if width <= atual {
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
    } else {
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
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

/// Mostra o widget sem iniciar uma ação de voz.
///
/// É útil para reposicionar o HUD e para confirmar que o Vox está disponível,
/// sem capturar microfone nem tentar ler a seleção atual.
pub fn show_idle_hud(app: &AppHandle) {
    let Some(window) = app.get_webview_window("hud") else {
        tracing::error!("janela do HUD não encontrada");
        return;
    };
    shape_hud(app, HudShape::Bar);
    let _ = app.emit("vox://hud", serde_json::json!({ "state": "idle" }));
    if let Err(error) = window.show() {
        tracing::error!(?error, "não deu para mostrar o HUD em repouso");
    }
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
