//! Fila de narração — respostas de agentes de CLI esperando a vez de virar fala.
//!
//! ## Por que fila, e não "o último ganha"
//!
//! Um agente de terminal termina um turno e o próximo já pode estar em curso;
//! cortar a narração anterior no meio da frase para tocar a nova é a decisão
//! certa para um aviso de cinco segundos, e a errada para uma resposta que a
//! pessoa escolheu ouvir. Aqui as narrações esperam: quem chega entra no fim,
//! e quem controla (o painel do mod, ou o painel do Vox) pode cancelar, pular
//! ou promover qualquer item.
//!
//! ## Onde o texto é adaptado
//!
//! A fila guarda o texto bruto do agente, com markdown e tudo. A adaptação
//! para fala (o preset `narracao` da speech-api) acontece quando o item chega
//! ao topo, e não ao entrar: adaptação custa uma chamada de modelo, e um item
//! cancelado antes de tocar não deve ter pago por ela.
//!
//! ## O corrente mora fora da fila
//!
//! O item do topo muda de estado (`adaptando`, `pronto`, `tocando`) enquanto
//! os outros nem saíram do lugar, e as operações do painel precisam distinguir
//! "cancelar quem está falando" (para a fala) de "cancelar quem espera"
//! (só sai da fila). Por isso ele sai da `VecDeque` e vira `corrente`.

use std::collections::VecDeque;

use parking_lot::Mutex;
use serde::Serialize;

/// Comprimento do pedaço que o painel do mod mostra de cada item.
///
/// Reconhecer a resposta pela primeira frase basta; mandar o texto inteiro
/// pela ponte seria trafegar kilobytes para desenhar uma linha.
pub const PREVIEW_CHARS: usize = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Estado {
    /// Esperando a vez na fila.
    Pendente,
    /// No topo, sendo adaptado para fala.
    Adaptando,
    /// Adaptado e segurado: toca quando a pessoa mandar.
    Pronto,
    /// Sendo falado.
    Tocando,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Modo {
    /// Fala sozinho ao chegar ao topo.
    Falar,
    /// Fica pronto e espera o comando de tocar.
    Segurar,
}

impl Modo {
    /// O nome que viaja no pedido da ponte. `None` deixa o chamador decidir o
    /// padrão: um pedido sem modo usa o que estiver configurado no painel.
    pub fn do_nome(nome: Option<&str>) -> Option<Self> {
        match nome.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("speak") | Some("falar") => Some(Modo::Falar),
            Some("hold") | Some("segurar") => Some(Modo::Segurar),
            _ => None,
        }
    }

    pub fn nome(self) -> &'static str {
        match self {
            Modo::Falar => "speak",
            Modo::Segurar => "hold",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: u64,
    /// De onde veio — o rótulo da sessão que o mod manda junto.
    pub origem: String,
    pub modo: Modo,
    pub estado: Estado,
    /// O texto bruto do agente, com a marcação que ele escreveu.
    pub original: String,
}

impl Item {
    /// A primeira frase do texto, para o painel do mod reconhecer a resposta.
    pub fn previa(&self) -> String {
        let texto: String = self.original.split_whitespace().collect::<Vec<_>>().join(" ");
        texto.chars().take(PREVIEW_CHARS).collect()
    }
}

/// Por que um pedido de narração não entrou na fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recusa {
    FilaCheia,
}

/// O que aconteceu ao pedir para tocar um item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jogada {
    /// Era o item pronto no topo: a fala vai começar.
    Tocando,
    /// Estava na fila e subiu para o topo.
    Promovido,
    /// Já está sendo adaptado ou falado.
    JaTocando,
}

/// O que aconteceu ao cancelar um item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancelamento {
    /// Item pendente, removido da fila.
    Removido,
    /// Era o item corrente — quem chamou precisa parar a fala.
    Corrente,
    /// Não existe: já tocou, ou nunca existiu.
    Inexistente,
}

#[derive(Default)]
struct Fila {
    pendentes: VecDeque<Item>,
    corrente: Option<Item>,
    next_id: u64,
    /// Id cujo "tocar" foi pedido enquanto ele estava pronto.
    tocar_pedido: Option<u64>,
}

pub struct Narration {
    fila: Mutex<Fila>,
    wake: tokio::sync::Notify,
}

impl Narration {
    pub fn new() -> Self {
        Self {
            fila: Mutex::new(Fila::default()),
            wake: tokio::sync::Notify::new(),
        }
    }

    /// Enfileira um pedido. Devolve o id e a posição na fila.
    ///
    /// O limite vem de fora, lido das preferências no ato: um teto mudado no
    /// painel vale para o pedido seguinte, sem reiniciar nada.
    pub fn enqueue(
        &self,
        original: String,
        origem: String,
        modo: Modo,
        limite: usize,
    ) -> Result<(u64, usize), Recusa> {
        let mut fila = self.fila.lock();
        if fila.pendentes.len() >= limite {
            return Err(Recusa::FilaCheia);
        }
        fila.next_id += 1;
        let id = fila.next_id;
        let posicao = fila.pendentes.len();
        fila.pendentes.push_back(Item {
            id,
            origem,
            modo,
            estado: Estado::Pendente,
            original,
        });
        drop(fila);
        self.wake.notify_one();
        Ok((id, posicao))
    }

