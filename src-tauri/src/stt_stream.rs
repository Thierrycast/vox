//! Transcrição incremental por WebSocket.
//!
//! ## O que muda para quem dita
//!
//! No caminho antigo (`/v1/audio/transcriptions`) o usuário fala, solta o
//! atalho, e só então o áudio inteiro sobe e volta transcrito. O HUD fica na
//! animação de "processando" sem nada para mostrar.
//!
//! Aqui o áudio sobe **enquanto** ele fala, e o texto vai aparecendo. Medido no
//! argos: num áudio de 42 s o primeiro texto surgiu em 4,86 s.
//!
//! Isso **não acelera** a transcrição — o total é praticamente o mesmo. Muda o
//! que a pessoa vê enquanto espera, que é a diferença entre parecer travado e
//! parecer vivo.
//!
//! ## `partial` não é promessa
//!
//! O Vosk revisa a hipótese conforme ouve mais contexto: um parcial pode mudar
//! por completo antes de virar `final`. Por isso o rascunho é **substituído
//! inteiro** a cada mensagem, nunca concatenado — concatenar produz texto
//! duplicado, e é o erro mais comum ao consumir este tipo de API.
//!
//! ## Por que não substitui o caminho em lote
//!
//! O Vosk é rápido mas impreciso: devolve minúsculas, sem pontuação, e erra
//! números. O `groq/whisper-large-v3-turbo` sai pronto para colar. Então o
//! streaming serve ao **retorno visual** e o lote serve ao **texto final** —
//! e o app usa os dois, cada um para o que faz bem.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Quanto áudio vai em cada quadro. 250 ms é o mesmo bloco que o servidor
/// reagrupa internamente; mandar menor só multiplica quadros sem antecipar nada.
const CHUNK_MS: usize = 250;

/// Em 16 kHz mono 16 bits, 250 ms são 4000 amostras.
const SAMPLES_PER_CHUNK: usize = crate::audio::TARGET_RATE as usize * CHUNK_MS / 1000;

/// Teto para o handshake do WebSocket.
///
/// Sem isto, um servidor que aceita a conexão TCP mas nunca completa o
/// handshake deixaria a tarefa pendurada para sempre. Como o streaming é um
/// acréscimo — o ditado funciona sem ele —, desistir rápido é melhor que
/// esperar: 4 s é muito mais que os ~200 ms de um caminho saudável na LAN.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

/// Teto para o `done` depois do `eof`.
///
/// O servidor ainda precisa fechar o último segmento, o que leva um instante.
/// Se demorar mais que isto, o texto acumulado em memória vale mais que
/// continuar esperando.
const FINISH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerMessage {
    Ready {
        #[serde(default)]
        sample_rate: u32,
    },
    Partial {
        text: String,
    },
    Final {
        text: String,
    },
    Done {
        text: String,
        #[serde(default)]
        duration_s: f32,
    },
    Error {
        message: String,
    },
}

/// Texto acumulado de uma sessão de streaming.
///
/// Separa o confirmado do rascunho de propósito: só o confirmado pode ser
/// colado, e o rascunho existe apenas para o HUD mostrar movimento.
#[derive(Default)]
pub struct LiveTranscript {
    confirmed: Vec<String>,
    draft: String,
}

impl LiveTranscript {
    /// O que já está fechado, sem o rascunho.
    pub fn confirmed_text(&self) -> String {
        self.confirmed.join(" ")
    }

    /// O que mostrar no HUD: confirmado mais o rascunho corrente.
    pub fn display_text(&self) -> String {
        let mut texto = self.confirmed_text();
        if !self.draft.is_empty() {
            if !texto.is_empty() {
                texto.push(' ');
            }
            texto.push_str(&self.draft);
        }
        texto
    }

    fn apply_partial(&mut self, text: String) {
        // Substitui, nunca concatena: o Vosk reescreve a hipótese inteira.
        self.draft = text;
    }

    fn apply_final(&mut self, text: String) {
        let limpo = text.trim();
        if !limpo.is_empty() {
            self.confirmed.push(limpo.to_string());
        }
        self.draft.clear();
    }
}

/// Comandos que o dono da sessão manda para a tarefa de rede.
enum Command {
    Audio(Vec<i16>),
    Finish,
}

/// Uma sessão de transcrição incremental em curso.
pub struct SttStream {
    sender: mpsc::UnboundedSender<Command>,
    transcript: Arc<Mutex<LiveTranscript>>,
    /// Dentro de um `Mutex` para o `finish` poder retirá-lo com `&self`: a
    /// sessão vive num `Arc` compartilhado entre a tarefa que bombeia áudio e
    /// o ciclo do ditado, e um método que consumisse `self` não caberia lá.
    worker: Mutex<Option<tokio::task::JoinHandle<Result<String>>>>,
}

