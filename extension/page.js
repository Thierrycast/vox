/* Presença leve do Vox em toda página.
 *
 * ## Por que este arquivo existe separado do content.js
 *
 * O `content.js` carrega a máquina de destaque — achatar a seleção, casar
 * trechos, montar intervalos. Ele é injetado sob demanda, só na aba onde alguém
 * pediu uma leitura, e essa decisão continua certa: seria desperdício tê-lo
 * parado em toda aba aberta esperando um clique que quase nunca vem.
 *
 * Mas duas coisas precisam estar presentes **antes** de qualquer pedido:
 *
 * - o botão que aparece ao passar o mouse num parágrafo;
 * - o ouvinte do atalho global do Vox, para a página poder acompanhar uma
 *   leitura que começou fora dela.
 *
 * Este arquivo é essa presença: sem DOM próprio até o mouse parar em algum
 * lugar, sem rede, sem observador de mutação.
 *
 * ## O problema que o ouvinte de atalho resolve
 *
 * O Vox é um aplicativo de desktop e a extensão é um cliente dele. Não há canal
 * do app para o navegador — a ponte é um servidor, e quem fala é a extensão.
 * Quando a leitura começa pelo atalho global, a página não fica sabendo de nada,
 * e o resultado é a voz falando sem destaque nenhum.
 *
 * A saída é que a própria página veja a tecla. Ela tem foco quando o texto foi
 * selecionado ali, então o mesmo `Ctrl+Alt+L` que o Windows entrega ao Vox
 * chega também a este ouvinte. Ao vê-lo, a página pergunta ao Vox o que está
 * sendo lido e passa a acompanhar.
 */

(() => {
  if (window.__voxPagina) return;
  window.__voxPagina = true;

  /* --------------------------------------------------------------- atalho */

  /* A combinação vem do Vox, e não de uma cópia aqui: quem a muda, muda no
     painel, e uma segunda lista divergiria no primeiro ajuste. */
  let atalhoDeLeitura = null;

  chrome.runtime.sendMessage({ tipo: "atalho-de-leitura" })
    .then((resposta) => { atalhoDeLeitura = resposta?.atalho ?? null; })
    .catch(() => {});

  function combinacaoBate(event, texto) {
    if (!texto) return false;

    const partes = texto.toLowerCase().split("+").map((parte) => parte.trim());
    const tecla = partes[partes.length - 1];

    if (partes.includes("ctrl") !== event.ctrlKey) return false;
    if (partes.includes("alt") !== event.altKey) return false;
    if (partes.includes("shift") !== event.shiftKey) return false;

    // `event.code` em vez de `event.key`: com Alt pressionado o Windows entrega
    // caracteres estranhos em `key`, e o código físico da tecla não muda.
    return event.code.toLowerCase() === `key${tecla}`;
  }

  document.addEventListener("keydown", (event) => {
    if (!combinacaoBate(event, atalhoDeLeitura)) return;

    const selecao = window.getSelection();
    if (!selecao || selecao.isCollapsed) return;

    // A tecla continua o caminho dela até o Vox — não há `preventDefault` aqui.
    // Quem lê é ele; esta página só quer acompanhar o que ele vai falar.
    chrome.runtime.sendMessage({ tipo: "acompanhar-leitura-externa" }).catch(() => {});
  }, true);

  /* ------------------------------------------------------------ botão flutuante
   *
   * Um alvo pequeno que segue o parágrafo sob o cursor. O gesto que ele resolve
   * é o mais comum de todos: ler **este** trecho, sem selecionar nada.
   *
   * Ele não entra no DOM da página: mora num `position: fixed` próprio, com
   * z-index alto e `all: initial` no container, para o CSS do site não o
   * deformar nem ser deformado por ele. */

  const ALVOS = "p, li, blockquote, h1, h2, h3, h4, dd, td";
  const MINIMO_DE_TEXTO = 60;

  let botao = null;
  let alvoAtual = null;
  let saidaHandle = null;

  function criarBotao() {
    if (botao) return botao;

    botao = document.createElement("button");
    botao.type = "button";
    botao.className = "vox-botao-flutuante";
    botao.title = "Ler daqui com o Vox";
    botao.setAttribute("aria-label", "Ler daqui com o Vox");
    botao.innerHTML =
      '<svg viewBox="0 0 20 20" width="13" height="13" fill="currentColor">' +
      '<path d="M6.5 4l9 6-9 6z"/></svg>';

    botao.addEventListener("mouseenter", () => clearTimeout(saidaHandle));
    botao.addEventListener("mouseleave", agendarSaida);
    botao.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (!alvoAtual) return;

      const texto = alvoAtual.innerText.trim();
      if (!texto) return;

      // A seleção é refeita no elemento inteiro: é ela que o destaque usa para
      // saber onde pintar, e clicar num botão não seleciona nada sozinho.
      const intervalo = document.createRange();
      intervalo.selectNodeContents(alvoAtual);
      const selecao = window.getSelection();
      selecao.removeAllRanges();
      selecao.addRange(intervalo);

      botao.dataset.estado = "pedindo";
      chrome.runtime.sendMessage({ tipo: "ler-elemento", texto })
        .catch(() => {})
        .finally(() => { delete botao.dataset.estado; });
    });

    document.documentElement.appendChild(botao);
    return botao;
  }

  function posicionar(elemento) {
    const caixa = elemento.getBoundingClientRect();
    const alvo = criarBotao();

    // À esquerda do parágrafo quando há margem; por dentro quando não há, para
    // não sair da janela em página sem recuo.
    const esquerda = caixa.left > 42 ? caixa.left - 34 : caixa.left + 6;
    alvo.style.top = `${Math.max(6, caixa.top + 2)}px`;
    alvo.style.left = `${esquerda}px`;
    alvo.dataset.visivel = "true";
  }

  function agendarSaida() {
    clearTimeout(saidaHandle);
    // A folga existe para o percurso do cursor até o botão: sem ela, o gesto de
    // ir até ele o faz desaparecer no meio do caminho.
    saidaHandle = setTimeout(() => {
      if (botao) botao.dataset.visivel = "false";
      alvoAtual = null;
    }, 260);
  }

  document.addEventListener("mouseover", (event) => {
    const alvo = event.target.closest?.(ALVOS);
    if (!alvo || alvo === alvoAtual) return;

    // Parágrafo curto não vale um botão: o custo de mirar nele é maior que o de
    // ler com os olhos.
    const texto = alvo.innerText?.trim() ?? "";
    if (texto.length < MINIMO_DE_TEXTO) return;

    clearTimeout(saidaHandle);
    alvoAtual = alvo;
    posicionar(alvo);
  }, true);

  document.addEventListener("mouseout", (event) => {
    if (!alvoAtual) return;
    if (event.relatedTarget === botao) return;
    if (alvoAtual.contains(event.relatedTarget)) return;
    agendarSaida();
  }, true);

  // Rolar move o parágrafo debaixo do botão; segui-lo a cada quadro custaria
  // mais do que o botão vale, e escondê-lo é o que a pessoa espera de qualquer
  // maneira — o cursor já saiu de onde estava.
  window.addEventListener("scroll", () => {
    if (botao) botao.dataset.visivel = "false";
    alvoAtual = null;
  }, { passive: true });
})();
