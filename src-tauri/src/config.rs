//! Configuração do app.
//!
//! Nada de credencial em código ou em arquivo versionado — a URL e o login do
//! servidor podem vir do ambiente (`VOX_API_URL` e
//! companhia), mas quem instala normalmente escolhe pelo painel, na seção
//! "Servidor", que grava no JSON de preferências do usuário. É o mesmo
//! arquivo que já guarda o `bridge_token` da extensão em texto puro: outra
//! credencial de uso local, sem sair da máquina, não muda o modelo de ameaça.
//! O ambiente continua funcionando como veio antes — é a saída pra quem
//! prefere configurar por fora do app (scripts, deploy automatizado) — e o
//! painel vence quando os dois estão preenchidos.

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

/// Onde a janela flutuante fica, por papel.
///
/// `Livre` é o comportamento de sempre: arrastar move e o lugar fica lembrado.
/// Cada valor restante ancora a janela num ponto do monitor — para quem quer o
/// widget sempre no mesmo canto, sem depender de onde ele foi parar da última
/// vez. Ancorado, o arraste não move.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HudPlacement {
    #[default]
    Livre,
    TopoEsquerda,
    TopoCentro,
    TopoDireita,
    MeioEsquerda,
    MeioDireita,
    BaixoEsquerda,
    BaixoCentro,
    BaixoDireita,
}