impl SttStream {
    /// Abre a conexão e começa a aceitar áudio.
    ///
    /// `base_url` é o mesmo da API (`http://host:porta`); o esquema é traduzido
    /// para `ws`/`wss` aqui, para o chamador não precisar saber disso.
    pub async fn connect(base_url: &str, credentials: Option<(String, String)>) -> Result<Self> {
        let url = websocket_url(base_url)?;
        let request = build_request(&url, credentials)?;

        let (stream, _) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| anyhow!("o handshake em {url} não respondeu em {CONNECT_TIMEOUT:?}"))?
        .with_context(|| format!("conectar em {url}"))?;
        let (mut write, mut read) = stream.split();

        // Declara a taxa antes do primeiro áudio: o servidor só aceita `config`
        // enquanto o reconhecedor ainda não existe.
        let config = serde_json::json!({
            "type": "config",
            "sample_rate": crate::audio::TARGET_RATE,
            "words": false,
        });
        write
            .send(Message::Text(config.to_string()))
            .await
            .context("enviar config")?;

        let transcript = Arc::new(Mutex::new(LiveTranscript::default()));
        let (sender, mut receiver) = mpsc::unbounded_channel::<Command>();

        let transcript_reader = transcript.clone();
        let worker = tokio::spawn(async move {
            // Duas metades: uma bombeia áudio para fora, a outra lê respostas.
            // Ficam no mesmo `select!` para uma falha em qualquer direção
            // encerrar a sessão inteira em vez de deixar a outra pendurada.
            let mut finished = false;

            loop {
                tokio::select! {
                    comando = receiver.recv(), if !finished => {
                        match comando {
                            Some(Command::Audio(samples)) => {
                                let bytes: Vec<u8> = samples
                                    .iter()
                                    .flat_map(|amostra| amostra.to_le_bytes())
                                    .collect();
                                if write.send(Message::Binary(bytes)).await.is_err() {
                                    break;
                                }
                            }
                            Some(Command::Finish) | None => {
                                let eof = serde_json::json!({"type": "eof"}).to_string();
                                let _ = write.send(Message::Text(eof)).await;
                                finished = true;
                            }
                        }
                    }

                    entrada = read.next() => {
                        let Some(entrada) = entrada else { break };
                        let mensagem = match entrada {
                            Ok(mensagem) => mensagem,
                            Err(err) => return Err(anyhow!("conexão caiu: {err}")),
                        };

                        let texto = match mensagem {
                            Message::Text(texto) => texto.to_string(),
                            Message::Close(_) => break,
                            // Ping/pong são tratados pela biblioteca; binário
                            // não é esperado nesta direção.
                            _ => continue,
                        };

                        match serde_json::from_str::<ServerMessage>(&texto) {
                            Ok(ServerMessage::Ready { sample_rate }) => {
                                tracing::debug!(sample_rate, "streaming de transcrição pronto");
                            }
                            Ok(ServerMessage::Partial { text }) => {
                                transcript_reader.lock().apply_partial(text);
                            }
                            Ok(ServerMessage::Final { text }) => {
                                transcript_reader.lock().apply_final(text);
                            }
                            Ok(ServerMessage::Done { text, duration_s }) => {
                                tracing::info!(
                                    chars = text.chars().count(),
                                    duration_s,
                                    "transcrição incremental concluída"
                                );
                                return Ok(text);
                            }
                            Ok(ServerMessage::Error { message }) => {
                                return Err(anyhow!("a speech-api recusou o stream: {message}"));
                            }
                            Err(err) => {
                                // Mensagem desconhecida não derruba a sessão: a
                                // API pode ganhar tipos novos, e um cliente
                                // antigo deve seguir funcionando.
                                tracing::debug!(?err, %texto, "mensagem não reconhecida");
                            }
                        }
                    }
                }
            }

            // Fechou sem `done`: devolve o que foi confirmado até aqui em vez
            // de perder o ditado inteiro por causa do fim da conexão.
            Ok(transcript_reader.lock().confirmed_text())
        });

