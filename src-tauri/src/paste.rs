//! Entrega da transcrição no app em foco.
//!
//! Colar via área de transferência + `Ctrl+V` sintético é mais confiável que
//! digitar caractere a caractere: campos com IME, editores que reagem a cada
//! tecla e caixas lentas engasgam com digitação simulada, e acentuação em
//! teclado ABNT2 se perde.

use std::time::Duration;

use anyhow::{Context, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};

use crate::config::{SubmitKey, SubmitMode};

/// Espera entre colar e apertar a tecla de envio.
///
/// O app de destino precisa processar o paste antes de receber o Enter; sem a
/// pausa, o envio chega num campo ainda vazio. 50 ms é o mesmo valor que o
/// Raycast usa.
const SUBMIT_DELAY: Duration = Duration::from_millis(50);

/// Espera entre escrever na área de transferência e disparar o `Ctrl+V`.
///
/// O Windows notifica os observadores da área de transferência de forma
/// assíncrona. Colar imediatamente às vezes cola o **conteúdo anterior** —
/// o app de destino lê antes de a atualização chegar até ele. É um erro raro,
/// intermitente e péssimo de diagnosticar, porque some quando se tenta
/// reproduzir com calma.
const PASTE_SETTLE: Duration = Duration::from_millis(30);

/// Quantas vezes reconferir se a área de transferência recebeu o texto.
///
/// Aplicativos com gerenciador de histórico (o próprio Windows tem um) podem
/// segurar o acesso por alguns milissegundos e fazer a escrita falhar em
/// silêncio.
const CLIPBOARD_RETRIES: u32 = 3;

/// Espera entre soltar os modificadores do atalho e mandar o `Ctrl+C`.
///
/// O app em foco precisa processar os keyups antes; sem a pausa ele ainda tem
/// Alt como pressionado quando o C chega.
const MODIFIER_SETTLE: Duration = Duration::from_millis(45);

/// Passo e teto da espera pela área de transferência depois do `Ctrl+C`.
const COPY_POLL: Duration = Duration::from_millis(25);
const COPY_TIMEOUT: Duration = Duration::from_millis(650);

pub struct Clipboard {
    inner: arboard::Clipboard,
}

impl Clipboard {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: arboard::Clipboard::new().context("abrir a área de transferência")?,
        })
    }

    pub fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner.set_text(text.to_string()).context("escrever na área de transferência")
    }

    /// Escreve e confere se o conteúdo realmente chegou.
    ///
    /// A escrita pode falhar em silêncio quando outro processo segura a área de
    /// transferência — gerenciadores de histórico fazem isso o tempo todo. Sem
    /// a conferência, o `Ctrl+V` seguinte colaria o conteúdo anterior, e o
    /// usuário veria o texto errado sem nenhum erro em lugar nenhum.
    pub fn set_text_verified(&mut self, text: &str) -> Result<()> {
        let mut ultimo_erro = None;

        for tentativa in 0..CLIPBOARD_RETRIES {
            match self.inner.set_text(text.to_string()) {
                Ok(()) => {
                    std::thread::sleep(PASTE_SETTLE);
                    match self.inner.get_text() {
                        Ok(lido) if lido == text => return Ok(()),
                        Ok(_) => {
                            tracing::debug!(tentativa, "a área de transferência não confirmou");
                        }
                        // Não conseguir ler de volta não prova que a escrita
                        // falhou; damos por boa em vez de repetir à toa.
                        Err(err) => {
                            tracing::debug!(?err, "sem leitura de volta; seguindo");
                            return Ok(());
                        }
                    }
                }
                Err(err) => {
                    tracing::debug!(?err, tentativa, "escrita falhou; tentando de novo");
                    ultimo_erro = Some(err);
                }
            }
            std::thread::sleep(PASTE_SETTLE);
        }

        match ultimo_erro {
            Some(err) => Err(err).context("escrever na área de transferência"),
            None => Err(anyhow::anyhow!(
                "a área de transferência não confirmou o texto após {CLIPBOARD_RETRIES} tentativas"
            )),
        }
    }

    pub fn text(&mut self) -> Result<String> {
        self.inner.get_text().context("ler a área de transferência")
    }
}