const ENV_BASE_URL: &str = "VOX_API_URL";
const ENV_USER: &str = "VOX_API_USER";
const ENV_PASSWORD: &str = "VOX_API_PASSWORD";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // --- servidor ---
    /// Endereço da speech-api. Vazio cai no ambiente (`VOX_API_URL`); sem os
    /// dois, o Vox não tem servidor e o assistente de primeiro uso pede um.
    ///
    /// O Vox depende de um servidor compatível pra falar e ouvir; ele não
    /// embute nenhum modelo de fala. Ver "Servidor" nas preferências e o
    /// README pro que esse servidor precisa expor.
    pub api_base_url: Option<String>,
    /// Usuário do basicAuth, se o caminho até o servidor exigir. A maioria
    /// não precisa — o caminho comum é sobre uma rede privada (tailnet) que
    /// não passa por nenhum proxy autenticado.
    pub api_user: Option<String>,
    pub api_password: Option<String>,

    /// Já passou pelo primeiro-uso. Enquanto `false`, o Vox abre a janela de
    /// boas-vindas em vez do comportamento normal — sem isso, alguém
    /// instalando pela primeira vez encara uma bandeja muda sem nenhuma pista
    /// de que precisa apontar pra um servidor antes de ditar qualquer coisa.
    pub onboarding_completed: bool,

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

    /// Passa o texto por uma limpeza de modelo antes de ler (preset `leitura`).
    ///
    /// Diferente da correção abaixo: esta não muda o que está escrito — só a
    /// forma de falar. Unidade abreviada vira "2,0 segundos", símbolo vira
    /// "por cento", markdown e tabela viram frases. Custa de 1 a 4 s no
    /// gateway, com prazo curto no cliente; estourado ou travado, lê o texto
    /// original em vez de prender o primeiro som.
    pub reading_rewrite_enabled: bool,

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

    /// Escuta a ponte também no IP do Tailscale.
    ///
    /// O loopback não alcança o WSL em modo NAT nem as máquinas remotas; o IP
    /// da tailnet alcança os dois, cifrado pelo próprio Tailscale. Desligado
    /// por padrão: abrir a porta para a tailnet é uma decisão, não um
    /// efeito colateral de atualizar o Vox.
    pub bridge_tailscale: bool,

    // --- narração de agentes de CLI ---
    /// Aceita pedidos de narração pela ponte (rota `/narrate`).
    ///
    /// Um mod do agente manda a resposta final do turno; o Vox adapta o texto
    /// para fala e usa o mesmo player da leitura. Desligado, a rota recusa e
    /// o resto do app segue idêntico.
    pub narration_enabled: bool,

    /// Fala sozinho quando o item chega ao topo da fila.
    ///
    /// Desligado, o item é adaptado e fica pronto, esperando o comando de
    /// tocar — quem quer ouvir no próprio ritmo, e não no ritmo do agente.
    /// Um pedido pode vir com modo próprio (`speak`/`hold`) e sobrescrever
    /// isto por item.
    pub narration_auto_speak: bool,

    /// Modelo da adaptação de narração (preset `narracao` da speech-api).
    ///
    /// Vazio usa o padrão do servidor, que já é o rápido; preencher é para
    /// trocar por um mais caprichado sem depender de deploy do servidor.
    pub narration_model: Option<String>,

    /// Teto de itens esperando na fila de narração.
    ///
    /// Um agente em loop pode terminar turno a cada minuto; sem teto, uma
    /// noite deixaria centenas de respostas velhas na fila, e a primeira
    /// coisa ouvida de manhã seria a mais antiga delas.
    pub narration_queue_limit: usize,

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

    /// Onde a pessoa deixou a barra do ditado: o canto superior esquerdo.
    ///
    /// Uma posição por papel, e não uma para o widget inteiro. A barra do ditado
    /// e a coluna da leitura são a mesma janela com formas e usos diferentes, e
    /// quem arrasta uma para o canto da tela não quer a outra indo junto — era
    /// exatamente o que acontecia com uma posição só: a gravação nascia onde a
    /// leitura tinha ficado.
    pub hud_position_dictation: Option<WindowPosition>,

    /// Onde a pessoa deixou a coluna da leitura: o canto superior **direito**.
    ///
    /// Direito, e não esquerdo, porque a legenda cresce para a esquerda. A borda
    /// direita é o que fica parado entre a pílula estreita e a aberta, e é ela
    /// que precisa ser lembrada para os controles aparecerem onde a pessoa os
    /// deixou, com ou sem legenda.
    pub hud_position_reading: Option<WindowPosition>,

    /// Se a barra do ditado é ancorada num ponto da tela ou segue o arraste.
    ///
    /// Ancorada, a posição lembrada deixa de valer e o arraste não move a
    /// janela — é uma escolha de quem quer o widget sempre no mesmo lugar, e
    /// não de quem o afastou do caminho uma vez.
    pub hud_placement_dictation: HudPlacement,

    /// O mesmo para a coluna da leitura.
    pub hud_placement_reading: HudPlacement,

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
            api_base_url: None,
            api_user: None,
            api_password: None,
            onboarding_completed: false,

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
            reading_rewrite_enabled: true,

            bridge_enabled: true,
            bridge_port: 8765,
            bridge_token: gerar_token(),
            vocabulary: Vec::new(),
            custom_instructions: String::new(),
            bridge_tailscale: false,

            narration_enabled: true,
            narration_auto_speak: true,
            narration_model: None,
            narration_queue_limit: 10,

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

            hud_position_dictation: None,
            hud_position_reading: None,
            hud_placement_dictation: HudPlacement::Livre,
            hud_placement_reading: HudPlacement::Livre,

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
        // Teto de fila: zero travaria a narração com a aparência de ligada, e
        // um número gigante seria o mesmo que não ter teto.
        self.narration_queue_limit = self.narration_queue_limit.clamp(1, 50);

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
        // Coordenada que não é número deixaria o widget num lugar que nenhum
        // monitor mostra. Negativa é válida — monitor à esquerda do principal.
        let invalida = |posicao: &Option<WindowPosition>| {
            posicao.is_some_and(|valor| !valor.x.is_finite() || !valor.y.is_finite())
        };
        if invalida(&self.hud_position_dictation) {
            self.hud_position_dictation = None;
        }
        if invalida(&self.hud_position_reading) {
            self.hud_position_reading = None;
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
            Ok(raw) => {
                // O onboarding é novo; um arquivo de antes dele nunca teve este
                // campo, e cairia no padrão (`false`) do jeito normal de tratar
                // campo ausente — igual e correto pra tudo mais nesta struct,
                // errado só aqui: forçaria o assistente de primeiro uso em
                // quem já configurou o Vox há dias, só porque a versão é mais
                // nova. `contains_key` decide isso **antes** do valor sumir no
                // meio da desserialização.
                let tinha_o_campo = serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|valor| {
                        valor.as_object().map(|mapa| mapa.contains_key("onboarding_completed"))
                    })
                    .unwrap_or(true);

                let mut settings: Settings = serde_json::from_str(&raw).unwrap_or_else(|err| {
                    tracing::warn!(?path, ?err, "preferências ilegíveis; usando os padrões");
                    Settings::default()
                });
                if !tinha_o_campo {
                    settings.onboarding_completed = true;
                }
                settings
            }
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

