//! Ponte HTTP local — como a extensão de navegador fala com o Vox.
//!
//! ## Por que um servidor, e não native messaging
//!
//! O native messaging do Chrome exige um executável à parte, um manifesto no
//! registro do Windows e o ID da extensão fixado nele. Três coisas para instalar
//! e manter sincronizadas. Um servidor em `127.0.0.1` é uma dependência a menos
//! e funciona igual em qualquer navegador baseado em Chromium.
//!
//! ## Por que HTTP escrito à mão
//!
//! São quatro rotas, corpo pequeno, só localhost. Trazer um framework para isso
//! adicionaria dezenas de dependências transitivas a um binário que hoje não tem
//! servidor nenhum — e o controle exato sobre quem pode chamar é justamente o
//! que não se quer terceirizar aqui.
//!
//! ## O modelo de ameaça, que não é teórico
//!
//! Um endereço em `127.0.0.1` é alcançável por **qualquer página** que o usuário
//! abrir. Um `fetch` de `https://site-qualquer.com` para cá é uma requisição
//! simples: o navegador bloqueia a *resposta* por CORS, mas o pedido chega e o
//! efeito acontece. Bloquear na resposta não serve de nada — a checagem tem que
//! acontecer antes de agir.
//!
//! Duas barreiras, as duas verificadas no servidor, antes de qualquer ação:
//!
//! 1. **`Origin`, quando existe, tem que ser `chrome-extension://`.** Corta
//!    toda página web. Pedido **sem** `Origin` passou a ser aceito: o navegador
//!    anexa `Origin` em todo POST, então sem o cabeçalho não é página — é um
//!    programa local (a página de opções da extensão, o mod do Claude Code),
//!    e quem o barra é o token, a barreira seguinte.
//! 2. **Token no cabeçalho `X-Vox-Token`.** Corta outras extensões e qualquer
//!    programa local que descubra a porta.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Tamanho máximo do corpo aceito.
///
/// Uma seleção de página inteira pode mandar bastante texto, mas nada perto
/// disto — e um teto explícito evita que um pedido com `Content-Length` enorme
/// faça o app alocar sem limite.
const MAX_CORPO: usize = 1024 * 1024;

/// Onde a leitura está agora, para a extensão desenhar o destaque.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Progress {
    /// Índice do trecho sendo falado.
    pub index: usize,
    /// Posição dentro do trecho, de 0 a 1.
    pub ratio: f32,
}

#[derive(Default)]
pub struct BridgeState {
    progress: Mutex<Progress>,
    /// Trechos da leitura corrente, na ordem. A extensão precisa deles para
    /// casar cada um com um pedaço do DOM.
    segments: Mutex<Vec<String>>,
    /// Quantas leituras começaram desde que o app subiu.
    ///
    /// É o que permite à extensão perceber uma leitura que **ela não começou** —
    /// a do atalho global, por exemplo. Sem este número ela não teria como
    /// distinguir "a mesma leitura continua" de "outra começou", e o destaque
    /// ficaria preso na página anterior.
    generation: Mutex<u64>,
}

impl BridgeState {
    pub fn set_progress(&self, index: usize, ratio: f32) {
        *self.progress.lock() = Progress { index, ratio: ratio.clamp(0.0, 1.0) };
    }

    pub fn generation(&self) -> u64 {
        *self.generation.lock()
    }

    pub fn set_segments(&self, segments: Vec<String>) {
        *self.generation.lock() += 1;
        *self.segments.lock() = segments;
        *self.progress.lock() = Progress::default();
    }

    pub fn clear(&self) {
        self.segments.lock().clear();
        *self.progress.lock() = Progress::default();
    }

    fn snapshot(&self) -> (Progress, usize) {
        (*self.progress.lock(), self.segments.lock().len())
    }

    fn segments(&self) -> Vec<String> {
        self.segments.lock().clone()
    }
}

#[derive(Deserialize)]
struct PedidoLeitura {
    text: String,
}

#[derive(Serialize)]
struct RespostaLeitura {
    ok: bool,
    /// Os trechos como o Vox os dividiu. A extensão casa esta lista com o texto
    /// selecionado para saber o que destacar — dividir dos dois lados daria
    /// listas diferentes na primeira abreviação ou reticência.
    segments: Vec<String>,
}

