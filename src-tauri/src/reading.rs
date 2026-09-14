//! Leitura de texto em voz alta.
//!
//! ## Por que existe uma fila
//!
//! A speech-api devolve o arquivo pronto, sem streaming. Medido no argos:
//! **gerar leva 1,06× o tempo de falar** (7,94 s de geração para 7,50 s de
//! áudio), e há um piso de ~740 ms por chamada.
//!
//! Isso tem duas consequências que mandam no desenho:
//!
//! - Pedir o texto inteiro de uma vez faz o usuário esperar dezenas de segundos
//!   pelo primeiro som. Por isso o texto é fatiado, com o primeiro trecho curto
//!   de propósito.
//! - Como gerar é **mais lento** que falar, a fila esvazia sozinha ao longo de um
//!   texto longo — o déficit de 6% acumula e nunca se recupera. Por isso o player
//!   acumula uma dianteira antes de começar; sem ela, a leitura engasga no meio.
//!
//! `tts_gate: limit 1` no servidor significa que pedir trechos em paralelo não
//! adianta — eles enfileiram. A geração aqui é sequencial de propósito.

use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::api::{self, SpeechApi};
use crate::sounds::{Cue, SoundBank};
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadingState {
    Idle,
    Generating,
    Playing,
    Paused,
    Complete,
    Failed,
}

/// Um trecho de texto e, quando pronto, o áudio dele.
pub struct Segment {
    pub text: String,
    pub audio: Option<Vec<u8>>,
    /// Duração real, conhecida só depois de gerar.
    pub seconds: f32,
}

#[derive(Default)]
pub struct Queue {
    segments: Vec<Segment>,
    /// Índice do trecho tocando agora.
    cursor: usize,
    state: Option<ReadingState>,
    generation: u64,
    /// Somatórios para medir a razão de geração desta leitura.
    generated_seconds: f32,
    audio_seconds: f32,
}

impl Queue {
    pub fn state(&self) -> ReadingState {
        self.state.unwrap_or(ReadingState::Idle)
    }

    /// Segundos já sintetizados e ainda não tocados.
    pub fn buffered_seconds(&self) -> f32 {
        self.segments
            .iter()
            .skip(self.cursor)
            .filter(|segment| segment.audio.is_some())
            .map(|segment| segment.seconds)
            .sum()
    }

    /// Estimativa do que ainda falta tocar.
    ///
    /// A referência é ~18 caracteres por segundo de fala, medida no `pf_dora`
    /// a 1×. É grosseira de propósito: serve só para dimensionar a dianteira,
    /// e a duração exata vem do próprio áudio quando ele existe.
    pub fn remaining_estimate_seconds(&self) -> f32 {
        let chars: usize = self
            .segments
            .iter()
            .skip(self.cursor)
            .map(|segment| segment.text.chars().count())
            .sum();
        chars as f32 / 18.0
    }

    /// Quantas vezes o tempo de fala custa para gerar, medido nesta leitura.
    ///
    /// **Não é constante.** Com o argos ocioso medimos 1,06×; com ele em carga
    /// (load 15, swap cheio) a mesma chamada foi a 2,7×. Um percentual fixo de
    /// dianteira serve para uma das duas situações e falha na outra, por isso a
    /// razão é medida a cada trecho em vez de assumida.
    ///
    /// Devolve `None` enquanto não há amostra suficiente.
    pub fn measured_ratio(&self) -> Option<f32> {
        if self.audio_seconds < 0.5 {
            return None;
        }
        Some(self.generated_seconds / self.audio_seconds)
    }

    /// Dianteira necessária para a fila não esvaziar até o fim.
    ///
    /// Se gerar custa `r` vezes o tempo de falar, ao longo de `d` segundos
    /// restantes a geração fica `(r - 1) * d` atrás. Essa é exatamente a
    /// dianteira que precisa existir antes de dar play. Com `r <= 1` a geração
    /// alcança a reprodução sozinha e basta o primeiro trecho.
    pub fn required_lead_seconds(&self, safety: f32) -> f32 {
        let Some(ratio) = self.measured_ratio() else {
            return 0.0;
        };
        let deficit = (ratio - 1.0).max(0.0);
        deficit * self.remaining_estimate_seconds() * (1.0 + safety)
    }
}

