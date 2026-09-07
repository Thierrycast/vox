//! Configuração do app.
//!
//! Nada de credencial em código ou em arquivo versionado: a URL e o login vêm do
//! ambiente. As preferências de uso (voz, velocidade, atalhos) ficam num JSON no
//! diretório de config do usuário, que pode ir para o disco à vontade porque não
//! guarda segredo nenhum.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::api::Credentials;

const ENV_BASE_URL: &str = "VOX_API_URL";
const ENV_USER: &str = "VOX_API_USER";
const ENV_PASSWORD: &str = "VOX_API_PASSWORD";

/// Caminho da API quando nada é informado.
///
/// Vai direto na porta publicada no tailnet, que não passa pelo Traefik e por
/// isso não pede credencial. O caminho pela LAN
/// (`https://speech-api.lab.home`) existe e é autenticado pelo `panel-auth`,
/// mas exige entrada no `hosts` e só funciona em casa.
const DEFAULT_BASE_URL: &str = "http://100.122.39.56:8010";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // --- ditado ---
    pub input_device: Option<String>,
    pub transcription_model: String,
    pub output_action: OutputAction,
    pub context_aware_paste: bool,
    pub submit_mode: SubmitMode,
    pub submit_keyword: String,
    pub submit_key: SubmitKey,
    /// Mostra o texto no HUD enquanto a pessoa fala.
    ///
    /// Usa o WebSocket `/stt/stream`, que roda no Vosk local. Isso **não**
    /// substitui a transcrição final: o Vosk é rápido mas devolve minúsculas,
    /// sem pontuação e erra números. O texto que é colado continua vindo do
    /// modelo escolhido em `transcription_model`, pelo caminho em lote.
    ///
    /// O ganho é de percepção: em vez de encarar a animação de "processando"
    /// sem nada, o usuário vê as palavras aparecendo.
    pub live_transcription: bool,

    /// Mostra o texto reconhecido numa janelinha à parte, no canto superior
    /// direito.
    ///
    /// Separado de `live_transcription` de propósito: o texto ao vivo continua
    /// sendo capturado (ele é a rede de segurança se a transcrição final
    /// falhar), mas por padrão **não aparece**. Texto correndo embaixo da onda
    /// disputa a atenção justamente enquanto a pessoa está formulando a frase.
    /// Quem quiser conferir o que está sendo entendido liga isto e ganha um
    /// popup fora do caminho.
    pub show_live_transcription: bool,

    /// Abre a janela de leitura guiada junto com a fala.
    ///
    /// Desligado por padrão. Ela mostra o texto de novo, numa segunda janela, e
    /// em cima de um navegador ou editor isso é o mesmo conteúdo duplicado
    /// tapando o original — a pessoa já está com o texto na tela. Quando o texto
    /// veio de onde não dá para acompanhar (um terminal que rolou, um PDF), a
    /// janela é útil: por isso ela continua a um clique, no relógio do player.
    pub open_reader_on_read: bool,

    // --- ponte da extensão de navegador ---
    /// Sobe o servidor local que a extensão usa. Desligue para fechar a porta.
    pub bridge_enabled: bool,
    /// Porta em `127.0.0.1`. A extensão precisa apontar para a mesma.
    pub bridge_port: u16,
    /// Segredo compartilhado com a extensão.
    ///
    /// Gerado na primeira execução. Existe porque `127.0.0.1` não é uma
    /// fronteira de confiança: qualquer programa da máquina alcança a porta, e
    /// o cabeçalho `Origin` sozinho só barra páginas web, não outras extensões.
    pub bridge_token: String,
    pub vocabulary: Vec<String>,
    pub custom_instructions: String,

    // --- leitura ---
    pub voice: String,
    pub speed: f32,
    /// Margem de segurança sobre a dianteira calculada, de 0 a 0,5.
    ///
    /// A dianteira em si **não** é configurada: o player mede quantas vezes o
    /// tempo de fala custa para gerar e deriva o quanto precisa acumular. Isso
    /// existe porque a razão varia muito com a carga do servidor — medimos
    /// 1,06× com o argos ocioso e 2,7× com ele em load 15. Este valor só
    /// acrescenta folga sobre o cálculo.
    pub prebuffer_ratio: f32,

    // --- atalhos ---
    /// Combinações globais, no formato do Tauri (`Ctrl+Shift+D`).
    ///
    /// Configuráveis porque atalho global é recurso disputado e o que está
    /// livre varia por máquina: `Ctrl+Shift+S` parecia seguro até descobrirmos
    /// que abre o DevTools no Chrome.
    pub shortcut_dictate: String,
    pub shortcut_read: String,

    // --- geral ---
    pub sounds_enabled: bool,
    pub mute_while_recording: bool,
    /// Pasta com os `.aif` do Raycast, para quem já os tem instalados.
    pub external_sounds_directory: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            // O Vosk devolve tudo em minúscula e sem pontuação; o Whisper sai
            // pronto. O padrão é o que dá menos trabalho ao usuário — quem
            // quiser privacidade total troca nas preferências.
            transcription_model: "groq/whisper-large-v3-turbo".into(),
            output_action: OutputAction::Paste,
            context_aware_paste: true,
            submit_mode: SubmitMode::Disabled,
            submit_keyword: "manda ver".into(),
            submit_key: SubmitKey::Enter,
            live_transcription: true,
            show_live_transcription: false,
            open_reader_on_read: false,

            bridge_enabled: true,
            bridge_port: 8765,
            bridge_token: gerar_token(),
            vocabulary: Vec::new(),
            custom_instructions: String::new(),

            // Cadu, do Piper: escolhido por escuta e confirmado pela medicao —
            // gera em 0,245x o tempo de fala contra 2,2x do Kokoro. Abaixo de
            // 1,0x a geracao acompanha a reproducao, e a leitura nunca engasga.
            voice: "piper:pt_BR-cadu-medium".into(),
            speed: 1.0,
            prebuffer_ratio: 0.10,

            shortcut_dictate: "Ctrl+Shift+D".into(),
            // `Ctrl+Shift+S` era o padrão e foi trocado: abre o DevTools no
            // Chrome, e o navegador ganha a disputa. `Ctrl+Alt+L` de "Ler" é
            // raro em atalho de aplicativo e não colide com nada do Windows.
            shortcut_read: "Ctrl+Alt+L".into(),

            sounds_enabled: true,
            mute_while_recording: false,
            external_sounds_directory: default_raycast_sounds_directory(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputAction {
    Paste,
    Clipboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitMode {
    Disabled,
    Keyword,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitKey {
    Enter,
    ShiftEnter,
}

/// Limites herdados da API e do desenho do Raycast.
pub const MAX_VOCABULARY_ITEMS: usize = 50;
pub const MAX_VOCABULARY_ITEM_LENGTH: usize = 50;
pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

impl Settings {
    /// Corrige valores fora de faixa em vez de recusar o arquivo inteiro.
    ///
    /// Um JSON editado à mão com `speed: 9` deve virar `speed: 2`, não impedir o
    /// app de abrir.
    pub fn sanitize(&mut self) {
        self.speed = self.speed.clamp(MIN_SPEED, MAX_SPEED);
        self.prebuffer_ratio = self.prebuffer_ratio.clamp(0.0, 0.5);

        self.vocabulary.retain(|term| {
            let trimmed = term.trim();
            !trimmed.is_empty() && trimmed.chars().count() <= MAX_VOCABULARY_ITEM_LENGTH
        });

        // Únicos, ignorando caixa, preservando a ordem de entrada.
        let mut seen = Vec::<String>::new();
        self.vocabulary.retain(|term| {
            let key = term.to_lowercase();
            if seen.contains(&key) {
                false
            } else {
                seen.push(key);
                true
            }
        });
        self.vocabulary.truncate(MAX_VOCABULARY_ITEMS);

        if self.submit_keyword.trim().is_empty() {
            self.submit_keyword = "manda ver".into();
        }
    }

    pub fn load() -> Self {
        let path = settings_path();
        let mut settings = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
                tracing::warn!(?path, ?err, "preferências ilegíveis; usando os padrões");
                Settings::default()
            }),
            Err(_) => {
                // Grava os padrões na primeira execução. Enquanto não há janela
                // de preferências, o arquivo é a única forma de descobrir e
                // mexer no que dá para configurar — e um arquivo que não existe
                // não se deixa descobrir.
                let padroes = Settings::default();
                if let Err(err) = padroes.save() {
                    tracing::warn!(?path, ?err, "não deu para gravar as preferências padrão");
                } else {
                    tracing::info!(?path, "preferências padrão gravadas");
                }
                padroes
            }
        };
        settings.sanitize();

        // Regrava sempre. Um arquivo escrito por uma versão anterior não tem os
        // campos novos, e um campo que não está no arquivo é um campo que o
        // usuário não tem como descobrir nem editar — foi assim que o token da
        // ponte ficou invisível justamente para quem precisava copiá-lo. Como o
        // que se grava é o que acabou de ser lido, nada do que ele configurou se
        // perde: só os ausentes entram, com o padrão.
        if let Err(err) = settings.save() {
            tracing::warn!(?err, "não deu para normalizar o arquivo de preferências");
        }

        settings
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("criar diretório de config")?;
        }
        let raw = serde_json::to_string_pretty(self).context("serializar preferências")?;
        std::fs::write(&path, raw).with_context(|| format!("gravar {}", path.display()))?;
        Ok(())
    }
}

