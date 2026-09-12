//! Cliente da speech-api.
//!
//! Duas decisões aqui existem por causa de latência medida, não por gosto:
//!
//! 1. **A conexão é pré-aquecida** quando o ditado começa, não quando termina.
//!    O handshake TCP+TLS custa 100–300 ms, e pagá-lo enquanto o usuário ainda
//!    fala é de graça; pagá-lo depois aparece como demora.
//!
//! 2. **A leitura é fatiada no cliente.** A API devolve o arquivo pronto, sem
//!    streaming, e gerar leva 1,06× o tempo de falar (medido: 7,94 s para 7,50 s
//!    de áudio). Um trecho curto sai em ~740 ms, então quebrar o texto tira a
//!    espera inicial de dezenas de segundos para menos de um.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Acima disso a espera passa a incomodar; abaixo, a prosódia sofre.
const CHUNK_TARGET_CHARS: usize = 220;
/// O primeiro trecho é curto de propósito: ele define o tempo até o primeiro som.
const FIRST_CHUNK_CHARS: usize = 90;

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct SpeechApi {
    client: reqwest::Client,
    base_url: String,
    credentials: Option<Credentials>,
}

/// Resposta de `/text/rewrite`.
#[derive(Debug, Deserialize)]
pub struct RewriteResult {
    pub text: String,
    #[serde(default)]
    pub rewritten: RewriteReport,
}

#[derive(Debug, Default, Deserialize)]
pub struct RewriteReport {
    #[serde(default)]
    pub applied: bool,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub skipped: Option<String>,
}

/// Resposta de `/text/prepare`.
///
/// Só o texto interessa ao fluxo; o resto do corpo é diagnóstico, e desses só o
/// erro da normalização vale registrar — ele diz por que a correção não saiu.
#[derive(Debug, Deserialize)]
struct PrepareResponse {
    text: String,
    #[serde(default)]
    normalized: Option<NormalizeReport>,
    #[serde(default)]
    tables: Option<TableReport>,
}