pub struct Reader {
    queue: Arc<Mutex<Queue>>,
    api: Arc<SpeechApi>,
    sounds: Arc<SoundBank>,
}

impl Reader {
    pub fn new(api: Arc<SpeechApi>, sounds: Arc<SoundBank>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Queue::default())),
            api,
            sounds,
        }
    }

    pub fn state(&self) -> ReadingState {
        self.queue.lock().state()
    }

    /// Interrompe o que estiver tocando e esquece a fila.
    ///
    /// Incrementa a geração: qualquer tarefa de síntese ainda em voo percebe que
    /// pertence a uma leitura antiga e se descarta em silêncio, sem tocar áudio
    /// de um texto que o usuário já abandonou.
    pub fn stop(&self, app: &AppHandle) {
        let mut queue = self.queue.lock();
        queue.generation += 1;
        queue.segments.clear();
        queue.cursor = 0;
        queue.state = Some(ReadingState::Idle);
        drop(queue);
        crate::dictation::hide_hud(app, crate::dictation::HudRole::Reading);
        emit(app, ReadingState::Idle, None);
    }

    pub fn pause(&self, app: &AppHandle) {
        let mut queue = self.queue.lock();
        if queue.state() != ReadingState::Playing {
            tracing::debug!(estado = ?queue.state(), "pausa ignorada: não está tocando");
            return;
        }
        queue.state = Some(ReadingState::Paused);
        drop(queue);
        self.sounds.play(Cue::SpeakPause);
        emit(app, ReadingState::Paused, None);
    }

    pub fn resume(&self, app: &AppHandle) {
        let mut queue = self.queue.lock();
        if queue.state() != ReadingState::Paused {
            tracing::debug!(estado = ?queue.state(), "retomada ignorada: não está pausado");
            return;
        }
        queue.state = Some(ReadingState::Playing);
        drop(queue);
        emit(app, ReadingState::Playing, None);
    }

    /// Prepara o texto no servidor e devolve o que de fato vai ser falado.
    ///
    /// Separado da leitura porque quem entra pela extensão precisa da lista de
    /// trechos **antes** de a fala começar, para casar cada um com o pedaço do
    /// DOM que vai destacar. Preparar duas vezes seria pior que um round-trip a
    /// mais: com a correção ligada, seriam dois modelos e o dobro da espera.
    pub async fn prepare(&self, text: &str, normalize: bool, narrate_tables: bool) -> String {
        match self.api.prepare_text(text, normalize, narrate_tables).await {
            Ok(preparado) if !preparado.trim().is_empty() => preparado,
            Ok(_) => {
                tracing::warn!("preparo devolveu texto vazio; usando o original");
                text.to_string()
            }
            Err(err) => {
                // O servidor limpa de novo na síntese, então perder esta etapa
                // custa o corte pior, não a marcação falada. Interromper a
                // leitura por isso seria trocar um problema pequeno por um
                // grande.
                tracing::warn!(?err, "preparo do texto falhou; seguindo com o original");
                text.to_string()
            }
        }
    }

    /// Lê um texto do começo ao fim, preparando-o antes.
    pub async fn speak(
        &self,
        app: AppHandle,
        text: String,
        voice: String,
        speed: f32,
        prebuffer_ratio: f32,
    ) -> Result<()> {
        self.speak_inner(app, text, voice, speed, prebuffer_ratio, false).await
    }

    /// Como `speak`, mas para texto que já passou por `prepare`.
    pub async fn speak_prepared(
        &self,
        app: AppHandle,
        text: String,
        voice: String,
        speed: f32,
        prebuffer_ratio: f32,
    ) -> Result<()> {
        self.speak_inner(app, text, voice, speed, prebuffer_ratio, true).await
    }

    async fn speak_inner(
        &self,
        app: AppHandle,
        text: String,
        voice: String,
        speed: f32,
        prebuffer_ratio: f32,
        already_prepared: bool,
    ) -> Result<()> {
        // A geração sobe antes de qualquer trabalho: quem parar a leitura durante
        // o preparo do texto — que pode levar segundos com a correção ligada —
        // invalida esta aqui, e o resto do método percebe e desiste.
        let generation = {
            let mut queue = self.queue.lock();
            queue.generation += 1;
            queue.segments.clear();
            queue.cursor = 0;
            queue.state = Some(ReadingState::Generating);
            queue.generation
        };

        // O player flutuante vira coluna e encosta na borda; a janela de texto
        // sobe junto. Um dá controle sem tirar o foco do que se está lendo, a
        // outra dá o texto acompanhado.
        // O estado vai primeiro, e só depois a janela aparece. Na ordem
        // inversa o HUD reaparecia com o último quadro do ditado ainda no DOM —
        // o "Copiado" — até o evento chegar e o player substituir o conteúdo.
        let (abrir_leitor, com_legenda, normalizar, narrar_tabelas) = {
            let state = app.state::<AppState>();
            let settings = state.settings.lock();
            (
                settings.open_reader_on_read,
                settings.reading_captions,
                settings.normalize_before_reading,
                settings.narrate_tables,
            )
        };

        emit(&app, ReadingState::Generating, None);
        // A forma já nasce certa: abrir estreito e alargar em seguida faria a
        // pílula saltar na tela a cada leitura de quem deixa a legenda ligada.
        crate::dictation::shape_hud(&app, if com_legenda {
            crate::dictation::HudShape::ColumnCaptions
        } else {
            crate::dictation::HudShape::Column
        });
        if let Some(hud) = app.get_webview_window("hud") {
            let _ = crate::presence::reveal(&hud);
        }

        // A janela já existe escondida desde a partida; mostrar custa um quadro.
        // Ela sobe antes do primeiro áudio para o texto aparecer imediatamente,
        // em vez de o usuário encarar o nada enquanto a voz é gerada.
        if abrir_leitor {
            if let Some(window) = app.get_webview_window("reader") {
                crate::raise(&window);
            }
        }

        // O texto é preparado **antes** de ser fatiado.
        //
        // A limpeza tira a marcação que o sintetizador leria em voz alta — o
        // "asterisco asterisco" de um negrito em Markdown. Fatiar antes de
        // limpar seria pior do que parece: um título ou uma linha de tabela
        // viram fronteira de frase falsa, e os trechos sairiam cortados no lugar
        // errado. Uma chamada, com o texto inteiro, e o resto do fluxo trabalha
        // com o texto que de fato vai ser falado.
        // Já preparado é o caso de quem entrou pela extensão: ela recebeu os
        // trechos antes da fala começar, e prepará-los de novo poderia mudá-los
        // debaixo do destaque que ela já montou.
        let texto = if already_prepared {
            text.clone()
        } else {
            self.prepare(&text, normalizar, narrar_tabelas).await
        };

        // Parar durante o preparo invalida esta leitura.
        if self.queue.lock().generation != generation {
            tracing::debug!("leitura descartada durante o preparo do texto");
            return Ok(());
        }

        let chunks = api::split_text(&texto);
        if chunks.is_empty() {
            return Ok(());
        }

        {
            let mut queue = self.queue.lock();
            if queue.generation != generation {
                return Ok(());
            }
            queue.segments = chunks
                .into_iter()
                .map(|trecho| Segment { text: trecho, audio: None, seconds: 0.0 })
                .collect();
        }

        // Os trechos vão para a ponte em **toda** leitura, e não só na que veio
        // da extensão. Sem isto, uma leitura começada pelo atalho global era
        // invisível para ela: o texto era falado e a página não destacava nada,
        // porque a lista do lado de lá continuava vazia.
        {
            let queue = self.queue.lock();
            let trechos: Vec<String> = queue.segments.iter().map(|s| s.text.clone()).collect();
            drop(queue);
            app.state::<AppState>().bridge.set_segments(trechos);
        }

        // O front desenha o texto inteiro antes de qualquer áudio existir, para
        // o leitor já aparecer legível e o destaque só precisar andar.
        {
            let queue = self.queue.lock();
            let plan: Vec<&str> = queue.segments.iter().map(|s| s.text.as_str()).collect();
            let _ = app.emit("vox://reading-plan", serde_json::json!({ "segments": plan }));
        }

        // Sem som de início aqui de propósito: o retorno de que a leitura
        // começou é a própria voz, e um bipe antes dela só atrasa o que a
        // pessoa pediu. Erro, pausa e conclusão continuam soando — esses são
        // avisos de que algo mudou sem a voz dizer.

        let mut index = 0;
        let mut playback_started = false;

        loop {
            // Uma leitura nova (ou um stop) invalidou esta.
            if self.queue.lock().generation != generation {
                tracing::debug!("leitura antiga descartada");
                return Ok(());
            }

            let Some(chunk_text) = self.queue.lock().segments.get(index).map(|s| s.text.clone())
            else {
                break;
            };

            tracing::debug!(trecho = index, chars = chunk_text.chars().count(), "sintetizando");
            let started = std::time::Instant::now();
            match self.api.speak(&chunk_text, &voice, speed).await {
                Ok((meta, audio)) => {
                    let elapsed = started.elapsed().as_secs_f32();
                    let mut queue = self.queue.lock();
                    if queue.generation != generation {
                        return Ok(());
                    }
                    // Sem decodificar o mp3, estimamos pela taxa que a API usa
                    // (48 kbps). Serve para medir a dianteira; o tempo exato
                    // quem sabe é o elemento de áudio no front.
                    let seconds = meta.bytes.max(1) as f32 / 6000.0;

                    if let Some(segment) = queue.segments.get_mut(index) {
                        segment.seconds = seconds;
                        segment.audio = Some(audio);
                    }

                    // Trecho vindo do cache não diz nada sobre a velocidade da
                    // máquina; incluí-lo faria a razão parecer melhor do que é.
                    if !meta.cached {
                        queue.generated_seconds += elapsed;
                        queue.audio_seconds += seconds;
                    }

                    let buffered = queue.buffered_seconds();
                    let required = queue.required_lead_seconds(prebuffer_ratio);
                    let ratio = queue.measured_ratio();
                    drop(queue);

                    let is_last = index + 1 >= self.total();
                    if !playback_started && (buffered >= required || is_last) {
                        playback_started = true;
                        let mut queue = self.queue.lock();
                        queue.state = Some(ReadingState::Playing);
                        drop(queue);
                        emit(&app, ReadingState::Playing, None);
                        tracing::info!(
                            dianteira_s = buffered,
                            necessaria_s = required,
                            razao = ratio,
                            "começando a tocar"
                        );
                    }

                    self.push_audio_to_front(&app, index);
                    tracing::debug!(trecho = index, "áudio entregue ao front");
                }
                Err(err) => {
                    tracing::error!(?err, trecho = index, "síntese falhou");
                    let mut queue = self.queue.lock();
                    if queue.generation != generation {
                        return Ok(());
                    }
                    queue.state = Some(ReadingState::Failed);
                    drop(queue);
                    self.sounds.play(Cue::SpeakError);
                    // O motivo real vai junto. "Não deu para gerar a voz" sozinho
                    // é a única informação que o usuário já tinha — ele viu que
                    // não funcionou. O que ele não sabe é se foi a rede, a voz
                    // configurada ou o servidor, e é isso que decide o que fazer.
                    emit(&app, ReadingState::Failed, Some(&motivo_legivel(&err)));
                    return Err(err);
                }
            }

            index += 1;
            if index >= self.total() {
                break;
            }
        }

        Ok(())
    }

    fn total(&self) -> usize {
        self.queue.lock().segments.len()
    }

    /// Manda o áudio de um trecho para o front, que o enfileira no elemento de
    /// reprodução. Áudio vai em base64 porque é o que atravessa a ponte do Tauri
    /// sem um servidor local só para isso.
    fn push_audio_to_front(&self, app: &AppHandle, index: usize) {
        use base64::Engine;

        let queue = self.queue.lock();
        let Some(segment) = queue.segments.get(index) else { return };
        let Some(audio) = &segment.audio else { return };

        let encoded = base64::engine::general_purpose::STANDARD.encode(audio);
        let text = segment.text.clone();
        let total = queue.segments.len();
        drop(queue);

        let _ = app.emit("vox://reading-chunk", serde_json::json!({
            "index": index,
            "total": total,
            "text": text,
            "audio": format!("data:audio/mpeg;base64,{encoded}"),
        }));
    }

    /// O front avisa qual trecho começou a tocar.
    ///
    /// Sem isto o `cursor` ficaria em zero e a dianteira necessária seria
    /// calculada sobre o texto inteiro, não sobre o que ainda falta.
    pub fn set_cursor(&self, index: usize) {
        let mut queue = self.queue.lock();
        if index < queue.segments.len() {
            queue.cursor = index;
        }
    }

    /// O front avisa que a reprodução mudou de estado por conta própria.
    ///
    /// Quem toca o áudio é a janela de leitura, então é ela que sabe se está
    /// tocando de verdade — e não este lado, que só sabe o que pediu. Sem este
    /// aviso os dois divergiam: bastava avançar 15 s numa leitura terminada para
    /// o áudio voltar a tocar enquanto aqui o estado continuava `Complete`, e o
    /// atalho seguinte começava uma leitura nova em vez de pausar.
    ///
    /// Não emite evento: a informação veio do front, e devolvê-la como evento
    /// faria os dois se avisarem em círculo.
    pub fn sync_state(&self, state: ReadingState) {
        let mut queue = self.queue.lock();
        let anterior = queue.state();

        // Parado é parado. Depois de um `stop` a janela ainda dispara o evento
        // de pausa do elemento de áudio, e aceitá-lo ressuscitaria uma leitura
        // que já não existe — o atalho seguinte tentaria retomar o nada.
        if anterior == ReadingState::Idle || anterior == state {
            return;
        }

        queue.state = Some(state);
        drop(queue);

        if state == ReadingState::Paused {
            self.sounds.play(Cue::SpeakPause);
        }
    }

    /// Chamado pelo front quando o último trecho termina de tocar.
    pub fn finished(&self, app: &AppHandle) {
        let mut queue = self.queue.lock();
        queue.state = Some(ReadingState::Complete);
        drop(queue);
        self.sounds.play(Cue::SpeakComplete);
        emit(app, ReadingState::Complete, None);
    }
}