/// Token de 32 caracteres hexadecimais para a ponte local.
///
/// Escrito à mão em vez de trazer o `rand`: o segredo protege uma porta de
/// loopback contra outros programas da própria máquina, não contra um
/// adversário com poder de computação. O relógio em nanossegundos misturado
/// com o identificador do processo e com um endereço de heap dá entropia de
/// sobra para isso, e uma dependência a menos num binário que já tem muitas.
fn gerar_token() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut saida = String::with_capacity(32);
    let semente = Box::new(0u8);
    let endereco = Box::as_ref(&semente) as *const u8 as usize;

    for rodada in 0..2u64 {
        let mut hasher = DefaultHasher::new();
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|passado| passado.as_nanos())
            .unwrap_or_default()
            .hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        endereco.hash(&mut hasher);
        rodada.hash(&mut hasher);
        saida.push_str(&format!("{:016x}", hasher.finish()));
    }
    saida
}

pub fn settings_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "vox")
        .map(|dirs| dirs.config_dir().join("settings.json"))
        .unwrap_or_else(|| PathBuf::from("vox-settings.json"))
}

/// Endereço da API: do ambiente, ou o padrão da LAN.
pub fn base_url() -> String {
    std::env::var(ENV_BASE_URL)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// Credencial do basicAuth. Ausente é válido — a API pode estar sem `panel-auth`
/// em algum caminho (o loopback do argos, por exemplo).
pub fn credentials() -> Option<Credentials> {
    let username = std::env::var(ENV_USER).ok()?;
    let password = std::env::var(ENV_PASSWORD).ok()?;
    if username.trim().is_empty() {
        return None;
    }
    Some(Credentials { username, password })
}

/// Onde o Raycast guarda os sons de ditado, quando instalado.
fn default_raycast_sounds_directory() -> Option<PathBuf> {
    let base = Path::new(r"C:\Program Files\WindowsApps");
    let entries = std::fs::read_dir(base).ok()?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("Raycast.Raycast_") {
            continue;
        }
        let candidate = entry.path().join(r"Raycast\Resources\Audio\Dictation");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_limita_velocidade() {
        let mut settings = Settings { speed: 9.0, ..Default::default() };
        settings.sanitize();
        assert_eq!(settings.speed, MAX_SPEED);
    }

    #[test]
    fn sanitize_remove_vocabulario_duplicado_e_longo() {
        let mut settings = Settings {
            vocabulary: vec![
                "Tauri".into(),
                "tauri".into(),          // duplicata, outra caixa
                "  ".into(),             // vazio
                "x".repeat(60),          // longo demais
                "Raycast".into(),
            ],
            ..Default::default()
        };
        settings.sanitize();
        assert_eq!(settings.vocabulary, vec!["Tauri".to_string(), "Raycast".to_string()]);
    }

    #[test]
    fn sanitize_corta_no_teto_de_itens() {
        let mut settings = Settings {
            vocabulary: (0..80).map(|index| format!("termo{index}")).collect(),
            ..Default::default()
        };
        settings.sanitize();
        assert_eq!(settings.vocabulary.len(), MAX_VOCABULARY_ITEMS);
    }

    #[test]
    fn palavra_chave_vazia_volta_ao_padrao() {
        let mut settings = Settings { submit_keyword: "   ".into(), ..Default::default() };
        settings.sanitize();
        assert_eq!(settings.submit_keyword, "manda ver");
    }
}
