//! Backup local do áudio de cada ditado.
//!
//! ## Por que existe
//!
//! O caminho normal é: gravar, mandar o WAV pro servidor, receber o texto,
//! colar. Cada passo desses pode falhar sem culpa de quem ditou — a rede cai,
//! o back-end não responde, o reconhecedor local decide (errado) que não
//! houve fala, a transcrição volta vazia ou errada. Antes desta peça, o WAV
//! só existia na memória do processo de ditado e morria com ele: uma falha
//! ali custava a gravação inteira, e não tinha volta — nem repetir, porque
//! quem ditou já seguiu em frente ou nem lembra mais exatamente o que falou.
//!
//! Agora cada gravação é escrita em disco **antes** de qualquer coisa que
//! possa falhar, incluindo o próprio julgamento local de "não teve fala".
//! Falhar depois disso vira, no pior caso, uma gravação sentada na pasta
//! esperando um retry manual — não uma perda.
//!
//! ## Por que uma pasta que se limpa sozinha, e não guardar tudo pra sempre
//!
//! Isto é uma rede de segurança contra falha, não um histórico de ditados. A
//! pessoa que ditou sabe se aquilo importava — se sabia, ela salva (e o
//! arquivo sai da faixa de limpeza); se não fez nada, entende-se que resolveu
//! do jeito normal e o áudio não faz falta depois de um dia. Sem o expurgo,
//! a pasta cresceria pra sempre com gravações que ninguém nunca mais vai
//! abrir.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Quanto tempo uma gravação não salva sobrevive antes do expurgo.
const EXPIRACAO: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Gravada, mandada pro servidor, ainda sem resposta — só existe se o
    /// processo morrer no meio do caminho.
    Pendente,
    /// O reconhecedor local decidiu que não teve fala; nunca chegou a ir pro
    /// servidor. É exatamente o caso que mais vale poder reabrir na mão.
    SemFala,
    /// Foi pro servidor e voltou com texto.
    Transcrito,
    /// Foi pro servidor e voltou vazio.
    Vazio,
    /// A chamada pro servidor falhou (rede, back-end fora do ar, etc).
    Falhou,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingMeta {
    pub id: String,
    /// Milissegundos desde a época — o front formata a hora, não o backend.
    pub created_at_ms: u64,
    pub duration_seconds: f32,
    pub device_name: String,
    pub status: Status,
    pub text: Option<String>,
    pub error: Option<String>,
    /// `true` tira a gravação da faixa de expurgo automático.
    pub saved: bool,
}

fn dir() -> PathBuf {
    crate::config::settings_path()
        .parent()
        .map(|pasta| pasta.join("recordings"))
        .unwrap_or_else(|| PathBuf::from("recordings"))
}

fn caminho_wav(id: &str, base: &Path) -> PathBuf {
    base.join(format!("{id}.wav"))
}

fn caminho_meta(id: &str, base: &Path) -> PathBuf {
    base.join(format!("{id}.json"))
}

fn agora_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Só precisa ser único dentro do processo: dois ditados nunca terminam no
/// mesmo milissegundo *e* colidem no contador ao mesmo tempo.
fn novo_id() -> String {
    static CONTADOR: AtomicU32 = AtomicU32::new(0);
    let sequencia = CONTADOR.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", agora_ms(), sequencia)
}

fn ler_meta(caminho: &Path) -> Result<RecordingMeta> {
    let texto = std::fs::read_to_string(caminho).context("ler os metadados da gravação")?;
    serde_json::from_str(&texto).context("metadados da gravação ilegíveis")
}

fn gravar_meta(caminho: &Path, meta: &RecordingMeta) -> Result<()> {
    let texto = serde_json::to_string_pretty(meta).context("serializar os metadados da gravação")?;
    std::fs::write(caminho, texto).context("gravar os metadados da gravação")
}

/// Grava um ditado recém-capturado em disco, antes de qualquer coisa que
/// possa falhar. Devolve o ID pra quem chamou poder atualizar o status depois
/// (`set_result`) — falhar em guardar não pode travar o ditado, então quem
/// chama trata `None` como "sem backup desta vez" e segue o fluxo normal.
pub fn save(recording: &crate::audio::Recording, wav: &[u8]) -> Option<String> {
    let base = dir();
    if let Err(err) = std::fs::create_dir_all(&base) {
        tracing::warn!(?err, "não deu para preparar a pasta de backup de áudio");
        return None;
    }

    let id = novo_id();
    let meta = RecordingMeta {
        id: id.clone(),
        created_at_ms: agora_ms(),
        duration_seconds: recording.duration_seconds(),
        device_name: recording.device_name.clone(),
        status: Status::Pendente,
        text: None,
        error: None,
        saved: false,
    };

    if let Err(err) = std::fs::write(caminho_wav(&id, &base), wav) {
        tracing::warn!(?err, "não deu para guardar o áudio de backup");
        return None;
    }
    if let Err(err) = gravar_meta(&caminho_meta(&id, &base), &meta) {
        tracing::warn!(?err, "não deu para guardar os metadados do backup de áudio");
        return None;
    }

    Some(id)
}

