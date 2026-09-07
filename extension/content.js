/* Destaque na própria página.
 *
 * ## Por que a CSS Custom Highlight API, e não <mark>
 *
 * Envolver o texto em elementos novos é o jeito clássico, e é o jeito errado
 * aqui: mudar o DOM de uma página que não é nossa quebra layout, dispara
 * observadores do site, invalida referências que o JavaScript dele guardou e
 * pode nem ser possível quando a seleção atravessa a fronteira de dois
 * elementos. `CSS.highlights` pinta um `Range` sem encostar no DOM — nenhum nó
 * é criado, movido ou alterado. É o que existe justamente para isto.
 *
 * ## Como o trecho falado vira uma posição na tela
 *
 * O Vox devolve os trechos na ordem em que vai falá-los. Aqui, os nós de texto
 * da seleção são achatados numa string única, guardando de qual nó veio cada
 * pedaço. Achar o trecho nessa string dá um par (nó, deslocamento) de início e
 * fim, que é exatamente o que um `Range` precisa.
 *
 * A busca é sequencial e nunca volta atrás: o texto que o Vox recebeu veio
 * desta mesma seleção, na mesma ordem, então cada trecho começa depois de onde
 * o anterior terminou. Isso também evita casar com uma repetição anterior da
 * mesma frase.
 */

