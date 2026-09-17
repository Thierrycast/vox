//! Mostrar e esconder as janelas flutuantes sem que o WebView2 pare de pintar.
//!
//! ## O defeito
//!
//! O widget sumia de vez em quando, com o Vox funcionando: o ditado colava, a
//! leitura falava, e na tela nada. Medido com ele sumido, a janela estava
//! visível, por cima de tudo, dentro do monitor e no tamanho certo — e a captura
//! da tela naquele retângulo vinha vazia. Os quatro processos de renderização
//! estavam vivos desde a partida. A janela existia; o conteúdo não era pintado.
//!
//! O Tauri esconde a janela nativa e não conta nada ao controlador do WebView2,
//! que continua se achando visível. Quem decide se a página desenha é o próprio
//! Chromium, pela detecção de oclusão: ele observa as janelas e para de compor as
//! que acha encobertas. No bloqueio de tela e na suspensão ele marca todas como
//! encobertas; uma janela que estava **escondida** nesse momento pode reaparecer
//! sem que ele reavalie. No log do sistema, o notebook tinha passado por oito
//! ciclos de bloqueio e tela apagada desde a partida do Vox.
//!
//! ## A saída
//!
//! Duas coisas, uma segurando a outra:
//!
//! - a detecção de oclusão sai (`CalculateNativeWinOcclusion` desligado no
//!   `tauri.conf.json`), para o Chromium não ter um estado próprio que possa
//!   ficar preso;
//! - a visibilidade passa a ser dita ao controlador aqui. Escondida, a página
//!   para de desenhar e não gasta nada; mostrada, ela volta a compor na hora.
//!
//! E, como a primeira explicação de um sumiço já custou uma investigação, cada
//! vez que o widget aparece ele confirma que pintou um quadro. Se não pintar, o
//! log diz, e a visibilidade é acordada de novo.
//!
//! ## O que essa confirmação ainda não cobria
//!
//! A confirmação de pintura só disparava na transição de escondido pra
//! visível (`reveal()`). Ela não protegia contra a mesma composição parar de
//! funcionar **depois**, com a janela já mostrada — sem uma transição, nada
//! disparava a checagem de novo. Foi exatamente esse o próximo relato: o HUD
//! ficou visível numa leitura, parou de pintar em algum momento no meio do
//! caminho, e continuou "visível e vazio" por dezenas de minutos até a pessoa
//! reiniciar o app na mão — a única coisa que de fato trouxe ele de volta.
//!
//! `watch_hud` fecha essa lacuna rodando a mesma checagem periodicamente
//! enquanto a janela estiver visível, não só no instante em que aparece.
//!
//! ## "Sempre por cima" tem o mesmo defeito de fundo
//!
//! `alwaysOnTop` nasce ligado no `tauri.conf.json`, mas isso é o estado
//! inicial, não uma garantia contínua — o Windows tira o topo do z-order de
//! quem tinha quando outra janela também topmost sobe (um vídeo em tela
//! cheia numa aba do navegador, um instalador, um UAC), e nada devolve
//! sozinho. Reafirmar só ao mudar de forma (`shape_hud`) cobre o caminho
//! comum, mas deixa o mesmo buraco: um widget parado, visível, sem trocar de
//! forma, pode ficar atrás de outra coisa indefinidamente. `watch_hud`
//! reafirma isso também, com frequência bem maior que a checagem de pintura
//! — é uma chamada síncrona do Win32, quase de graça.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

/// Número de cada pedido de confirmação, para uma resposta atrasada de um
/// pedido antigo não passar por resposta ao atual.
static PRESENCE_REQUEST: AtomicU64 = AtomicU64::new(0);
static PRESENCE_CONFIRMED: AtomicU64 = AtomicU64::new(0);

/// Quanto esperar pelo primeiro quadro. Um quadro leva 16 ms; o resto é folga
/// para máquina ocupada e para o evento atravessar a ponte até a página.
const PRESENCE_TIMEOUT: Duration = Duration::from_millis(900);

/// Tentativas de acordar a página antes de desistir e só registrar.
const PRESENCE_ATTEMPTS: u8 = 2;

/// De quanto em quanto tempo o vigia reafirma "sempre por cima".
///
/// Barato — é só uma chamada síncrona do Win32 — então não custa nada
/// conferir com frequência. Outra janela pode assumir o topo do z-order a
/// qualquer momento (um vídeo em tela cheia numa aba, um instalador, um UAC),
/// e o Vox precisa voltar pra cima logo, não só na próxima vez que mudar de
/// forma.
const TOPMOST_INTERVAL: Duration = Duration::from_secs(3);

/// De quanto em quanto tempo o vigia confere se o HUD visível continua
/// pintando.
///
/// Mais cara que a reafirmação de "sempre por cima" — envolve ida e volta até
/// a página e uma espera — então roda com menos frequência: a cada
/// `PAINT_CHECK_A_CADA` batimentos do temporizador de topmost.
const PAINT_CHECK_A_CADA: u32 = 5;

fn set_webview_visible(window: &WebviewWindow, visible: bool) {
    let result = window.with_webview(move |webview| {
        #[cfg(windows)]
        unsafe {
            let controller = webview.controller();
            if let Err(error) = controller.SetIsVisible(visible) {
                tracing::warn!(?error, visible, "o WebView2 recusou a mudança de visibilidade");
            }
            if visible {
                // Faz o controlador recalcular onde está na tela: depois de uma
                // troca de monitor ou de escala, ele pode estar compondo para um
                // lugar que não existe mais.
                let _ = controller.NotifyParentWindowPositionChanged();
            }
        }
        #[cfg(not(windows))]
        let _ = (webview, visible);
    });
    if let Err(error) = result {
        tracing::warn!(?error, janela = window.label(), "não deu para alcançar o WebView2");
    }
}