pub const CAMPOS_AJUSTAVEIS_PELA_PONTE: &[&str] = &[
    "service_enabled",
    "start_with_windows",
    "sounds_enabled",
    "sounds_volume",
    "theme_accent",
    "voice",
    "speed",
    "prebuffer_ratio",
    "normalize_before_reading",
    "narrate_tables",
    "reading_rewrite_enabled",
    "open_reader_on_read",
    "narration_enabled",
    "narration_auto_speak",
    "narration_model",
    "narration_queue_limit",
    "input_device",
    "transcription_model",
    "output_action",
    "submit_mode",
    "submit_keyword",
    "submit_key",
    "live_transcription",
    "show_live_transcription",
    "stt_rewrite_enabled",
    "stt_rewrite_preset",
    "stt_rewrite_intensity",
    "stt_rewrite_model",
    "vocabulary",
    "custom_instructions",
    "shortcuts",
    "hud_placement_dictation",
    "hud_placement_reading",
];

const CAMPOS_SO_LEITURA_PELA_PONTE: &[&str] = &[
    "bridge_enabled",
    "bridge_port",
    "bridge_tailscale",
    "reading_captions",
];

pub fn visao_da_ponte(settings: &Settings) -> serde_json::Value {
    let completo = serde_json::to_value(settings).unwrap_or_default();
    let mut visao = serde_json::Map::new();
    for campo in CAMPOS_AJUSTAVEIS_PELA_PONTE.iter().chain(CAMPOS_SO_LEITURA_PELA_PONTE) {
        if let Some(valor) = completo.get(*campo) {
            visao.insert((*campo).to_string(), valor.clone());
        }
    }
    visao.insert("api_base_url".into(), serde_json::Value::String(base_url(settings)));
    visao.insert(
        "api_auth".into(),
        serde_json::Value::Bool(credentials(settings).is_some()),
    );
    visao.insert(
        "settings_path".into(),
        serde_json::Value::String(settings_path().to_string_lossy().to_string()),
    );
    serde_json::Value::Object(visao)
}