    /// Tira o próximo da fila e o declara corrente. `None` quando não há.
    pub fn begin_next(&self) -> Option<Item> {
        let mut fila = self.fila.lock();
        if fila.corrente.is_some() {
            return None;
        }
        let mut item = fila.pendentes.pop_front()?;
        item.estado = Estado::Adaptando;
        fila.corrente = Some(item.clone());
        Some(item)
    }

    pub fn corrente(&self) -> Option<Item> {
        self.fila.lock().corrente.clone()
    }

    pub fn pendentes(&self) -> Vec<Item> {
        self.fila.lock().pendentes.iter().cloned().collect()
    }

    /// Muda o estado do corrente, se ele ainda for este item.
    pub fn mark(&self, id: u64, estado: Estado) {
        let mut fila = self.fila.lock();
        if let Some(item) = &mut fila.corrente {
            if item.id == id {
                item.estado = estado;
            }
        }
    }

    /// O corrente ainda é este item?
    pub fn is_corrente(&self, id: u64) -> bool {
        self.fila.lock().corrente.as_ref().is_some_and(|item| item.id == id)
    }

    /// Encerra o corrente e libera o topo para o próximo.
    pub fn finish(&self, id: u64) {
        let mut fila = self.fila.lock();
        if fila.corrente.as_ref().is_some_and(|item| item.id == id) {
            fila.corrente = None;
        }
        drop(fila);
        self.wake.notify_one();
    }

    /// Manda um item tocar.
    ///
    /// O pronto no topo acorda o trabalhador; um item mais fundo sobe para o
    /// topo com modo `Falar` — a ordem da fila é a ordem das chegadas até a
    /// pessoa dizer o contrário.
    pub fn request_play(&self, id: u64) -> Result<Jogada, Cancelamento> {
        let mut fila = self.fila.lock();
        if let Some(item) = fila.corrente.as_ref().filter(|item| item.id == id) {
            if item.estado == Estado::Pronto {
                fila.tocar_pedido = Some(id);
                drop(fila);
                self.wake.notify_one();
                return Ok(Jogada::Tocando);
            }
            return Ok(Jogada::JaTocando);
        }

        let Some(posicao) = fila.pendentes.iter().position(|item| item.id == id) else {
            return Err(Cancelamento::Inexistente);
        };
        let mut item = fila.pendentes.remove(posicao).expect("posição confirmada acima");
        item.modo = Modo::Falar;
        fila.pendentes.push_front(item);
        drop(fila);
        self.wake.notify_one();
        Ok(Jogada::Promovido)
    }

    /// Remove um item da fila, ou declara o corrente cancelado.
    ///
    /// Quem cancela o corrente precisa parar a fala — a fila não sabe como;
    /// devolver `Corrente` é o contrato para o chamador fazer a sua parte.
    pub fn cancel(&self, id: u64) -> Cancelamento {
        let mut fila = self.fila.lock();
        if let Some(posicao) = fila.pendentes.iter().position(|item| item.id == id) {
            fila.pendentes.remove(posicao);
            drop(fila);
            self.wake.notify_one();
            return Cancelamento::Removido;
        }
        if fila.corrente.as_ref().is_some_and(|item| item.id == id) {
            fila.corrente = None;
            drop(fila);
            self.wake.notify_one();
            return Cancelamento::Corrente;
        }
        Cancelamento::Inexistente
    }

    /// Pula o corrente, se houver. `true` quando havia.
    pub fn skip(&self) -> bool {
        let mut fila = self.fila.lock();
        let tinha = fila.corrente.take().is_some();
        drop(fila);
        if tinha {
            self.wake.notify_one();
        }
        tinha
    }

    /// Consome o pedido de tocar `id`, se ele existir.
    pub fn take_play_request(&self, id: u64) -> bool {
        let mut fila = self.fila.lock();
        if fila.tocar_pedido == Some(id) {
            fila.tocar_pedido = None;
            true
        } else {
            false
        }
    }

