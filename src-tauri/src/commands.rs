//! O catálogo de comandos do Vox.
//!
//! ## Por que existe uma tabela
//!
//! Cada atalho global custava três coisas espalhadas: um campo em `Settings`, um
//! bloco de registro em `main.rs` e uma linha no menu da bandeja. Com três
//! comandos isso era repetição; com oito seria um lugar para esquecer um deles e
//! não perceber. Aqui o comando é declarado uma vez e as três coisas derivam
//! dele.
//!
//! ## Sobre as combinações escolhidas
//!
//! Duas famílias, separadas pelo que a mão está fazendo:
//!
//! - **`Ctrl+Shift+<letra>`** para o ditado, que se usa enquanto se escreve.
//! - **`Ctrl+Alt+<letra>`** para a leitura e a janela, que se usam enquanto se lê.
//!
//! Há uma armadilha específica de teclado brasileiro aqui, e ela é o motivo de
//! as letras serem estas: **no ABNT2, `AltGr` é `Ctrl+Alt`**. Registrar
//! `Ctrl+Alt+Q` como atalho global rouba o `/` de quem digita, e `Ctrl+Alt+E`
//! rouba o `€`. As letras deste catálogo (L, V, P, K, G, O) não produzem
//! caractere nenhum com AltGr no ABNT2, então nada é tirado de quem escreve.
//!
//! Atalho global é recurso disputado e quem registra primeiro leva. O Vox sobe
//! depois do navegador, então uma combinação tomada falha em silêncio — por isso
//! o registro reporta o que falhou, e o menu da bandeja mostra.

use serde::Serialize;

/// Tudo o que o app sabe fazer por atalho.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Dictate,
    CancelDictation,
    ReadSelection,
    TogglePlayback,
    StopReading,
    ToggleCaptions,
    ShowWidget,
    OpenSettings,
}

impl Command {
    /// A ordem aqui é a que aparece no painel e na bandeja: primeiro o que se usa
    /// todo dia, depois o que se usa quando algo já está acontecendo.
    pub const ALL: [Command; 8] = [
        Command::Dictate,
        Command::CancelDictation,
        Command::ReadSelection,
        Command::TogglePlayback,
        Command::StopReading,
        Command::ToggleCaptions,
        Command::ShowWidget,
        Command::OpenSettings,
    ];

    /// Chave estável. É o que vai para o `settings.json`, então mudar isto
    /// descarta a combinação que a pessoa tinha escolhido.
    pub fn id(self) -> &'static str {
        match self {
            Command::Dictate => "dictate",
            Command::CancelDictation => "cancel_dictation",
            Command::ReadSelection => "read_selection",
            Command::TogglePlayback => "toggle_playback",
            Command::StopReading => "stop_reading",
            Command::ToggleCaptions => "toggle_captions",
            Command::ShowWidget => "show_widget",
            Command::OpenSettings => "open_settings",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Command::Dictate => "Ditar",
            Command::CancelDictation => "Cancelar o ditado",
            Command::ReadSelection => "Ler a seleção",
            Command::TogglePlayback => "Pausar e retomar",
            Command::StopReading => "Parar a leitura",
            Command::ToggleCaptions => "Legenda guiada",
            Command::ShowWidget => "Mostrar o widget",
            Command::OpenSettings => "Abrir as preferências",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Command::Dictate => "Uma vez grava; de novo transcreve e entrega.",
            Command::CancelDictation => "Descarta a gravação em curso sem transcrever nada.",
            Command::ReadSelection => "Lê o que estiver selecionado. Durante a leitura, pausa e retoma.",
            Command::TogglePlayback => "Só pausa e retoma — nunca começa uma leitura nova.",
            Command::StopReading => "Encerra a leitura e esconde o widget.",
            Command::ToggleCaptions => "Abre e fecha o texto dentro da pílula.",
            Command::ShowWidget => "Traz a pílula à tela sem começar nada.",
            Command::OpenSettings => "Esta janela.",
        }
    }

    pub fn default_binding(self) -> &'static str {
        match self {
            Command::Dictate => "Ctrl+Shift+D",
            Command::CancelDictation => "Ctrl+Shift+X",
            Command::ReadSelection => "Ctrl+Alt+L",
            Command::TogglePlayback => "Ctrl+Alt+P",
            Command::StopReading => "Ctrl+Alt+K",
            Command::ToggleCaptions => "Ctrl+Alt+G",
            Command::ShowWidget => "Ctrl+Alt+V",
            Command::OpenSettings => "Ctrl+Alt+O",
        }
    }
}

/// Uma entrada do catálogo como o painel a desenha.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub default_binding: &'static str,
    /// A combinação em uso, que pode não ser a padrão.
    pub binding: String,
    /// Por que este comando não está respondendo, quando for o caso.
    pub failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_e_combinacoes_sao_unicos() {
        // Dois comandos com o mesmo id compartilhariam a preferência em silêncio;
        // com a mesma combinação, o segundo registro falharia e o comando
        // simplesmente não responderia.
        let mut ids: Vec<&str> = Command::ALL.iter().map(|c| c.id()).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "há ids repetidos no catálogo");

        let mut teclas: Vec<&str> = Command::ALL.iter().map(|c| c.default_binding()).collect();
        teclas.sort_unstable();
        teclas.dedup();
        assert_eq!(teclas.len(), total, "há combinações padrão repetidas");
    }

    /// No ABNT2 o `AltGr` é `Ctrl+Alt`. Um atalho global em `Ctrl+Alt+<letra>`
    /// rouba do teclado o caractere que aquela letra produziria — e as letras
    /// abaixo produzem caractere de verdade.
    #[test]
    fn nenhum_padrao_rouba_uma_tecla_do_abnt2() {
        const OCUPADAS_PELO_ALTGR: [&str; 6] = ["Q", "W", "E", "R", "C", "5"];

        for comando in Command::ALL {
            let combinacao = comando.default_binding();
            if !combinacao.starts_with("Ctrl+Alt+") {
                continue;
            }
            let tecla = combinacao.trim_start_matches("Ctrl+Alt+");
            assert!(
                !OCUPADAS_PELO_ALTGR.contains(&tecla),
                "{} usa Ctrl+Alt+{tecla}, que no ABNT2 é AltGr e produz caractere",
                comando.id()
            );
        }
    }
}