/// Atualiza o resultado depois que o ditado terminou de tentar — sucesso,
/// vazio, sem fala ou falha. `id` vem de `save`; se for `None` (o backup não
/// pôde ser criado), não há nada pra atualizar.
pub fn set_result(id: Option<&str>, status: Status, text: Option<String>, error: Option<String>) {
    let Some(id) = id else { return };
    let base = dir();
    let caminho = caminho_meta(id, &base);

    let mut meta = match ler_meta(&caminho) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::warn!(?err, id, "não deu para atualizar o resultado do backup de áudio");
            return;
        }
    };
    meta.status = status;
    meta.text = text;
    meta.error = error;

    if let Err(err) = gravar_meta(&caminho, &meta) {
        tracing::warn!(?err, id, "não deu para gravar o resultado do backup de áudio");
    }
}

/// Todas as gravações guardadas agora, mais recente primeiro. Cada chamada
/// também expurga o que já passou das 24h e não foi salvo — não precisa de
/// uma tarefa em segundo plano só pra isso.
pub fn list() -> Result<Vec<RecordingMeta>> {
    let base = dir();
    if !base.exists() {
        return Ok(Vec::new());
    }

    let agora = agora_ms();
    let mut gravacoes = Vec::new();

    for entrada in std::fs::read_dir(&base).context("listar a pasta de backup de áudio")? {
        let Ok(entrada) = entrada else { continue };
        let caminho = entrada.path();
        if caminho.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = ler_meta(&caminho) else { continue };

        let idade = Duration::from_millis(agora.saturating_sub(meta.created_at_ms));
        if !meta.saved && idade > EXPIRACAO {
            let _ = std::fs::remove_file(&caminho);
            let _ = std::fs::remove_file(caminho_wav(&meta.id, &base));
            continue;
        }

        gravacoes.push(meta);
    }

    gravacoes.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(gravacoes)
}

/// Reenvia o áudio de uma gravação pro servidor, com as preferências atuais
/// (modelo, vocabulário) — não necessariamente as de quando ela foi feita.
/// Atualiza o backup com o novo resultado antes de devolver.
pub async fn retry(
    id: &str,
    api: &crate::api::SpeechApi,
    model: &str,
    prompt: &str,
) -> Result<RecordingMeta> {
    let base = dir();
    let wav = std::fs::read(caminho_wav(id, &base)).context("ler o áudio guardado")?;

    let resultado = api.transcribe(wav, model, prompt).await;

    let (status, text, error) = match resultado {
        Ok(resposta) => {
            let texto = resposta.text.trim().to_string();
            if texto.is_empty() {
                (Status::Vazio, None, None)
            } else {
                (Status::Transcrito, Some(texto), None)
            }
        }
        Err(err) => (Status::Falhou, None, Some(err.to_string())),
    };

    set_result(Some(id), status, text, error);
    ler_meta(&caminho_meta(id, &base))
}

/// Tira a gravação da faixa de expurgo automático.
pub fn mark_saved(id: &str) -> Result<()> {
    let base = dir();
    let caminho = caminho_meta(id, &base);
    let mut meta = ler_meta(&caminho)?;
    meta.saved = true;
    gravar_meta(&caminho, &meta)
}

/// Apaga a gravação na hora, sem esperar o expurgo — mesmo uma marcada como
/// salva: pedido explícito vale mais que a proteção.
pub fn delete(id: &str) -> Result<()> {
    let base = dir();
    let _ = std::fs::remove_file(caminho_wav(id, &base));
    std::fs::remove_file(caminho_meta(id, &base)).context("apagar os metadados da gravação")
}

/// Abre a pasta de backup no Explorer, pra quem quiser copiar um WAV pra
/// outro lugar na mão em vez de só marcar "salvo".
pub fn open_folder() -> Result<()> {
    let base = dir();
    std::fs::create_dir_all(&base).context("preparar a pasta de backup de áudio")?;
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &base.to_string_lossy()])
        .spawn()
        .context("abrir a pasta de backup no Explorer")?;
    Ok(())
}