/// De onde o texto veio, para o chamador poder avisar o usuário.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionSource {
    /// O `Ctrl+C` funcionou e trouxe algo novo.
    Selection,
    /// Nada foi copiado; usamos o que já estava na área de transferência.
    ///
    /// Acontece em **terminais**, onde `Ctrl+C` é interrupção e não cópia, e em
    /// apps que ignoram teclado sintético. Sem esta distinção o usuário
    /// selecionaria um texto, apertaria o atalho, e ouviria outra coisa
    /// completamente — sem nenhuma pista do porquê.
    ClipboardFallback,
}

/// Copia a seleção do app em foco e diz de onde o texto veio.
///
/// Quando o `Ctrl+C` não produz nada, cai para o conteúdo já existente na área
/// de transferência em vez de devolver vazio: é quase sempre o que a pessoa
/// quer, e falhar sem tentar seria pior.
pub fn copy_selection_with_source() -> Result<(String, SelectionSource)> {
    let mut clipboard = Clipboard::new()?;
    let previous = clipboard.text().ok();
    let antes = previous.clone().unwrap_or_default();

    let mut enigo = Enigo::new(&EnigoSettings::default()).context("abrir o teclado virtual")?;

    // Solta os modificadores do atalho antes de mandar o Ctrl+C.
    //
    // Isto não é zelo: é o motivo de a cópia nunca funcionar. Quando o atalho
    // global dispara, a pessoa ainda está com Ctrl+Alt+L pressionado — o
    // sistema avisa na descida da tecla, não na subida. O Ctrl+C sintético
    // chega por cima disso e o app em foco recebe **Ctrl+Alt+C**, que não copia
    // nada em lugar nenhum. Sem seleção nova, caíamos sempre para o conteúdo
    // antigo da área de transferência — exatamente o sintoma de "só lê o que eu
    // copiei antes".
    //
    // Soltar uma tecla que o usuário ainda segura é seguro: o app recebe o
    // keyup, e o keyup físico que vem depois é ignorado por já estar solta.
    for modificador in [Key::Alt, Key::Shift, Key::Meta, Key::Control] {
        let _ = enigo.key(modificador, Direction::Release);
    }
    std::thread::sleep(MODIFIER_SETTLE);

    enigo.key(Key::Control, Direction::Press)?;
    enigo.key(Key::Unicode('c'), Direction::Click)?;
    enigo.key(Key::Control, Direction::Release)?;

    // Espera a área de transferência mudar em vez de apostar num tempo fixo.
    //
    // 120 ms bastavam num editor leve e não bastavam num navegador com a página
    // ocupada — e o erro aparecia como "leu o texto errado", sem nada no log.
    // Conferir em intervalos curtos devolve rápido quando é rápido e continua
    // funcionando quando não é.
    let mut depois = String::new();
    let limite = std::time::Instant::now() + COPY_TIMEOUT;
    while std::time::Instant::now() < limite {
        std::thread::sleep(COPY_POLL);
        depois = clipboard.text().unwrap_or_default();
        if !depois.trim().is_empty() && depois != antes {
            break;
        }
    }

    // Mudou: a cópia funcionou. Restauramos o anterior e devolvemos o novo.
    if !depois.trim().is_empty() && depois != antes {
        if let Some(previous) = previous {
            let _ = clipboard.set_text(&previous);
        }
        return Ok((depois, SelectionSource::Selection));
    }

    // Não mudou. Ou não havia seleção, ou o app não atende ao Ctrl+C — num
    // terminal ele interrompe o processo em vez de copiar. O que já estava na
    // área de transferência é o melhor palpite disponível.
    Ok((antes, SelectionSource::ClipboardFallback))
}