/// Um pedido de narração: a resposta final de um agente de CLI.
///
/// `source` é o rótulo que o painel do mod mostra ao lado do item ("claude
/// na pasta vox"), e `mode` diz se fala sozinho ao chegar ao topo (`speak`)
/// ou fica pronto esperando o play (`hold`). Ambos opcionais: sem eles valem
/// os padrões das preferências do Vox.
#[derive(Deserialize)]
pub struct PedidoNarracao {
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Deserialize)]
struct PedidoId {
    id: u64,
}

/// Por que um pedido de narração não foi aceito.
#[derive(Debug, Clone, Copy)]
pub enum NarrarErro {
    /// Narração desligada nas preferências, ou o serviço em pausa.
    Desligado,
    FilaCheia,
}

/// O que o servidor precisa saber fazer, sem depender do resto do app.
///
/// Fica como traço para o módulo não puxar o estado inteiro do app — e para o
/// teste poder passar um dublê que só registra o que foi pedido.
pub trait Acoes: Send + Sync + 'static {
    /// Começa a ler e devolve os trechos na ordem em que serão falados.
    ///
    /// É assíncrono porque o texto passa pelo servidor antes de ser dividido: a
    /// limpeza da marcação muda onde as frases começam e terminam, e a extensão
    /// precisa receber **a lista que vai ser falada**, não uma divisão do texto
    /// cru que divergiria dela na primeira URL ou no primeiro título.
    ///
    /// Futuro em caixa em vez de `async fn` no traço: `dyn Acoes` precisa ser
    /// objeto de traço para atravessar o servidor, e `async fn` em traço não
    /// produz um tipo que possa ser embrulhado assim sem uma dependência a mais.
    fn ler(&self, texto: String) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>>;
    fn parar(&self);
    fn pausar(&self);
    fn retomar(&self);

    /// O Vox está respondendo aos comandos, ou em pausa?
    ///
    /// A ponte precisa disto para o `/health` poder distinguir "não achei o Vox"
    /// de "achei, e ele está de folga" — dois problemas com a mesma aparência
    /// para quem está configurando a extensão, e soluções completamente
    /// diferentes.
    fn servico_ligado(&self) -> bool;

    /// A combinação global que começa uma leitura, como `"Ctrl+Alt+L"`.
    fn atalho_de_leitura(&self) -> String;
}

pub struct Config {
    /// Onde escutar. `127.0.0.1` é o usual; o listener do Tailscale usa o IP
    /// da tailnet para alcançar WSL em modo NAT e as máquinas remotas.
    pub endereco: String,
    pub porta: u16,
    pub token: String,
}

/// O que a ponte precisa saber fazer com pedidos de narração.
///
/// Mesma forma do `Acoes`: um traço para o módulo não puxar o app inteiro e
/// para o teste passar um dublê. As respostas já vêm em JSON, porque o que
/// muda entre "tocou" e "promovido" é apresentação, não mecânica.
pub trait AcoesNarracao: Send + Sync + 'static {
    /// Enfileira a resposta e devolve o corpo de sucesso (`id`, posição).
    fn narrar(
        &self,
        pedido: PedidoNarracao,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, NarrarErro>> + Send>>;
    /// A fila inteira para desenhar: corrente e pendentes.
    fn fila(&self) -> serde_json::Value;
    /// Manda um item tocar. `None` é "não existe" (404).
    fn tocar(&self, id: u64) -> Option<serde_json::Value>;
    /// Cancela um item. `None` é "não existe" (404).
    fn cancelar(&self, id: u64) -> Option<serde_json::Value>;
    /// Pula o corrente. `false` quando não havia nada tocando.
    fn pular(&self) -> bool;
}

