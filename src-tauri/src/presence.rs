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
    let _ = window.hide();
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
            tracing::error!(
                "o widget está na tela mas continua sem pintar, mesmo depois de acordar o WebView2"
            );
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
