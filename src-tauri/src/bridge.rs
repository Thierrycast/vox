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
//! 1. **`Origin` tem que ser `chrome-extension://`.** Corta toda página web.
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
}

impl BridgeState {
    pub fn set_progress(&self, index: usize, ratio: f32) {
        *self.progress.lock() = Progress { index, ratio: ratio.clamp(0.0, 1.0) };
    }

    pub fn set_segments(&self, segments: Vec<String>) {
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
}

pub struct Config {
    pub porta: u16,
    pub token: String,
}

/// Sobe o servidor. Só retorna se o listener não puder ser aberto.
pub async fn servir(config: Config, estado: Arc<BridgeState>, acoes: Arc<dyn Acoes>) -> Result<()> {
    // Só loopback. Escutar em 0.0.0.0 exporia o comando de fala para a rede
    // inteira, e não há caso de uso para isso.
    let listener = TcpListener::bind(("127.0.0.1", config.porta))
        .await
        .with_context(|| format!("abrir a porta {} da ponte", config.porta))?;

    tracing::info!(porta = config.porta, "ponte da extensão no ar");

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
        tokio::spawn(async move {
            if let Err(err) = atender(socket, &token, &estado, acoes.as_ref()).await {
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

/// Só extensão de Chromium. Uma página web tem `Origin` do site dela e cai aqui.
fn origem_permitida(origem: Option<&str>) -> bool {
    matches!(origem, Some(valor) if valor.starts_with("chrome-extension://"))
}

async fn atender(
    mut socket: TcpStream,
    token_esperado: &str,
    estado: &BridgeState,
    acoes: &dyn Acoes,
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
        let corpo = "{\"error\":\"origem nao permitida\"}";
        socket.write_all(responder(403, corpo, None).as_bytes()).await?;
        return Ok(());
    }

    if pedido.token.as_deref() != Some(token_esperado) {
        tracing::warn!(caminho = %pedido.caminho, "ponte recusou o token");
        let corpo = "{\"error\":\"token invalido\"}";
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

        ("GET", "/progress") => {
            let (progress, total) = estado.snapshot();
            let corpo = serde_json::json!({
                "index": progress.index,
                "ratio": progress.ratio,
                "segments": total,
            });
            (200, serde_json::to_string(&corpo)?)
        }

        ("GET", "/health") => (200, "{\"ok\":true,\"app\":\"vox\"}".to_string()),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_extensao_passa_na_origem() {
        assert!(origem_permitida(Some("chrome-extension://abcdefghijklmnop")));
        assert!(!origem_permitida(Some("https://exemplo.com")));
        assert!(!origem_permitida(Some("http://127.0.0.1:8765")));
        assert!(!origem_permitida(None));
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