/// Sobe o servidor. Só retorna se o listener não puder ser aberto.
pub async fn servir(
    config: Config,
    estado: Arc<BridgeState>,
    acoes: Arc<dyn Acoes>,
    narracao: Arc<dyn AcoesNarracao>,
) -> Result<()> {
    // O endereço vem na configuração, e não fixo: a ponte do Tailscale escuta
    // no IP da tailnet (WSL em modo NAT e máquinas remotas não alcançam o
    // loopback do Windows), e 0.0.0.0 continua fora de questão — exporia o
    // comando de fala para a rede inteira.
    let listener = TcpListener::bind((config.endereco.as_str(), config.porta))
        .await
        .with_context(|| format!("abrir {}:{} da ponte", config.endereco, config.porta))?;

    tracing::info!(endereco = %config.endereco, porta = config.porta, "ponte no ar");

    let token = Arc::new(config.token);
    loop {
        let (socket, _) = match listener.accept().await {
            Ok(par) => par,
            Err(err) => {
                tracing::warn!(?err, "falha ao aceitar conexão na ponte");
                continue;
            }
        };
        let token = token.clone();
        let estado = estado.clone();
        let acoes = acoes.clone();
        let narracao = narracao.clone();
        tokio::spawn(async move {
            if let Err(err) = atender(socket, &token, &estado, acoes.as_ref(), narracao.as_ref()).await {
                tracing::debug!(?err, "conexão da ponte encerrada com erro");
            }
        });
    }
}

struct Requisicao {
    metodo: String,
    caminho: String,
    origem: Option<String>,
    token: Option<String>,
    corpo: Vec<u8>,
}

async fn ler_requisicao(socket: &mut TcpStream) -> Result<Requisicao> {
    let mut bruto = Vec::new();
    let mut buffer = [0u8; 4096];

    let fim_cabecalhos = loop {
        if let Some(posicao) = encontrar(&bruto, b"\r\n\r\n") {
            break posicao + 4;
        }
        if bruto.len() > MAX_CORPO {
            anyhow::bail!("cabecalhos grandes demais");
        }
        let lidos = socket.read(&mut buffer).await?;
        if lidos == 0 {
            anyhow::bail!("conexao fechada antes dos cabecalhos");
        }
        bruto.extend_from_slice(&buffer[..lidos]);
    };

    let cabecalho = String::from_utf8_lossy(&bruto[..fim_cabecalhos]).to_string();
    let mut linhas = cabecalho.lines();
    let inicial = linhas.next().unwrap_or_default();
    let mut partes = inicial.split_whitespace();
    let metodo = partes.next().unwrap_or_default().to_string();
    let caminho = partes.next().unwrap_or_default().to_string();

    let mut origem = None;
    let mut token = None;
    let mut tamanho = 0usize;
    for linha in linhas {
        let Some((nome, valor)) = linha.split_once(':') else { continue };
        let valor = valor.trim();
        match nome.to_ascii_lowercase().as_str() {
            "origin" => origem = Some(valor.to_string()),
            "x-vox-token" => token = Some(valor.to_string()),
            "content-length" => tamanho = valor.parse().unwrap_or(0),
            _ => {}
        }
    }

    if tamanho > MAX_CORPO {
        anyhow::bail!("corpo grande demais: {tamanho}");
    }

    let mut corpo = bruto[fim_cabecalhos..].to_vec();
    while corpo.len() < tamanho {
        let lidos = socket.read(&mut buffer).await?;
        if lidos == 0 {
            break;
        }
        corpo.extend_from_slice(&buffer[..lidos]);
    }
    corpo.truncate(tamanho);

    Ok(Requisicao { metodo, caminho, origem, token, corpo })
}

fn encontrar(agulha: &[u8], padrao: &[u8]) -> Option<usize> {
    agulha.windows(padrao.len()).position(|janela| janela == padrao)
}