    /// Dorme até a próxima mudança: item novo, cancelamento ou pedido de tocar.
    ///
    /// O `Notify` guarda um permisso entre a checagem e a espera, então a
    /// ordem "confere, depois dorme" não perde sinal.
    pub async fn wait(&self) {
        self.wake.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn narracao() -> Narration {
        Narration::new()
    }

    fn empilhar(fila: &Narration, rotulos: &[&str]) {
        for rotulo in rotulos {
            fila.enqueue(
                format!("resposta de {rotulo} com markdown"),
                (*rotulo).into(),
                Modo::Falar,
                10,
            )
            .expect("enqueue");
        }
    }

    #[test]
    fn fila_respeita_a_ordem_de_chegada() {
        let fila = narracao();
        empilhar(&fila, &["primeira", "segunda", "terceira"]);
        let ordem: Vec<String> = fila
            .pendentes()
            .iter()
            .map(|item| item.origem.clone())
            .collect();
        assert_eq!(ordem, vec!["primeira", "segunda", "terceira"]);
    }

    #[test]
    fn teto_recusa_sem_descartar_o_que_espera() {
        let fila = narracao();
        empilhar(&fila, &["uma", "dua"]);
        let terceiro = fila.enqueue("x".into(), "tre".into(), Modo::Falar, 2);
        assert_eq!(terceiro, Err(Recusa::FilaCheia));
        assert_eq!(fila.pendentes().len(), 2);
    }

    #[test]
    fn begin_next_promove_e_marca_como_corrente() {
        let fila = narracao();
        empilhar(&fila, &["primeira", "segunda"]);
        let item = fila.begin_next().expect("havia item");
        assert_eq!(item.origem, "primeira");
        assert_eq!(item.estado, Estado::Adaptando);
        // O corrente sai da fila de pendentes.
        assert_eq!(fila.pendentes().len(), 1);
        // Um segundo begin não furá a fila: o topo ainda está ocupado.
        assert!(fila.begin_next().is_none());
    }

    #[test]
    fn cancelar_pendente_sozinho_o_tira_da_fila() {
        let fila = narracao();
        empilhar(&fila, &["uma", "dua", "tre"]);
        let id = fila.pendentes()[1].id;
        assert_eq!(fila.cancel(id), Cancelamento::Removido);
        let restantes: Vec<String> = fila.pendentes().iter().map(|i| i.origem.clone()).collect();
        assert_eq!(restantes, vec!["uma", "tre"]);
    }

    #[test]
    fn cancelar_o_corrente_devolve_corrente_para_quem_para() {
        let fila = narracao();
        empilhar(&fila, &["uma"]);
        let item = fila.begin_next().expect("havia item");
        assert_eq!(fila.cancel(item.id), Cancelamento::Corrente);
        assert!(fila.corrente().is_none());
        assert!(!fila.is_corrente(item.id));
    }

    #[test]
    fn cancelar_o_que_ja_foi_e_inexistente() {
        let fila = narracao();
        assert_eq!(fila.cancel(999), Cancelamento::Inexistente);
        empilhar(&fila, &["uma"]);
        let item = fila.begin_next().expect("havia item");
        fila.finish(item.id);
        assert_eq!(fila.cancel(item.id), Cancelamento::Inexistente);
    }

    #[test]
    fn pular_so_funciona_com_corrente() {
        let fila = narracao();
        assert!(!fila.skip());
        empilhar(&fila, &["uma"]);
        fila.begin_next().expect("havia item");
        assert!(fila.skip());
        assert!(fila.corrente().is_none());
    }

    #[test]
    fn tocar_o_pronto_acorda_o_trabalhador() {
        let fila = narracao();
        empilhar(&fila, &["uma"]);
        let item = fila.begin_next().expect("havia item");
        fila.mark(item.id, Estado::Pronto);
        assert_eq!(fila.request_play(item.id), Ok(Jogada::Tocando));
        assert!(fila.take_play_request(item.id), "o pedido foi feito e deve ser consumível");
        assert!(!fila.take_play_request(item.id), "consumido duas vezes não é pedido");
    }

    #[test]
    fn tocar_quem_esta_adaptando_nao_registra_pedido() {
        let fila = narracao();
        empilhar(&fila, &["uma"]);
        let item = fila.begin_next().expect("havia item");
        assert_eq!(fila.request_play(item.id), Ok(Jogada::JaTocando));
        assert!(!fila.take_play_request(item.id));
    }

    #[test]
    fn tocar_pendente_promove_para_o_topo_e_muda_o_modo() {
        let fila = narracao();
        empilhar(&fila, &["uma", "dua"]);
        fila.enqueue("segurada".into(), "segurada".into(), Modo::Segurar, 10).expect("enqueue");
        let id = fila.pendentes()[2].id;
        assert_eq!(fila.request_play(id), Ok(Jogada::Promovido));
        let topo = &fila.pendentes()[0];
        assert_eq!(topo.id, id);
        assert_eq!(topo.modo, Modo::Falar, "quem a pessoa mandou tocar fala na chegada ao topo");
    }

    #[test]
    fn previa_colapsa_espacos_e_corta_no_limite() {
        let item = Item {
            id: 1,
            origem: "s".into(),
            modo: Modo::Falar,
            estado: Estado::Pendente,
            original: "  uma   frase\n\ncom    quebras ".into(),
        };
        assert_eq!(item.previa(), "uma frase com quebras");
        let longo = Item {
            original: "x".repeat(PREVIEW_CHARS * 3),
            ..item
        };
        assert_eq!(longo.previa().chars().count(), PREVIEW_CHARS);
    }

    #[test]
    fn modo_aceita_os_dois_vocabulos() {
        assert_eq!(Modo::do_nome(Some("speak")), Some(Modo::Falar));
        assert_eq!(Modo::do_nome(Some("HOLD")), Some(Modo::Segurar));
        assert_eq!(Modo::do_nome(Some("estranho")), None);
        assert_eq!(Modo::do_nome(None), None);
    }
}