/// Achata a cadeia de erros numa frase que cabe na tela.
///
/// O `anyhow` empilha contexto ("pedir síntese" → "a speech-api respondeu 400");
/// mostrar a pilha inteira num balão de 360px não ajuda ninguém. A causa mais
/// funda é a que diz o que aconteceu de fato, e é ela que vai na frente.
fn motivo_legivel(err: &anyhow::Error) -> String {
    let fundo = err.chain().last().map(|causa| causa.to_string());
    let texto = fundo.unwrap_or_else(|| err.to_string());

    const LIMITE: usize = 140;
    if texto.chars().count() <= LIMITE {
        return texto;
    }
    texto.chars().take(LIMITE - 1).collect::<String>() + "…"
}

fn emit(app: &AppHandle, state: ReadingState, message: Option<&str>) {
    let _ = app.emit("vox://reading", serde_json::json!({
        "state": state,
        "message": message,
    }));
}


#[cfg(test)]
mod tests {
    use super::*;

    fn queue_with(segments: &[(f32, bool)]) -> Queue {
        Queue {
            segments: segments
                .iter()
                .map(|(seconds, ready)| Segment {
                    text: "x".repeat(180),
                    audio: ready.then(|| vec![0u8; 8]),
                    seconds: *seconds,
                })
                .collect(),
            cursor: 0,
            state: Some(ReadingState::Playing),
            generation: 1,
            generated_seconds: 0.0,
            audio_seconds: 0.0,
        }
    }