/// Versão sem a origem, para quem não precisa distinguir.
pub fn copy_selection() -> Result<String> {
    copy_selection_with_source().map(|(texto, _)| texto)
}

/// Ajusta espaçamento e caixa da transcrição conforme o que já está antes do cursor.
///
/// Regras, na ordem:
/// 1. Um espaço à frente, exceto se já houver espaço.
/// 2. Olha só a linha corrente (depois da última quebra).
/// 3. Linha vazia ou terminada em `.!?…` → mantém a maiúscula, é frase nova.
/// 4. Texto que começa com "I" isolado, ou que já não começa com uma única
///    maiúscula (siglas, nomes próprios, código) → mantém como veio.
/// 5. Caso contrário → minusculiza a primeira letra: é continuação de frase.
pub fn fit_to_context(text: &str, before_caret: Option<&str>) -> String {
    let Some(before) = before_caret else {
        return text.to_string();
    };

    let lead = if before.chars().next_back().is_some_and(char::is_whitespace) || before.is_empty() {
        ""
    } else {
        " "
    };

    let current_line = before.rsplit(['\n', '\r']).next().unwrap_or("").trim_end();
    let starts_sentence =
        current_line.is_empty() || current_line.ends_with(['.', '!', '?', '…']);

    if starts_sentence || keeps_own_case(text) {
        return format!("{lead}{text}");
    }

    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{lead}{}{}", first.to_lowercase(), chars.as_str()),
        None => lead.to_string(),
    }
}

/// Texto que não deve ter a caixa mexida.
fn keeps_own_case(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else { return true };

    // Pronome "I" em inglês, sozinho.
    if first == 'I' && chars.clone().next().is_none_or(|c| c.is_whitespace() || c == '\'') {
        return true;
    }

    // Não começa com maiúscula: nada a rebaixar.
    if !first.is_uppercase() {
        return true;
    }

    // Duas maiúsculas seguidas indicam sigla — "API", "CEP", "HTTP".
    chars.next().is_some_and(char::is_uppercase)
}

/// Remove a palavra-chave de envio do fim do texto.
///
/// Devolve `Some(texto_sem_a_palavra)` quando ela estava lá, `None` quando não.
/// A pontuação final é descartada antes da comparação, porque a transcrição
/// costuma acrescentar um ponto depois da palavra falada.
pub fn strip_submit_keyword(text: &str, keyword: &str) -> Option<String> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return None;
    }

    let trimmed = text.trim_end_matches([' ', '.', '!', '?', '…', ',', ';', ':', '\n', '\r', '\t']);

    // Corta por contagem de caracteres, não de bytes: `to_lowercase()` pode mudar
    // o comprimento em bytes (ß → ss), e aí um índice derivado do texto
    // minusculizado não serve para fatiar o original.
    let keyword_chars = keyword.chars().count();
    let trimmed_chars = trimmed.chars().count();
    if trimmed_chars < keyword_chars {
        return None;
    }

    let split_at = if trimmed_chars == keyword_chars {
        0
    } else {
        trimmed
            .char_indices()
            .nth(trimmed_chars - keyword_chars)
            .map(|(index, _)| index)?
    };
    let (head, tail) = trimmed.split_at(split_at);

    if tail.to_lowercase() != keyword.to_lowercase() {
        return None;
    }

    // A palavra tem que estar solta: "revolver" não pode casar com "ver".
    if !head.is_empty() && !head.ends_with(char::is_whitespace) {
        return None;
    }

    Some(head.trim_end().to_string())
}

/// Decide se o texto deve ser enviado, e devolve o texto já sem a palavra-chave.
pub fn resolve_submit(text: &str, mode: SubmitMode, keyword: &str) -> (String, bool) {
    match mode {
        SubmitMode::Disabled => (text.to_string(), false),
        SubmitMode::Auto => (text.to_string(), true),
        SubmitMode::Keyword => match strip_submit_keyword(text, keyword) {
            Some(stripped) => (stripped, true),
            None => (text.to_string(), false),
        },
    }
}