pub fn ajustar_pela_ponte(
    settings: &Settings,
    mudancas: &serde_json::Value,
) -> std::result::Result<Settings, String> {
    let mudancas = mudancas
        .as_object()
        .ok_or_else(|| "o corpo precisa ser um objeto com os campos a mudar".to_string())?;
    if mudancas.is_empty() {
        return Err("nenhum campo para mudar".into());
    }

    let mut completo = serde_json::to_value(settings).map_err(|err| err.to_string())?;
    let mapa = completo
        .as_object_mut()
        .ok_or_else(|| "preferências em formato inesperado".to_string())?;

    for (campo, valor) in mudancas {
        if !CAMPOS_AJUSTAVEIS_PELA_PONTE.contains(&campo.as_str()) {
            return Err(format!("{campo} não muda pela ponte; use as preferências do Vox"));
        }
        if campo == "shortcuts" {
            let novos = valor
                .as_object()
                .ok_or_else(|| "shortcuts precisa ser um objeto {comando: combinação}".to_string())?;
            let atuais = mapa
                .entry("shortcuts")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            let atuais = atuais
                .as_object_mut()
                .ok_or_else(|| "shortcuts em formato inesperado".to_string())?;
            for (comando, combinacao) in novos {
                let conhecido = crate::commands::Command::ALL
                    .iter()
                    .any(|existente| existente.id() == comando);
                if !conhecido {
                    return Err(format!("comando desconhecido: {comando}"));
                }
                if !combinacao.is_string() {
                    return Err(format!("a combinação de {comando} precisa ser texto"));
                }
                atuais.insert(comando.clone(), combinacao.clone());
            }
            continue;
        }
        mapa.insert(campo.clone(), valor.clone());
    }

    let mut ajustado: Settings = serde_json::from_value(completo)
        .map_err(|err| format!("valor inválido: {err}"))?;
    ajustado.sanitize();
    Ok(ajustado)
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

/// Endereço do servidor: o que está no painel, senão o ambiente, senão vazio.
pub fn base_url(settings: &Settings) -> String {
    settings
        .api_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var(ENV_BASE_URL)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_default()
}

/// Credencial do basicAuth: o que está no painel, senão o ambiente. Ausente é
/// válido — a maioria dos caminhos até o servidor não exige nenhuma.
pub fn credentials(settings: &Settings) -> Option<Credentials> {
    let username = settings
        .api_user
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var(ENV_USER).ok())?;
    if username.trim().is_empty() {
        return None;
    }
    let password = settings
        .api_password
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var(ENV_PASSWORD).ok())
        .unwrap_or_default();
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
    fn sanitize_limita_a_fila_de_narracao() {
        // Zero travaria a narração com a aparência de ligada; gigante é o
        // mesmo que não ter teto.
        for (entrada, esperado) in [(0usize, 1usize), (999, 50)] {
            let mut settings = Settings {
                narration_queue_limit: entrada,
                ..Default::default()
            };
            settings.sanitize();
            assert_eq!(settings.narration_queue_limit, esperado);
        }
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
            hud_position_dictation: Some(WindowPosition { x: f64::NAN, y: 24.0 }),
            hud_position_reading: Some(WindowPosition { x: -1920.0, y: 300.0 }),
            ..Default::default()
        };
        settings.sanitize();
        assert_eq!(settings.hud_position_dictation, None);
        // Negativa é monitor à esquerda do principal, e precisa sobreviver.
        assert_eq!(
            settings.hud_position_reading,
            Some(WindowPosition { x: -1920.0, y: 300.0 })
        );
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

#[cfg(test)]
mod testes_da_ponte {
    use super::*;

    #[test]
    fn visao_nunca_leva_token_nem_senha() {
        let settings = Settings {
            bridge_token: "segredo-da-ponte".into(),
            api_password: Some("segredo-do-servidor".into()),
            ..Default::default()
        };
        let texto = visao_da_ponte(&settings).to_string();
        assert!(!texto.contains("segredo-da-ponte"));
        assert!(!texto.contains("segredo-do-servidor"));
        assert!(!texto.contains("bridge_token"));
    }

    #[test]
    fn visao_traz_os_ajustaveis() {
        let visao = visao_da_ponte(&Settings::default());
        for campo in CAMPOS_AJUSTAVEIS_PELA_PONTE {
            assert!(visao.get(*campo).is_some(), "faltou {campo}");
        }
    }

    #[test]
    fn ajuste_muda_so_o_pedido_e_normaliza() {
        let atual = Settings::default();
        let ajustado =
            ajustar_pela_ponte(&atual, &serde_json::json!({ "speed": 9.0, "narrate_tables": false }))
                .expect("ajuste aceito");
        assert_eq!(ajustado.speed, MAX_SPEED);
        assert!(!ajustado.narrate_tables);
        assert_eq!(ajustado.voice, atual.voice);
        assert_eq!(ajustado.bridge_token, atual.bridge_token);
    }