/// O que aconteceu com as tabelas do texto.
///
/// Registrado no log porque é a única forma de saber, depois, por que uma
/// leitura soou como uma fila de células em vez de uma explicação.
#[derive(Debug, Deserialize)]
struct TableReport {
    #[serde(default)]
    count: usize,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NormalizeReport {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TtsResponse {
    /// Caminho para buscar o áudio. `audio_file` da API é o mesmo nome sem a
    /// pasta, então não guardamos os dois.
    pub url_path: String,
    /// Nem sempre é a voz pedida: a API cai na padrão se a escolhida sumiu.
    pub voice: String,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
}

#[derive(Debug, Serialize)]
struct TtsRequest<'a> {
    text: &'a str,
    voice: &'a str,
    speed: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceCatalog {
    pub default: String,
    #[serde(default)]
    pub custom: Vec<String>,
    #[serde(default)]
    pub base: Vec<String>,
}

impl SpeechApi {
    pub fn new(base_url: impl Into<String>, credentials: Option<Credentials>) -> Result<Self> {
        let client = reqwest::Client::builder()
            // Abaixo do keep-alive do uvicorn, que é de 5 s por padrão.
            //
            // Com 300 s aqui, o cliente guardava por cinco minutos uma conexão
            // que o servidor tinha fechado cinco segundos depois de usar — e
            // entregava essa conexão morta para a requisição seguinte. O
            // sintoma foi uma leitura que sintetizou o primeiro trecho e
            // congelou no segundo, sem erro, até o timeout estourar. Descartar
            // antes do servidor custa um handshake de LAN entre leituras, que
            // é ruído perto disso.
            .pool_idle_timeout(Duration::from_secs(4))
            .pool_max_idle_per_host(4)
            // Generoso porque o Kokoro é lento (mede-se em dezenas de segundos
            // por trecho), mas não tanto quanto era: em 300 s uma falha de rede
            // aparecia como cinco minutos de tela parada.
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(8))
            .user_agent(concat!("vox/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("montar cliente HTTP")?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            credentials,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self.client.request(method, format!("{}{}", self.base_url, path));
        match &self.credentials {
            Some(creds) => builder.basic_auth(&creds.username, Some(&creds.password)),
            None => builder,
        }
    }

    /// Abre a conexão adiantado. Erro aqui é ignorado de propósito: é otimização,
    /// não pré-requisito — se falhar, a chamada real tenta de novo e reporta.
    pub async fn prewarm(&self) {
        let started = std::time::Instant::now();
        match self.request(reqwest::Method::GET, "/health").send().await {
            Ok(_) => tracing::debug!(ms = started.elapsed().as_millis(), "conexão pré-aquecida"),
            Err(err) => tracing::debug!(?err, "pré-aquecimento falhou; seguindo"),
        }
    }

    pub async fn health(&self) -> Result<serde_json::Value> {
        let response = self.request(reqwest::Method::GET, "/health").send().await?;
        ensure_ok(&response)?;
        Ok(response.json().await?)
    }

    pub async fn voices(&self) -> Result<VoiceCatalog> {
        let response = self.request(reqwest::Method::GET, "/voices/names").send().await?;
        ensure_ok(&response)?;
        Ok(response.json().await?)
    }

    /// Transcreve um WAV. `model` é `vosk` (local) ou `groq/whisper-large-v3-turbo`.
    ///
    /// O `prompt` ensina vocabulário ao modelo remoto: nomes próprios e termos
    /// técnicos que ele não teria como adivinhar. Sem ele "Traefik" volta como
    /// "trafic" toda vez. O Vosk local ignora o campo — reconhecedor offline não
    /// tem onde encaixar contexto.
    pub async fn transcribe(
        &self,
        wav: Vec<u8>,
        model: &str,
        prompt: &str,
    ) -> Result<TranscriptionResponse> {
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", model.to_string())
            .text("response_format", "json");
        if !prompt.trim().is_empty() {
            form = form.text("prompt", prompt.to_string());
        }

        let response = self
            .request(reqwest::Method::POST, "/v1/audio/transcriptions")
            .multipart(form)
            .send()
            .await
            .context("enviar áudio para transcrição")?;

        ensure_ok(&response)?;
        Ok(response.json().await.context("ler transcrição")?)
    }

    /// Gera a fala de um trecho e devolve os bytes do mp3 já baixados.
    /// Devolve o texto pronto para virar voz: sem marcação e, se pedido, com
    /// acentuação e ortografia corrigidas.
    ///
    /// Chamado **uma vez, com o texto inteiro**, antes de fatiar. Duas razões:
    /// fatiar Markdown corta a frase no lugar errado — um título ou uma linha de
    /// tabela viram fronteira falsa —, e normalizar trecho a trecho multiplicaria
    /// o custo do modelo e ainda lhe daria menos contexto.
    ///
    /// Falhar aqui não impede a leitura: o texto original ainda é legível, só não
    /// está limpo. A API também limpa de novo na síntese, e a limpeza é
    /// idempotente.
    pub async fn prepare_text(
        &self,
        text: &str,
        normalize: bool,
        narrate_tables: bool,
    ) -> Result<String> {
        let response = self
            .request(reqwest::Method::POST, "/text/prepare")
            .json(&serde_json::json!({
                "text": text,
                "sanitize": true,
                "normalize": normalize,
                "narrate_tables": narrate_tables,
            }))
            .send()
            .await
            .context("pedir a preparação do texto")?;

        ensure_ok(&response)?;
        let corpo: PrepareResponse = response.json().await.context("ler o texto preparado")?;

        if let Some(relato) = corpo.tables.filter(|relato| relato.count > 0) {
            match relato.error {
                Some(motivo) => tracing::warn!(
                    tabelas = relato.count, motivo, "tabelas não foram narradas"),
                None => tracing::info!(
                    tabelas = relato.count, ms = relato.elapsed_ms, "tabelas narradas"),
            }
        }

        if let Some(motivo) = corpo.normalized.and_then(|estado| estado.error) {
            // A correção é opcional por natureza; perdê-la não vale interromper a
            // leitura, mas some do log se não for dita aqui.
            tracing::warn!(motivo, "normalização do texto não foi aplicada");
        }

        Ok(corpo.text)
    }

    /// Reescreve a transcrição segundo um preset, e devolve o texto final.
    ///
    /// Nunca falha para quem chamou: o servidor devolve o texto original quando o
    /// modelo não responde, e aqui um erro de rede vira o mesmo. Perder o que foi
    /// ditado porque a reescrita não deu certo seria trocar "ficou menos bonito"
    /// por "sumiu".
    pub async fn rewrite_text(
        &self,
        text: &str,
        preset: &str,
        intensity: u8,
        model: Option<&str>,
    ) -> Result<RewriteResult> {
        let mut corpo = serde_json::json!({
            "text": text,
            "preset": preset,
            "intensity": intensity,
        });
        if let Some(nome) = model.filter(|valor| !valor.trim().is_empty()) {
            corpo["model"] = serde_json::Value::String(nome.to_string());
        }

        let response = self
            .request(reqwest::Method::POST, "/text/rewrite")
            .json(&corpo)
            .send()
            .await
            .context("pedir a reescrita do texto")?;

        ensure_ok(&response)?;
        response.json().await.context("ler o texto reescrito")
    }

    /// Os moldes de reescrita que o servidor conhece.
    ///
    /// A lista vem de lá para o painel não guardar uma cópia: um preset novo no
    /// servidor aparece no app sem uma versão nova dele.
    pub async fn rewrite_presets(&self) -> Result<serde_json::Value> {
        let response = self
            .request(reqwest::Method::GET, "/text/presets")
            .send()
            .await
            .context("listar os presets de reescrita")?;
        ensure_ok(&response)?;
        Ok(response.json().await?)
    }

    pub async fn speak(&self, text: &str, voice: &str, speed: f32) -> Result<(TtsResponse, Vec<u8>)> {
        let response = self
            .request(reqwest::Method::POST, "/tts")
            .json(&TtsRequest { text, voice, speed })
            .send()
            .await
            .context("pedir síntese")?;

        ensure_ok(&response)?;
        let meta: TtsResponse = response.json().await.context("ler resposta do /tts")?;

        let audio = self
            .request(reqwest::Method::GET, &meta.url_path)
            .send()
            .await
            .context("baixar áudio gerado")?;
        ensure_ok(&audio)?;
        let bytes = audio.bytes().await?.to_vec();

        tracing::info!(
            chars = text.chars().count(),
            voice = %meta.voice,
            elapsed_ms = meta.elapsed_ms,
            cached = meta.cached,
            bytes = bytes.len(),
            "trecho sintetizado"
        );

        Ok((meta, bytes))
    }
}

fn ensure_ok(response: &reqwest::Response) -> Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("a speech-api recusou a credencial (401). Confira VOX_API_USER e VOX_API_PASSWORD.");
    }
    bail!("a speech-api respondeu {status}");
}

/// Quebra o texto em trechos que respeitam a pontuação.
///
/// O primeiro sai curto para o som começar rápido; os seguintes crescem, porque
/// a partir daí o que importa é manter a fila cheia, não a latência.
pub fn split_text(text: &str) -> Vec<String> {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return Vec::new();
    }