/// Só extensão de Chromium quando há `Origin`; sem `Origin`, passa.
///
/// A checagem de origem existe para uma coisa só: impedir que uma **página web**
/// mande o computador falar. Loopback não é fronteira de confiança — qualquer
/// página aberta alcança `127.0.0.1`, e o CORS bloqueia a *resposta*, não o
/// efeito. Contra outros programas da máquina quem defende é o token.
///
/// O navegador manda `Origin` sempre que o método não é GET/HEAD, e sempre que o
/// pedido é `cors` — que é o padrão de um `fetch`. Uma página web, portanto,
/// **não consegue fazer um POST sem `Origin`**: o cabeçalho ausente só existe
/// para programas locais — a página de opções da extensão, que o Chrome não
/// autentica em GET simples, e o mod do Claude Code, que fala com a ponte de
/// dentro do processo do agente. Ambos chegam ao token, a barreira de verdade.
fn origem_permitida(origem: Option<&str>) -> bool {
    match origem {
        Some(valor) => valor.starts_with("chrome-extension://"),
        None => true,
    }
}

async fn atender(
    mut socket: TcpStream,
    token_esperado: &str,
    estado: &BridgeState,
    acoes: &dyn Acoes,
    narracao: &dyn AcoesNarracao,
) -> Result<()> {
    let pedido = ler_requisicao(&mut socket).await?;
    let origem = pedido.origem.as_deref();

    // O preflight responde antes da checagem de token: o navegador o manda sem
    // cabeçalhos nossos, por definição. A origem, essa sim, já vale aqui.
    if pedido.metodo == "OPTIONS" {
        let resposta = if origem_permitida(origem) {
            responder(204, "", origem)
        } else {
            responder(403, "", None)
        };
        socket.write_all(resposta.as_bytes()).await?;
        return Ok(());
    }

    if !origem_permitida(origem) {
        tracing::warn!(?origem, caminho = %pedido.caminho, "ponte recusou a origem");
        // A mensagem diz o que aconteceu **e** o que fazer. Um "403" sozinho
        // manda a pessoa adivinhar, e ela adivinha errado: da primeira vez que
        // isto apareceu, a suspeita caiu num container que não tinha relação
        // nenhuma com a recusa.
        let corpo = match origem {
            None => "{\"error\":\"pedido sem cabecalho Origin\",\"detail\":\"So GET e aceito sem origem. Um POST precisa vir de uma extensao do navegador.\"}",
            Some(_) => "{\"error\":\"origem nao permitida\",\"detail\":\"A ponte so aceita chrome-extension://. Paginas web sao recusadas de proposito: qualquer pagina aberta alcanca 127.0.0.1.\"}",
        };
        socket.write_all(responder(403, corpo, None).as_bytes()).await?;
        return Ok(());
    }

    if pedido.token.as_deref() != Some(token_esperado) {
        let motivo = if pedido.token.is_none() { "ausente" } else { "diferente" };
        tracing::warn!(caminho = %pedido.caminho, motivo, "ponte recusou o token");
        let corpo = if pedido.token.is_none() {
            "{\"error\":\"token ausente\",\"detail\":\"Mande o cabecalho X-Vox-Token. O valor esta em bridge_token, nas preferencias do Vox.\"}"
        } else {
            "{\"error\":\"token invalido\",\"detail\":\"O valor nao confere com bridge_token. Copie de novo nas preferencias do Vox, secao Extensao.\"}"
        };
        socket.write_all(responder(401, corpo, origem).as_bytes()).await?;
        return Ok(());
    }

    let (codigo, corpo) = match (pedido.metodo.as_str(), pedido.caminho.as_str()) {
        ("POST", "/read") => match serde_json::from_slice::<PedidoLeitura>(&pedido.corpo) {
            Ok(entrada) if !entrada.text.trim().is_empty() => {
                let segments = acoes.ler(entrada.text).await;
                let resposta = RespostaLeitura { ok: true, segments };
                (200, serde_json::to_string(&resposta)?)
            }
            Ok(_) => (400, "{\"error\":\"texto vazio\"}".to_string()),
            Err(err) => (400, format!("{{\"error\":\"json invalido: {err}\"}}")),
        },

        ("POST", "/stop") => {
            acoes.parar();
            (200, "{\"ok\":true}".to_string())
        }

        ("POST", "/pause") => {
            acoes.pausar();
            (200, "{\"ok\":true}".to_string())
        }

        ("POST", "/resume") => {
            acoes.retomar();
            (200, "{\"ok\":true}".to_string())
        }

        // Narração: a resposta final de um agente de CLI, pela fila.
        //
        // O POST responde na hora, sem esperar a adaptação nem a fala: quem
        // chama é um mod dentro do processo do agente, e o pedido não pode
        // segurar o turno dele. O que acontece depois é acompanhado pela
        // rota de fila, que devolve corrente e pendentes.
        ("POST", "/narrate") => match serde_json::from_slice::<PedidoNarracao>(&pedido.corpo) {
            Ok(entrada) if !entrada.text.trim().is_empty() => {
                let saida = narracao
                    .narrar(entrada)
                    .await
                    .map(|corpo| (200, corpo.to_string()));
                match saida {
                    Ok(resposta) => resposta,
                    Err(NarrarErro::Desligado) => (
                        403,
                        "{\"error\":\"narracao desligada\",\"detail\":\"Ligue Narração nas \
                         preferências do Vox, e confira se o serviço não está em pausa.\"}"
                            .to_string(),
                    ),
                    Err(NarrarErro::FilaCheia) => (
                        429,
                        "{\"error\":\"fila cheia\",\"detail\":\"Cancele um item no painel ou \
                         espere a fila andar.\"}"
                            .to_string(),
                    ),
                }
            }
            Ok(_) => (400, "{\"error\":\"texto vazio\"}".to_string()),
            Err(err) => (400, format!("{{\"error\":\"json invalido: {err}\"}}")),
        },

        ("GET", "/narrate/queue") => (200, narracao.fila().to_string()),

        ("POST", "/narrate/play") => match serde_json::from_slice::<PedidoId>(&pedido.corpo) {
            Ok(entrada) => match narracao.tocar(entrada.id) {
                Some(corpo) => (200, corpo.to_string()),
                None => (
                    404,
                    "{\"error\":\"item inexistente\",\"detail\":\"Ele já tocou, foi cancelado \
                     ou nunca existiu.\"}"
                        .to_string(),
                ),
            },
            Err(err) => (400, format!("{{\"error\":\"json invalido: {err}\"}}")),
        },

        ("POST", "/narrate/cancel") => match serde_json::from_slice::<PedidoId>(&pedido.corpo) {
            Ok(entrada) => match narracao.cancelar(entrada.id) {
                Some(corpo) => (200, corpo.to_string()),
                None => (
                    404,
                    "{\"error\":\"item inexistente\",\"detail\":\"Ele já tocou, foi cancelado \
                     ou nunca existiu.\"}"
                        .to_string(),
                ),
            },
            Err(err) => (400, format!("{{\"error\":\"json invalido: {err}\"}}")),
        },

        ("POST", "/narrate/skip") => {
            if narracao.pular() {
                (200, "{\"ok\":true}".to_string())
            } else {
                (404, "{\"error\":\"nada tocando\"}".to_string())
            }
        }

        ("GET", "/progress") => {
            let (progress, total) = estado.snapshot();
            let corpo = serde_json::json!({
                "index": progress.index,
                "ratio": progress.ratio,
                "segments": total,
                "generation": estado.generation(),
            });
            (200, serde_json::to_string(&corpo)?)
        }

        // O `service_enabled` existe para a extensão poder dizer "conectado, mas
        // o Vox está em pausa". Sem ele, a configuração pareceria certa e nada
        // funcionaria — o pior par de sintomas possível.
        // Os trechos da leitura corrente, para a extensão acompanhar uma leitura
        // que ela não começou — a do atalho global. Sem isto ela só conseguiria
        // destacar o que tivesse pedido ela mesma.
        ("GET", "/current") => {
            let segments = estado.segments();
            let corpo = serde_json::json!({
                "generation": estado.generation(),
                "segments": segments,
            });
            (200, serde_json::to_string(&corpo)?)
        }

        ("GET", "/health") => {
            let corpo = serde_json::json!({
                "ok": true,
                "app": "vox",
                "service_enabled": acoes.servico_ligado(),
                // Qual tecla a página deve escutar para acompanhar uma leitura
                // começada pelo atalho global. Vai daqui porque uma cópia na
                // extensão divergiria no primeiro ajuste feito no painel — e o
                // sintoma seria a leitura funcionando com o destaque parado.
                "read_shortcut": acoes.atalho_de_leitura(),
            });
            (200, serde_json::to_string(&corpo)?)
        }

        _ => (404, "{\"error\":\"rota desconhecida\"}".to_string()),
    };

    socket.write_all(responder(codigo, &corpo, origem).as_bytes()).await?;
    socket.flush().await?;
    Ok(())
}

