//! Configuração do app.
//!
//! Nada de credencial em código ou em arquivo versionado: a URL e o login vêm do
//! ambiente. As preferências de uso (voz, velocidade, atalhos) ficam num JSON no
//! diretório de config do usuário, que pode ir para o disco à vontade porque não
//! guarda segredo nenhum.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::api::Credentials;

/// Posição lógica escolhida para a janela flutuante.
///
/// Pode conter coordenadas negativas quando o monitor fica à esquerda do
/// principal, portanto não deve ser limitada a zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPosition {
    pub x: f64,
    pub y: f64,
}

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

    /// Mostra o texto sendo lido dentro da própria pílula flutuante.
    ///
    /// Diferente de `open_reader_on_read`: aqui não abre janela nenhuma — a
    /// pílula cresce e passa a exibir o texto com o trecho falado destacado.
    /// Quem só quer ouvir deixa desligado e a pílula continua do tamanho de
    /// sempre. O botão no próprio player liga e desliga, e o valor volta para
    /// cá para a próxima leitura começar como a última terminou.
    pub reading_captions: bool,

    /// Manda o texto ser corrigido antes de ser falado.
    ///
    /// A limpeza de marcação é sempre feita e custa milissegundos. Isto é outra
    /// coisa: acentuação, ortografia e pontuação corrigidas por um modelo, o que
    /// custa de 3,7 a 6,9 segundos por parágrafo — medido no gateway do lab,
    /// contra ~1 s da síntese do mesmo texto.
    ///
    /// Desligado por padrão porque essa espera vale para texto mal escrito e é
    /// pura perda para texto que já está certo, que é a maioria do que se lê.
    pub normalize_before_reading: bool,

    /// Transforma tabelas em explicação falada antes de ler.
    ///
    /// Ligado por padrão, ao contrário da correção: aqui o custo só existe
    /// quando o texto **tem** tabela, e o que ele substitui é inaudível de
    /// qualquer jeito — "app, versão, o que entrou, toolbox inventory, zero
    /// ponto dois ponto três" é uma fila de palavras sem a grade que as fazia
    /// significar alguma coisa.
    pub narrate_tables: bool,

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

    // --- reescrita do que foi ditado ---
    /// Passa a transcrição por um modelo antes de entregar.
    ///
    /// Desligado, o que sai é o que foi **dito** — com hesitação, repetição e
    /// frase recomeçada no meio. É fiel, e quase nunca é o que a pessoa queria
    /// ter escrito. Ligado, o texto vira o que ela escreveria se tivesse
    /// digitado, ao custo de alguns segundos antes da colagem.
    pub stt_rewrite_enabled: bool,

    /// Qual molde de reescrita usar. Os nomes vêm de `GET /text/presets`; um
    /// nome desconhecido cai no padrão do servidor em vez de falhar.
    pub stt_rewrite_preset: String,

    /// Quanto o modelo pode mexer, de 1 a 3.
    ///
    /// Não é um botão de qualidade: é a escolha entre fidelidade e fluência. Em
    /// 1 ele tira hesitação e mantém as frases como foram ditas; em 3 reorganiza
    /// o texto — e aí passa a inventar contexto de vez em quando, que é o preço
    /// de deixá-lo reescrever.
    pub stt_rewrite_intensity: u8,

    /// Modelo específico, quando o padrão do servidor não serve.
    pub stt_rewrite_model: Option<String>,

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
    /// Combinação de cada comando, pela chave do catálogo em `commands.rs`.
    ///
    /// Um mapa, e não um campo por comando: com oito comandos, um campo cada
    /// significaria oito lugares para lembrar de mexer a cada comando novo — e
    /// o oitavo é o que se esquece.
    pub shortcuts: BTreeMap<String, String>,

    /// Campos de antes do catálogo. Ficam para migrar a escolha de quem já
    /// tinha personalizado, e somem do arquivo assim que ela entra no mapa.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_dictate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_read: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_show_widget: Option<String>,

    /// Último local para onde a pessoa arrastou o widget.
    pub hud_position: Option<WindowPosition>,

    // --- geral ---
    pub sounds_enabled: bool,
    /// Volume dos avisos sonoros, de 0 a 1.
    ///
    /// Separado de `sounds_enabled` porque as duas perguntas são diferentes:
    /// "quero ser avisado?" e "quão alto?". Antes só existia a primeira, e quem
    /// achava o bipe alto demais não tinha saída senão desligar tudo — e aí
    /// perdia o aviso de erro junto.
    pub sounds_volume: f32,
    /// Pasta com os `.aif` do Raycast, para quem já os tem instalados.
    pub external_sounds_directory: Option<PathBuf>,

    /// O Vox responde aos comandos, ou está de folga.
    ///
    /// Desligado, ele solta os atalhos globais e a ponte recusa pedidos — mas
    /// continua na bandeja. É a diferença entre "não quero isso agora" e "não
    /// quero isto instalado", e só a primeira precisa de um interruptor.
    pub service_enabled: bool,

    /// Sobe junto com o Windows.
    ///
    /// A preferência é a intenção; o estado de verdade é a chave `Run` do
    /// registro, e a partida alinha as duas — alguém pode ter limpado a chave
    /// com um utilitário de inicialização por fora.
    pub start_with_windows: bool,

    /// Cor de destaque da interface, em hexadecimal.
    ///
    /// Vale para o painel e para o realce da palavra na legenda guiada — as duas
    /// coisas que a pessoa olha, e que ficariam estranhas se discordassem.
    pub theme_accent: String,
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
            submit_mode: SubmitMode::Disabled,
            submit_keyword: "manda ver".into(),
            submit_key: SubmitKey::Enter,
            live_transcription: true,
            show_live_transcription: false,
            open_reader_on_read: false,
            reading_captions: false,
            normalize_before_reading: false,
            narrate_tables: true,

            bridge_enabled: true,
            bridge_port: 8765,
            bridge_token: gerar_token(),
            vocabulary: Vec::new(),
            custom_instructions: String::new(),

            stt_rewrite_enabled: false,
            stt_rewrite_preset: "fala-limpa".into(),
            stt_rewrite_intensity: 2,
            stt_rewrite_model: None,

            // Cadu, do Piper: escolhido por escuta e confirmado pela medicao —
            // gera em 0,245x o tempo de fala contra 2,2x do Kokoro. Abaixo de
            // 1,0x a geracao acompanha a reproducao, e a leitura nunca engasga.
            voice: "piper:pt_BR-cadu-medium".into(),
            speed: 1.0,
            prebuffer_ratio: 0.10,

            shortcuts: BTreeMap::new(),
            shortcut_dictate: None,
            shortcut_read: None,
            shortcut_show_widget: None,

            hud_position: None,

            sounds_enabled: true,
            sounds_volume: 1.0,
            external_sounds_directory: default_raycast_sounds_directory(),
            service_enabled: true,
            start_with_windows: false,
            theme_accent: "#966aff".into(),
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
        // NaN vindo de um JSON editado à mão passaria pelo clamp e chegaria ao
        // `rodio` como volume inválido, silenciando tudo sem erro nenhum.
        if !self.sounds_volume.is_finite() {
            self.sounds_volume = 1.0;
        }
        self.sounds_volume = self.sounds_volume.clamp(0.0, 1.0);
        self.stt_rewrite_intensity = self.stt_rewrite_intensity.clamp(1, 3);
        if self.stt_rewrite_preset.trim().is_empty() {
            self.stt_rewrite_preset = "fala-limpa".into();
        }

        // Cada comando precisa de uma combinação, e a do arquivo tem prioridade
        // sobre a padrão. O campo antigo entra aqui uma única vez: depois disto
        // ele some do JSON, porque só é serializado quando existe.
        for comando in crate::commands::Command::ALL {
            let legado = match comando {
                crate::commands::Command::Dictate => self.shortcut_dictate.take(),
                crate::commands::Command::ReadSelection => self.shortcut_read.take(),
                crate::commands::Command::ShowWidget => self.shortcut_show_widget.take(),
                _ => None,
            };

            let entrada = self.shortcuts.entry(comando.id().to_string()).or_insert_with(|| {
                legado.unwrap_or_else(|| comando.default_binding().to_string())
            });

            // Vazio significa "sem atalho" de propósito — quem não quer um
            // comando ocupando uma combinação global apaga o campo no painel.
            *entrada = entrada.trim().to_string();
        }

        // Cor inválida vira a padrão em vez de virar CSS quebrado: o front
        // escreve isto direto numa variável, e `background: lixo` não pinta nada.
        let cor = self.theme_accent.trim();
        let valida = cor.len() == 7
            && cor.starts_with('#')
            && cor[1..].chars().all(|digito| digito.is_ascii_hexdigit());
        if !valida {
            self.theme_accent = "#966aff".into();
        }
        if self.hud_position.is_some_and(|position| {
            !position.x.is_finite() || !position.y.is_finite()
        }) {
            self.hud_position = None;
        }

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

    #[test]
    fn descarta_posicao_de_widget_invalida() {
        let mut settings = Settings {
            hud_position: Some(WindowPosition { x: f64::NAN, y: 24.0 }),
            ..Default::default()
        };
        settings.sanitize();
        assert_eq!(settings.hud_position, None);
    }
}