    #[test]
    fn limpeza_de_leitura_ligada_por_padrao_e_ajustavel() {
        let atual = Settings::default();
        assert!(atual.reading_rewrite_enabled, "a limpeza antes de ler vem ligada");
        let ajustado = ajustar_pela_ponte(
            &atual,
            &serde_json::json!({ "reading_rewrite_enabled": false }),
        )
        .expect("ajuste aceito");
        assert!(!ajustado.reading_rewrite_enabled);
        assert!(ajustado.normalize_before_reading == atual.normalize_before_reading);
    }

    #[test]
    fn ajuste_recusa_campo_fora_da_lista() {
        let atual = Settings::default();
        for campo in ["bridge_token", "api_password", "api_base_url", "bridge_port"] {
            let pedido = serde_json::json!({ campo: "x" });
            assert!(ajustar_pela_ponte(&atual, &pedido).is_err(), "{campo} passou");
        }
    }

    #[test]
    fn ajuste_recusa_tipo_errado_sem_mexer_em_nada() {
        let atual = Settings::default();
        assert!(ajustar_pela_ponte(&atual, &serde_json::json!({ "speed": "rápido" })).is_err());
        assert!(ajustar_pela_ponte(&atual, &serde_json::json!({ "submit_mode": "talvez" })).is_err());
        assert!(ajustar_pela_ponte(&atual, &serde_json::json!([])).is_err());
        assert!(ajustar_pela_ponte(&atual, &serde_json::json!({})).is_err());
    }

    #[test]
    fn atalho_mescla_por_comando() {
        let mut atual = Settings::default();
        atual.sanitize();
        let comando = crate::commands::Command::ALL[0];
        let outro = crate::commands::Command::ALL[1];
        let antes_do_outro = atual.shortcuts.get(outro.id()).cloned();
        let pedido = serde_json::json!({ "shortcuts": { comando.id(): "Ctrl+Alt+F9" } });
        let ajustado = ajustar_pela_ponte(&atual, &pedido).expect("ajuste aceito");
        assert_eq!(ajustado.shortcuts.get(comando.id()).map(String::as_str), Some("Ctrl+Alt+F9"));
        assert_eq!(ajustado.shortcuts.get(outro.id()).cloned(), antes_do_outro);
    }

    #[test]
    fn atalho_de_comando_inexistente_e_recusado() {
        let pedido = serde_json::json!({ "shortcuts": { "nao-existe": "Ctrl+F1" } });
        assert!(ajustar_pela_ponte(&Settings::default(), &pedido).is_err());
    }

    #[test]
    fn ancora_de_widget_muda_pela_ponte_por_papel() {
        let atual = Settings::default();
        let pedido = serde_json::json!({
            "hud_placement_dictation": "baixo-direita",
            "hud_placement_reading": "meio-direita",
        });
        let ajustado = ajustar_pela_ponte(&atual, &pedido).expect("ajuste aceito");
        assert_eq!(ajustado.hud_placement_dictation, HudPlacement::BaixoDireita);
        assert_eq!(ajustado.hud_placement_reading, HudPlacement::MeioDireita);
        // A posição arrastada continua guardada: voltar a "livre" devolve o
        // lugar de antes, em vez de apagar a lembrança de quem só ancorou.
        assert_eq!(ajustado.hud_position_dictation, atual.hud_position_dictation);
    }

    #[test]
    fn ancora_de_widget_recusa_valor_fora_do_catalogo() {
        let atual = Settings::default();
        let pedido = serde_json::json!({ "hud_placement_dictation": "em-cima-do-mouse" });
        assert!(ajustar_pela_ponte(&atual, &pedido).is_err());
    }
}