        Ok(Self { sender, transcript, worker: Mutex::new(Some(worker)) })
    }

    /// Envia áudio já no formato de captura (16 kHz mono).
    ///
    /// Fatia em blocos de 250 ms por conta própria: o chamador entrega o que
    /// tiver acumulado e não precisa se preocupar com o tamanho.
    pub fn push(&self, samples: &[i16]) {
        for bloco in samples.chunks(SAMPLES_PER_CHUNK) {
            if self.sender.send(Command::Audio(bloco.to_vec())).is_err() {
                tracing::debug!("a sessão de streaming já encerrou; áudio descartado");
                return;
            }
        }
    }

    /// O que mostrar no HUD agora — confirmado mais rascunho.
    pub fn display_text(&self) -> String {
        self.transcript.lock().display_text()
    }

    /// Encerra a entrada e espera o `done` do servidor.
    ///
    /// O texto de `done` é mais completo que o acumulado em memória: o Vosk
    /// fecha o último segmento com o contexto inteiro, e a hipótese que estava
    /// como rascunho pode virar frase.
    ///
    /// Se já foi encerrada antes, devolve o que estiver confirmado — chamar
    /// duas vezes não é erro para quem trata caminho de falha.
    pub async fn finish(&self) -> Result<String> {
        let _ = self.sender.send(Command::Finish);
        let Some(worker) = self.worker.lock().take() else {
            return Ok(self.transcript.lock().confirmed_text());
        };

        match tokio::time::timeout(FINISH_TIMEOUT, worker).await {
            Ok(resultado) => resultado.context("a tarefa de streaming falhou")?,
            Err(_) => {
                // O servidor não fechou a tempo. O acumulado ainda vale: é
                // melhor entregar texto imperfeito que segurar o ditado.
                tracing::warn!("o streaming não fechou em {FINISH_TIMEOUT:?}; usando o acumulado");
                Ok(self.transcript.lock().confirmed_text())
            }
        }
    }

    /// Encerra sem esperar resultado — para quando o usuário cancela.
    pub fn abort(&self) {
        if let Some(worker) = self.worker.lock().take() {
            worker.abort();
        }
    }
}

/// `http://host:porta` → `ws://host:porta/stt/stream`.
fn websocket_url(base_url: &str) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let convertido = if let Some(resto) = base.strip_prefix("https://") {
        format!("wss://{resto}")
    } else if let Some(resto) = base.strip_prefix("http://") {
        format!("ws://{resto}")
    } else {
        return Err(anyhow!("endereço da API sem esquema http/https: {base_url}"));
    };
    Ok(format!("{convertido}/stt/stream"))
}

/// Monta o handshake, com basicAuth quando a API estiver atrás do `panel-auth`.
fn build_request(
    url: &str,
    credentials: Option<(String, String)>,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
    use base64::Engine;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = url.into_client_request().context("montar handshake")?;

    if let Some((usuario, senha)) = credentials {
        let par = base64::engine::general_purpose::STANDARD
            .encode(format!("{usuario}:{senha}"));
        request.headers_mut().insert(
            "Authorization",
            format!("Basic {par}").parse().context("cabeçalho de autorização")?,
        );
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traduz_o_esquema_para_websocket() {
        assert_eq!(
            websocket_url("http://100.122.39.56:8010").unwrap(),
            "ws://100.122.39.56:8010/stt/stream"
        );
        assert_eq!(
            websocket_url("https://speech-api.lab.home/").unwrap(),
            "wss://speech-api.lab.home/stt/stream"
        );
    }

    #[test]
    fn recusa_endereco_sem_esquema() {
        assert!(websocket_url("100.122.39.56:8010").is_err());
    }

    #[test]
    fn parcial_substitui_em_vez_de_concatenar() {
        let mut vivo = LiveTranscript::default();
        vivo.apply_partial("bom".into());
        vivo.apply_partial("bom dia".into());
        vivo.apply_partial("bom dia vamos".into());
        // Se concatenasse, aqui estaria "bom bom dia bom dia vamos".
        assert_eq!(vivo.display_text(), "bom dia vamos");
    }

    #[test]
    fn final_confirma_e_limpa_o_rascunho() {
        let mut vivo = LiveTranscript::default();
        vivo.apply_partial("bom dia vamo".into());
        vivo.apply_final("Bom dia, vamos falar.".into());
        assert_eq!(vivo.confirmed_text(), "Bom dia, vamos falar.");
        assert_eq!(vivo.display_text(), "Bom dia, vamos falar.");
    }

    #[test]
    fn segmentos_confirmados_se_acumulam_com_espaco() {
        let mut vivo = LiveTranscript::default();
        vivo.apply_final("Primeira frase.".into());
        vivo.apply_final("Segunda frase.".into());
        assert_eq!(vivo.confirmed_text(), "Primeira frase. Segunda frase.");
    }

    #[test]
    fn final_vazio_nao_polui_o_texto() {
        let mut vivo = LiveTranscript::default();
        vivo.apply_final("   ".into());
        assert_eq!(vivo.confirmed_text(), "");
    }

    #[test]
    fn rascunho_aparece_depois_do_confirmado() {
        let mut vivo = LiveTranscript::default();
        vivo.apply_final("Primeira.".into());
        vivo.apply_partial("segunda em andamento".into());
        assert_eq!(vivo.display_text(), "Primeira. segunda em andamento");
        // Mas só o confirmado é colável.
        assert_eq!(vivo.confirmed_text(), "Primeira.");
    }
}