    let sentences = split_sentences(&cleaned);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for sentence in sentences {
        let budget = if chunks.is_empty() {
            FIRST_CHUNK_CHARS
        } else {
            CHUNK_TARGET_CHARS
        };

        if !current.is_empty() && current.chars().count() + sentence.chars().count() > budget {
            chunks.push(std::mem::take(&mut current));
        }

        // Uma frase sozinha maior que o orçamento vai inteira: cortar no meio
        // estraga a prosódia, e o ganho de latência não compensa.
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for character in text.chars() {
        current.push(character);
        if matches!(character, '.' | '!' | '?' | '…' | ';' | '\n') {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                sentences.push(trimmed.to_string());
            }
            current.clear();
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        sentences.push(trimmed.to_string());
    }
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primeiro_trecho_sai_curto() {
        let texto = "Primeira frase curta. Segunda frase um pouco mais longa que a primeira, \
                     para empurrar o acumulador. Terceira frase.";
        let chunks = split_text(texto);
        assert!(chunks.len() >= 2, "esperava fatiar, veio {chunks:?}");
        assert!(
            chunks[0].chars().count() <= FIRST_CHUNK_CHARS,
            "o primeiro trecho deveria caber em {FIRST_CHUNK_CHARS}: {:?}",
            chunks[0]
        );
    }

    #[test]
    fn texto_vazio_nao_gera_trecho() {
        assert!(split_text("   \n  ").is_empty());
    }

    #[test]
    fn frase_unica_gigante_nao_e_partida() {
        let longa = "palavra ".repeat(80);
        let chunks = split_text(&longa);
        assert_eq!(chunks.len(), 1, "frase sem pontuação deve sair inteira");
    }

}