    /// Simula ter gerado `audio` segundos de fala gastando `spent` segundos.
    fn with_measurement(mut queue: Queue, spent: f32, audio: f32) -> Queue {
        queue.generated_seconds = spent;
        queue.audio_seconds = audio;
        queue
    }

    #[test]
    fn buffered_conta_so_o_que_esta_pronto_e_a_frente() {
        let mut queue = queue_with(&[(3.0, true), (4.0, true), (5.0, false)]);
        assert_eq!(queue.buffered_seconds(), 7.0);
        queue.cursor = 1;
        assert_eq!(queue.buffered_seconds(), 4.0);
    }

    #[test]
    fn estimativa_cresce_com_o_texto() {
        let curto = queue_with(&[(0.0, false)]);
        let longo = queue_with(&[(0.0, false), (0.0, false)]);
        assert!(longo.remaining_estimate_seconds() > curto.remaining_estimate_seconds());
    }

    #[test]
    fn sem_amostra_nao_arrisca_palpite() {
        let queue = queue_with(&[(0.0, false), (0.0, false)]);
        assert_eq!(queue.measured_ratio(), None);
        // Sem medida, não segura a reprodução: melhor começar e medir.
        assert_eq!(queue.required_lead_seconds(0.1), 0.0);
    }

    #[test]
    fn maquina_rapida_nao_exige_dianteira() {
        // O caso medido com o argos ocioso: 1,06× — a geração quase acompanha.
        let queue = with_measurement(queue_with(&[(7.5, true), (7.5, false)]), 7.94, 7.50);
        let ratio = queue.measured_ratio().unwrap();
        assert!((ratio - 1.06).abs() < 0.01, "razão medida foi {ratio}");
        // Déficit de 6% sobre o que resta: pouca coisa.
        assert!(queue.required_lead_seconds(0.0) < queue.remaining_estimate_seconds() * 0.1);
    }