/// Mostra a janela e garante que a página volte a desenhar.
pub fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    set_webview_visible(window, true);
    if window.label() == "hud" {
        confirm_paint(window.app_handle().clone(), PRESENCE_ATTEMPTS);
    }
    Ok(())
}

/// Esconde a janela e diz ao WebView2 que ninguém está olhando.
pub fn conceal(window: &WebviewWindow) {
    set_webview_visible(window, false);
    // Igual a `reveal`: um `hide()` que falha não pode passar batido — sem o
    // log, a janela ficaria visível pro Windows enquanto o WebView2 já se
    // acha escondido, o mesmo desencontro de estado que este arquivo existe
    // pra evitar, só que ao contrário.
    if let Err(error) = window.hide() {
        tracing::warn!(?error, janela = window.label(), "não deu para esconder a janela");
    }
}

/// A página respondeu a um pedido de confirmação de dentro de um quadro.
pub fn confirm(request: u64) {
    PRESENCE_CONFIRMED.fetch_max(request, Ordering::Relaxed);
}

/// Pede à página que responda de dentro de um `requestAnimationFrame`.
///
/// É o sinal certo porque é exatamente o que falha: página que o Chromium acha
/// encoberta continua recebendo eventos e rodando JavaScript, mas não ganha
/// quadro nenhum. Resposta que chega prova que houve pintura.
fn confirm_paint(app: AppHandle, attempts_left: u8) {
    let request = PRESENCE_REQUEST.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = app.emit_to("hud", "vox://presence-check", serde_json::json!({ "request": request }));

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PRESENCE_TIMEOUT).await;

        if PRESENCE_CONFIRMED.load(Ordering::Relaxed) >= request {
            return;
        }
        let Some(window) = app.get_webview_window("hud") else { return };
        // Escondida antes de dar tempo — um "Copiado" curto, um cancelamento —
        // não é falha de pintura.
        if !window.is_visible().unwrap_or(false) {
            return;
        }
        // Um pedido mais novo já está em curso e vai tirar a própria conclusão.
        if PRESENCE_REQUEST.load(Ordering::Relaxed) != request {
            return;
        }

        if attempts_left == 0 {
            // Acordar o controlador (a troca de visibilidade acima) não
            // bastou — é hora de um remédio mais forte. Recarregar a página
            // obriga o WebView2 a reconstruir a superfície de composição do
            // zero, e não só reafirmar uma flag; é o mesmo efeito que
            // reiniciar o app inteiro tinha na prática, sem precisar disso.
            tracing::error!(
                "o widget está na tela mas continua sem pintar mesmo depois de acordar o \
                 WebView2; recarregando a página"
            );
            if let Err(error) = window.reload() {
                tracing::warn!(?error, "não deu nem para recarregar a página do HUD");
            }
            return;
        }

        tracing::warn!(
            tentativas_restantes = attempts_left,
            "o widget apareceu sem pintar nenhum quadro; acordando o WebView2"
        );
        // Falso e depois verdadeiro: repetir `true` num controlador que já se acha
        // visível não muda nada, e é justamente a transição que faz a página
        // voltar a compor.
        set_webview_visible(&window, false);
        tokio::time::sleep(Duration::from_millis(60)).await;
        set_webview_visible(&window, true);
        confirm_paint(app, attempts_left - 1);
    });
}

/// Vigia o HUD pro resto da vida do processo, enquanto ele estiver visível:
///
/// - a cada `TOPMOST_INTERVAL`, reafirma "sempre por cima" — sozinho isso não
///   bastava: era reafirmado só quando a forma mudava (`shape_hud`), e uma
///   janela visível que ficasse parada num estado (idle, ou uma leitura
///   longa) podia perder o topo do z-order pra outra coisa e nunca recuperar
///   até a próxima mudança de forma;
/// - a cada `PAINT_CHECK_A_CADA` batimentos desses, roda também a checagem de
///   pintura que `reveal()` dispara — pegando o caso em que o WebView2 para
///   de pintar **depois** de já mostrado, sem uma transição de visibilidade
///   que acionasse a checagem sozinha.
///
/// Chamado uma vez no arranque; o `tick` que não faz nada com a janela
/// escondida é mais simples do que ligar e desligar temporizadores a cada
/// `show`/`hide`.
pub fn watch_hud(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut batimento = tokio::time::interval(TOPMOST_INTERVAL);
        // O primeiro `tick` de um `interval` novo resolve na hora; sem pular
        // ele, a primeira checagem rodaria antes mesmo do primeiro `reveal()`
        // ter chance de mostrar alguma coisa.
        batimento.tick().await;

        let mut contagem: u32 = 0;
        loop {
            batimento.tick().await;
            let Some(window) = app.get_webview_window("hud") else { continue };
            if !window.is_visible().unwrap_or(false) {
                continue;
            }

            let _ = window.set_always_on_top(true);

            contagem += 1;
            if contagem >= PAINT_CHECK_A_CADA {
                contagem = 0;
                confirm_paint(app.clone(), PRESENCE_ATTEMPTS);
            }
        }
    });
}