/// Onde o log vai parar. Fica ao lado do `settings.json` de propósito: o item da
/// bandeja que abre as preferências abre a mesma pasta, e assim o arquivo que
/// explica um erro está a um clique de quem acabou de ver o erro.
pub fn log_path() -> PathBuf {
    settings_path().with_file_name("vox.log")
}

/// Monta o `prompt` da transcrição a partir do vocabulário e das instruções.
///
/// O campo existe no contrato da OpenAI para dar contexto ao reconhecedor: uma
/// lista de nomes próprios e termos técnicos que ele não teria como adivinhar.
/// Sem isso "Traefik" volta como "trafic" toda vez, e nenhuma correção posterior
/// desfaz isso sem adivinhar junto.
///
/// A ordem é deliberada: os termos primeiro. O campo tem teto de tamanho do lado
/// do servidor, e o que for cortado deve ser a instrução em prosa — que ajuda
/// menos que a lista de palavras.
pub fn transcription_prompt(settings: &Settings) -> String {
    let mut partes = Vec::new();

    if !settings.vocabulary.is_empty() {
        partes.push(format!("Termos: {}.", settings.vocabulary.join(", ")));
    }

    let instrucoes = settings.custom_instructions.trim();
    if !instrucoes.is_empty() {
        partes.push(instrucoes.to_string());
    }

    partes.join(" ")
}