    #[test]
    fn maquina_carregada_exige_dianteira_grande() {
        // O caso medido com o argos em load 15: 2,7×.
        let queue = with_measurement(queue_with(&[(7.0, true), (7.0, false)]), 18.9, 7.0);
        let ratio = queue.measured_ratio().unwrap();
        assert!(ratio > 2.5, "razão medida foi {ratio}");
        // Precisa de mais de uma vez o que resta: a geração fica muito atrás.
        let required = queue.required_lead_seconds(0.0);
        assert!(
            required > queue.remaining_estimate_seconds(),
            "exigiu {required}s para {}s restantes",
            queue.remaining_estimate_seconds()
        );
    }

    #[test]
    fn margem_de_seguranca_aumenta_a_dianteira() {
        let queue = with_measurement(queue_with(&[(7.0, true), (7.0, false)]), 18.9, 7.0);
        assert!(queue.required_lead_seconds(0.2) > queue.required_lead_seconds(0.0));
    }

    #[test]
    fn dianteira_encolhe_conforme_a_leitura_avanca() {
        let mut queue = with_measurement(
            queue_with(&[(7.0, true), (7.0, true), (7.0, true)]), 18.9, 7.0);
        let no_comeco = queue.required_lead_seconds(0.0);
        queue.cursor = 2;
        assert!(queue.required_lead_seconds(0.0) < no_comeco,
                "com menos texto pela frente, exige menos dianteira");
    }

    #[test]
    fn fila_vazia_nao_tem_dianteira() {
        let queue = Queue::default();
        assert_eq!(queue.buffered_seconds(), 0.0);
        assert_eq!(queue.state(), ReadingState::Idle);
    }
}