/// Cola o texto no app em foco e, se pedido, aperta a tecla de envio.
pub fn paste(text: &str, submit: Option<SubmitKey>) -> Result<()> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text_verified(text)?;

    let mut enigo = Enigo::new(&EnigoSettings::default()).context("abrir o teclado virtual")?;

    // Um respiro antes do Ctrl+V: o Windows propaga a mudança da área de
    // transferência de forma assíncrona, e apps que a observam podem ler o
    // valor antigo se o atalho chegar rápido demais.
    std::thread::sleep(PASTE_SETTLE);

    enigo.key(Key::Control, Direction::Press).context("segurar Ctrl")?;
    enigo.key(Key::Unicode('v'), Direction::Click).context("apertar V")?;
    enigo.key(Key::Control, Direction::Release).context("soltar Ctrl")?;

    let Some(submit_key) = submit else { return Ok(()) };

    std::thread::sleep(SUBMIT_DELAY);

    match submit_key {
        SubmitKey::Enter => {
            enigo.key(Key::Return, Direction::Click).context("apertar Enter")?;
        }
        SubmitKey::ShiftEnter => {
            enigo.key(Key::Shift, Direction::Press)?;
            enigo.key(Key::Return, Direction::Click)?;
            enigo.key(Key::Shift, Direction::Release)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuacao_de_frase_vira_minuscula() {
        assert_eq!(fit_to_context("Então fui", Some("Eu acordei e")), " então fui");
    }

    #[test]
    fn depois_de_ponto_mantem_maiuscula() {
        assert_eq!(fit_to_context("Então fui", Some("Cheguei.")), " Então fui");
    }

    #[test]
    fn campo_vazio_mantem_e_nao_poe_espaco() {
        assert_eq!(fit_to_context("Olá", Some("")), "Olá");
    }

    #[test]
    fn nao_duplica_espaco() {
        assert_eq!(fit_to_context("teste", Some("frase ")), "teste");
    }

    #[test]
    fn sigla_nao_e_rebaixada() {
        assert_eq!(fit_to_context("API nova", Some("usei a")), " API nova");
    }

    #[test]
    fn olha_apenas_a_linha_corrente() {
        // Linha corrente vazia: frase nova, e a quebra já serve de separador —
        // por isso nenhum espaço à frente.
        assert_eq!(fit_to_context("Nova", Some("Frase anterior.\n")), "Nova");
    }

    #[test]
    fn palavra_chave_e_removida_com_pontuacao() {
        assert_eq!(
            strip_submit_keyword("compra o pão manda ver.", "manda ver").as_deref(),
            Some("compra o pão")
        );
    }

    #[test]
    fn palavra_chave_grudada_nao_casa() {
        assert_eq!(strip_submit_keyword("passa o revolver", "ver"), None);
    }

    #[test]
    fn palavra_chave_solta_casa_mesmo_curta() {
        assert_eq!(strip_submit_keyword("manda ver", "ver").as_deref(), Some("manda"));
    }

    #[test]
    fn texto_menor_que_a_palavra_chave() {
        assert_eq!(strip_submit_keyword("oi", "manda ver"), None);
    }

    #[test]
    fn sem_palavra_chave_nao_marca_envio() {
        let (text, submit) = resolve_submit("só um texto", SubmitMode::Keyword, "manda ver");
        assert_eq!(text, "só um texto");
        assert!(!submit);
    }

    #[test]
    fn modo_auto_envia_sem_mexer_no_texto() {
        let (text, submit) = resolve_submit("qualquer coisa", SubmitMode::Auto, "manda ver");
        assert_eq!(text, "qualquer coisa");
        assert!(submit);
    }
}