fn responder(codigo: u16, corpo: &str, origem: Option<&str>) -> String {
    let motivo = match codigo {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };

    // Ecoa a origem exata em vez de `*`: devolver curinga aqui autorizaria
    // qualquer extensão instalada a ler a resposta.
    let cors = match origem {
        Some(valor) => format!(
            "Access-Control-Allow-Origin: {valor}\r\n\
             Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type, X-Vox-Token\r\n\
             Access-Control-Max-Age: 600\r\n"
        ),
        None => String::new(),
    };

    format!(
        "HTTP/1.1 {codigo} {motivo}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         {cors}\
         Connection: close\r\n\r\n{corpo}",
        corpo.len()
    )
}

/// O IPv4 do Tailscale desta máquina, ou `None`.
///
/// Usado para o segundo listener da ponte: o loopback não alcança WSL em
/// modo NAT nem as máquinas do homelab, e o IP da tailnet alcança os dois —
/// o próprio Tailscale cifra o que passa por ele.
pub fn tailscale_ipv4() -> Option<std::net::IpAddr> {
    for programa in ["tailscale", r"C:\Program Files\Tailscale\tailscale.exe"] {
        let Ok(saida) = std::process::Command::new(programa).args(["ip", "-4"]).output() else {
            continue;
        };
        if !saida.status.success() {
            continue;
        }
        for linha in String::from_utf8_lossy(&saida.stdout).lines() {
            if let Ok(ip) = linha.trim().parse::<std::net::IpAddr>() {
                if ip.is_ipv4() {
                    return Some(ip);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagina_web_nunca_passa() {
        // O que a regra existe para barrar: qualquer página aberta alcança
        // 127.0.0.1, e o CORS bloqueia a resposta, não o efeito.
        for origem in ["https://exemplo.com", "http://127.0.0.1:8765"] {
            assert!(!origem_permitida(Some(origem)));
        }
    }

    #[test]
    fn extensao_passa() {
        assert!(origem_permitida(Some("chrome-extension://abcdefghijklmnop")));
    }

    /// O Chrome não anexa `Origin` num GET simples vindo de uma página de
    /// extensão, e o mod do Claude Code não anexa em pedido nenhum — ele não
    /// é navegador. Aceitar o cabeçalho ausente é seguro porque uma página
    /// web **não consegue** fazer um POST sem `Origin`: o navegador o anexa
    /// sempre que o método não é GET/HEAD. Quem barra o programa local é o
    /// token, uma checagem adiante.
    #[test]
    fn sem_origem_passa_e_o_token_decide() {
        assert!(origem_permitida(None));
    }

    #[test]
    fn resposta_ecoa_a_origem_e_nunca_curinga() {
        let origem = "chrome-extension://abc";
        let saida = responder(200, "{}", Some(origem));
        assert!(saida.contains(&format!("Access-Control-Allow-Origin: {origem}")));
        assert!(!saida.contains("Allow-Origin: *"));
    }

    #[test]
    fn sem_origem_nao_ha_cabecalho_de_cors() {
        let saida = responder(403, "{}", None);
        assert!(!saida.contains("Access-Control-Allow-Origin"));
    }

    #[test]
    fn encontra_o_fim_dos_cabecalhos() {
        assert_eq!(encontrar(b"GET / HTTP/1.1\r\n\r\nX", b"\r\n\r\n"), Some(14));
        assert_eq!(encontrar(b"sem separador", b"\r\n\r\n"), None);
    }

    #[test]
    fn progresso_fica_entre_zero_e_um() {
        let estado = BridgeState::default();
        estado.set_progress(3, 2.5);
        assert_eq!(estado.snapshot().0.ratio, 1.0);
        estado.set_progress(3, -1.0);
        assert_eq!(estado.snapshot().0.ratio, 0.0);
    }
}
