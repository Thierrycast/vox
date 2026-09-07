//! Ícone de bandeja.
//!
//! O Vox não tem janela principal: o HUD é flutuante e some sozinho, e o leitor
//! só aparece quando há algo para ler. Sem a bandeja o app subiria e **não
//! haveria como fechá-lo** a não ser pelo Gerenciador de Tarefas — e nem como
//! descobrir se ele está rodando.
//!
//! O menu também é onde os problemas ficam visíveis. Um atalho global que falhou
//! ao registrar é silencioso por natureza: a tecla simplesmente não faz nada, e
//! o usuário conclui que o app está quebrado. Aqui isso vira uma linha no menu,
//! dizendo qual atalho falhou e por quê.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::AppState;

/// Estado dos atalhos, para o menu poder relatar falhas.
#[derive(Debug, Default, Clone)]
pub struct ShortcutReport {
    pub dictate: Option<String>,
    pub read: Option<String>,
    /// A combinação configurada, para o menu mostrar a real e não uma fixa.
    pub dictate_label: String,
    pub read_label: String,
}

impl ShortcutReport {
    pub fn has_failure(&self) -> bool {
        self.dictate.is_some() || self.read.is_some()
    }
}

pub fn build(app: &AppHandle, report: &ShortcutReport) -> tauri::Result<()> {
    let separador = PredefinedMenuItem::separator(app)?;

    let preferencias = MenuItem::with_id(
        app, "preferencias", "Preferências…", true, None::<&str>)?;
    let ler = MenuItem::with_id(
        app, "ler_clipboard", "Ler a área de transferência", true, None::<&str>)?;
    let leitor = MenuItem::with_id(
        app, "abrir_leitor", "Abrir a janela de leitura", true, None::<&str>)?;
    let vozes = MenuItem::with_id(
        app, "abrir_vozes", "Comparar vozes no navegador", true, None::<&str>)?;
    // O arquivo continua acessível: o painel cobre o que se muda no dia a dia,
    // e o JSON cobre o resto — inclusive copiar o token da ponte.
    let config = MenuItem::with_id(
        app, "abrir_config", "Abrir o arquivo de preferências", true, None::<&str>)?;
    let sair = MenuItem::with_id(app, "sair", "Sair do Vox", true, None::<&str>)?;

    // Os atalhos aparecem como itens desabilitados: servem de lembrete, e é
    // onde uma falha de registro fica visível em vez de silenciosa.
    let atalho_ditado = MenuItem::with_id(
        app,
        "info_ditado",
        match &report.dictate {
            None => format!("Ditar   {}", report.dictate_label),
            Some(erro) => format!("⚠ Ditar — indisponível ({erro})"),
        },
        false,
        None::<&str>,
    )?;
    let atalho_leitura = MenuItem::with_id(
        app,
        "info_leitura",
        match &report.read {
            None => format!("Ler     {}", report.read_label),
            Some(erro) => format!("⚠ Ler — indisponível ({erro})"),
        },
        false,
        None::<&str>,
    )?;

    let ajuda = Submenu::with_id_and_items(
        app, "ajuda", "Atalhos", true, &[&atalho_ditado, &atalho_leitura])?;

    let menu = Menu::with_items(
        app,
        &[
            &ajuda, &separador,
            &preferencias, &ler, &leitor, &separador,
            &vozes, &config, &separador, &sair,
        ],
    )?;

    let dica = if report.has_failure() {
        "Vox — um atalho não pôde ser registrado"
    } else {
        "Vox — ditado e leitura por voz"
    };

    TrayIconBuilder::with_id("vox")
        .icon(app.default_window_icon().cloned().ok_or_else(|| {
            tauri::Error::AssetNotFound("ícone do app".into())
        })?)
        .tooltip(dica)
        .menu(&menu)
        // O menu não abre no clique esquerdo: esse gesto abre o leitor, que é o
        // que se quer 90% das vezes. O menu fica no botão direito, como manda o
        // costume do Windows.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| responder_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                // O clique esquerdo abre as preferências, e não o leitor: o
                // leitor já está a um clique no relógio da pílula, e o painel
                // não tinha nenhum caminho curto.
                crate::open_settings_from_tray(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn responder_menu(app: &AppHandle, id: &str) {
    match id {
        "sair" => {
            tracing::info!("saindo pelo menu da bandeja");
            app.exit(0);
        }
        "abrir_leitor" => mostrar_leitor(app),
        "preferencias" => crate::open_settings_from_tray(app),
        "ler_clipboard" => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = crate::read_clipboard_aloud(handle).await {
                    tracing::error!(?err, "leitura da área de transferência falhou");
                }
            });
        }
        "abrir_vozes" => {
            // A página de comparação vive no próprio servidor; abrir no
            // navegador evita embutir uma terceira janela no app para algo que
            // se usa uma vez a cada tanto.
            let url = format!("{}/vozes", crate::config::base_url());
            if let Err(err) = abrir_no_navegador(&url) {
                tracing::error!(?err, %url, "não foi possível abrir o navegador");
            }
        }
        "abrir_config" => {
            let caminho = crate::config::settings_path();
            // Grava os padrões antes de abrir: sem isso, na primeira vez o
            // usuário abriria um arquivo que ainda não existe.
            if !caminho.exists() {
                if let Some(state) = app.try_state::<AppState>() {
                    let _ = state.settings.lock().save();
                }
            }
            if let Err(err) = abrir_no_navegador(&caminho.to_string_lossy()) {
                tracing::error!(?err, ?caminho, "não foi possível abrir as preferências");
            }
        }
        outro => tracing::debug!(id = outro, "item de menu sem ação"),
    }
}

fn mostrar_leitor(app: &AppHandle) {
    if let Some(janela) = app.get_webview_window("reader") {
        let _ = janela.show();
        let _ = janela.unminimize();
        let _ = janela.set_focus();
    }
}

/// Abre um caminho ou URL no aplicativo padrão do sistema.
fn abrir_no_navegador(alvo: &str) -> std::io::Result<()> {
    // `cmd /c start` no lugar de uma dependência só para isto. O primeiro
    // argumento de `start` é o título da janela, por isso o `""` — sem ele um
    // caminho entre aspas seria interpretado como título e nada abriria.
    std::process::Command::new("cmd")
        .args(["/C", "start", "", alvo])
        .spawn()
        .map(|_| ())
}