(() => {
  // O script é injetado sob demanda e pode ser injetado de novo na mesma aba.
  if (window.__voxAtivo) return;
  window.__voxAtivo = true;

  const NOME_FRASE = "vox-frase";
  const NOME_PALAVRA = "vox-palavra";
  const suportaDestaque = typeof CSS !== "undefined" && CSS.highlights;

  let intervalos = [];      // um Range por trecho
  let palavrasPorTrecho = []; // [{ range, from, to }] por trecho
  let indiceAtual = -1;

  /* ------------------------------------------------------ achatar a seleção */

  function coletarNosDeTexto(range) {
    const raiz = range.commonAncestorContainer;
    const alvo = raiz.nodeType === Node.TEXT_NODE ? raiz.parentNode : raiz;
    const caminhador = document.createTreeWalker(alvo, NodeFilter.SHOW_TEXT, {
      acceptNode(no) {
        if (!range.intersectsNode(no)) return NodeFilter.FILTER_REJECT;
        if (!no.nodeValue || !no.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      },
    });

    const nos = [];
    let no;
    while ((no = caminhador.nextNode())) nos.push(no);
    return nos;
  }

  /* Monta a string achatada e o mapa de volta para (nó, deslocamento).
   *
   * O espaço entre nós é acrescentado de propósito: sem ele, o fim de um bloco
   * e o começo do seguinte virariam uma palavra só, e a busca pelo trecho
   * falharia logo na primeira quebra de parágrafo. */
  function achatar(nos, range) {
    let texto = "";
    const mapa = [];

    nos.forEach((no, ordem) => {
      const inicio = no === range.startContainer ? range.startOffset : 0;
      const fim = no === range.endContainer ? range.endOffset : no.nodeValue.length;
      const pedaco = no.nodeValue.slice(inicio, fim);
      if (!pedaco) return;

      mapa.push({ no, deslocamentoNoNo: inicio, inicioNaString: texto.length, tamanho: pedaco.length });
      texto += pedaco;
      if (ordem < nos.length - 1) texto += " ";
    });

    return { texto, mapa };
  }

  function posicaoParaNo(mapa, posicao) {
    for (const parte of mapa) {
      const fim = parte.inicioNaString + parte.tamanho;
      if (posicao >= parte.inicioNaString && posicao <= fim) {
        return { no: parte.no, deslocamento: parte.deslocamentoNoNo + (posicao - parte.inicioNaString) };
      }
    }
    const ultima = mapa[mapa.length - 1];
    if (!ultima) return null;
    return { no: ultima.no, deslocamento: ultima.deslocamentoNoNo + ultima.tamanho };
  }

  function intervaloEntre(mapa, de, ate) {
    const comeco = posicaoParaNo(mapa, de);
    const termino = posicaoParaNo(mapa, ate);
    if (!comeco || !termino) return null;
    const range = document.createRange();
    try {
      range.setStart(comeco.no, comeco.deslocamento);
      range.setEnd(termino.no, termino.deslocamento);
    } catch {
      return null;
    }
    return range;
  }

  /* Normaliza para comparar: a seleção traz quebras de linha e espaços duplos
     que o Vox já colapsou no texto que recebeu. */
  function normalizar(valor) {
    return valor.replace(/\s+/g, " ").trim();
  }

  /* Acha o trecho na string achatada tolerando diferença de espaços.
   *
   * Comparar direto falharia: a página pode ter quebra de linha onde o Vox tem
   * um espaço só. A busca compara caractere a caractere ignorando a quantidade
   * de espaço em branco, e devolve o intervalo na string original. */
  function acharTrecho(texto, alvo, apartirDe) {
    const procurado = normalizar(alvo);
    if (!procurado) return null;

    for (let inicio = apartirDe; inicio < texto.length; inicio += 1) {
      if (/\s/.test(texto[inicio])) continue;

      let posicaoNoTexto = inicio;
      let posicaoNoAlvo = 0;

      while (posicaoNoAlvo < procurado.length && posicaoNoTexto < texto.length) {
        const doTexto = texto[posicaoNoTexto];
        const doAlvo = procurado[posicaoNoAlvo];

        if (/\s/.test(doAlvo)) {
          if (!/\s/.test(doTexto)) break;
          while (posicaoNoTexto < texto.length && /\s/.test(texto[posicaoNoTexto])) posicaoNoTexto += 1;
          posicaoNoAlvo += 1;
          continue;
        }
        if (doTexto !== doAlvo) break;
        posicaoNoTexto += 1;
        posicaoNoAlvo += 1;
      }

      if (posicaoNoAlvo >= procurado.length) {
        return { de: inicio, ate: posicaoNoTexto };
      }
    }
    return null;
  }

  /* ------------------------------------------------------------- destaque */

  function limpar() {
    if (!suportaDestaque) return;
    CSS.highlights.delete(NOME_FRASE);
    CSS.highlights.delete(NOME_PALAVRA);
  }

  function preparar(segmentos, range) {
    const nos = coletarNosDeTexto(range);
    const { texto, mapa } = achatar(nos, range);

    intervalos = [];
    palavrasPorTrecho = [];
    let cursor = 0;

    for (const trecho of segmentos) {
      const achado = acharTrecho(texto, trecho, cursor);
      if (!achado) {
        // Um trecho que não casa não invalida os outros: guardamos um vazio
        // para os índices continuarem alinhados com a lista do Vox.
        intervalos.push(null);
        palavrasPorTrecho.push([]);
        continue;
      }
      cursor = achado.ate;

      const intervalo = intervaloEntre(mapa, achado.de, achado.ate);
      intervalos.push(intervalo);
      palavrasPorTrecho.push(fatiarEmPalavras(mapa, texto, achado));
    }
  }

  /* As frações de cada palavra são calculadas por caractere, igual à janela de
     leitura do Vox — as duas mostram a mesma fala e não podem discordar sobre
     onde ela está. */
  function fatiarEmPalavras(mapa, texto, achado) {
    const total = achado.ate - achado.de;
    if (total <= 0) return [];

    const palavras = [];
    let posicao = achado.de;

    while (posicao < achado.ate) {
      while (posicao < achado.ate && /\s/.test(texto[posicao])) posicao += 1;
      if (posicao >= achado.ate) break;

      const inicio = posicao;
      while (posicao < achado.ate && !/\s/.test(texto[posicao])) posicao += 1;

      const intervalo = intervaloEntre(mapa, inicio, posicao);
      if (intervalo) {
        palavras.push({
          range: intervalo,
          from: (inicio - achado.de) / total,
          to: (posicao - achado.de) / total,
        });
      }
    }
    return palavras;
  }

  function mostrar(index, ratio) {
    if (!suportaDestaque) return;

    if (index !== indiceAtual) {
      indiceAtual = index;
      const frase = intervalos[index];
      if (frase) {
        CSS.highlights.set(NOME_FRASE, new Highlight(frase));
        rolarAte(frase);
      } else {
        CSS.highlights.delete(NOME_FRASE);
      }
    }

    const palavras = palavrasPorTrecho[index] || [];
    const atual = palavras.find((palavra) => ratio >= palavra.from && ratio < palavra.to);
    if (atual) {
      CSS.highlights.set(NOME_PALAVRA, new Highlight(atual.range));
    } else {
      CSS.highlights.delete(NOME_PALAVRA);
    }
  }

  /* Só rola se a frase saiu de vista. Rolar a cada frase brigaria com o usuário
     que resolveu olhar outra parte da página enquanto ouve. */
  function rolarAte(range) {
    const caixa = range.getBoundingClientRect();
    if (caixa.height === 0 && caixa.width === 0) return;
    const dentro = caixa.top >= 0 && caixa.bottom <= window.innerHeight;
    if (dentro) return;

    const alvo = window.scrollY + caixa.top - window.innerHeight * 0.35;
    window.scrollTo({ top: Math.max(0, alvo), behavior: "smooth" });
  }

  /* ---------------------------------------------------------------- avisos */

  function avisar(mensagem) {
    const caixa = document.createElement("div");
    caixa.className = "vox-aviso";
    caixa.textContent = mensagem;
    document.body.appendChild(caixa);
    setTimeout(() => caixa.remove(), 4200);
  }

  /* ------------------------------------------------------------- mensagens */

  chrome.runtime.onMessage.addListener((mensagem, _remetente, responder) => {
    switch (mensagem?.tipo) {
      case "ping":
        responder({ ok: true });
        return false;

      case "pegar-selecao": {
        const selecao = window.getSelection();
        if (!selecao || selecao.isCollapsed || selecao.rangeCount === 0) {
          responder({ texto: "" });
          return false;
        }
        window.__voxRange = selecao.getRangeAt(0).cloneRange();
        responder({ texto: normalizar(selecao.toString()) });
        return false;
      }

      case "comecar":
        limpar();
        if (window.__voxRange) preparar(mensagem.segmentos || [], window.__voxRange);
        indiceAtual = -1;
        if (!suportaDestaque) {
          avisar("Vox: este navegador não tem a API de destaque; a leitura continua sem marcar o texto.");
        }
        responder({ ok: true });
        return false;

      case "posicao":
        mostrar(mensagem.index, mensagem.ratio);
        responder({ ok: true });
        return false;

      case "terminar":
        limpar();
        responder({ ok: true });
        return false;

      case "aviso":
        avisar(`Vox: ${mensagem.mensagem}`);
        responder({ ok: true });
        return false;

      default:
        return false;
    }
  });

  /* Esc para: é o gesto que já significa "para com isso" em toda parte. */
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!CSS.highlights?.has?.(NOME_FRASE)) return;
    limpar();
    chrome.runtime.sendMessage({ tipo: "parar" }).catch(() => {});
  });
})();
