/* Service worker da extensão.
 *
 * Ele é o único que fala com o Vox. O content script poderia chamar a ponte
 * direto, mas aí o `Origin` da requisição seria o do site — e a ponte recusa
 * qualquer origem que não seja `chrome-extension://`, justamente para uma
 * página web não conseguir mandar o computador falar. Mantendo o `fetch` aqui,
 * a origem é a da extensão e a barreira continua fazendo sentido.
 *
 * O worker também não guarda estado entre acordadas: o Chrome o descarrega
 * quando quiser. Tudo o que precisa sobreviver vive no content script ou no
 * `chrome.storage`.
 */

const PADRAO = { port: 8765, token: "" };

async function lerConfig() {
  const guardado = await chrome.storage.sync.get(PADRAO);
  return { port: Number(guardado.port) || PADRAO.port, token: guardado.token || "" };
}

async function chamarVox(caminho, opcoes = {}) {
  const { port, token } = await lerConfig();
  if (!token) {
    throw new Error(
      "Falta o token do Vox. Abra as opções da extensão e cole o valor de " +
        "bridge_token que está no settings.json do Vox.",
    );
  }

  const resposta = await fetch(`http://127.0.0.1:${port}${caminho}`, {
    ...opcoes,
    headers: {
      "Content-Type": "application/json",
      "X-Vox-Token": token,
      ...(opcoes.headers || {}),
    },
  });

  if (!resposta.ok) {
    const detalhe = await resposta.text().catch(() => "");
    throw new Error(`Vox respondeu ${resposta.status}. ${detalhe}`);
  }
  return resposta.json();
}

/* ------------------------------------------------------------- menu e atalho */

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "ler-com-vox",
    title: "Ler com o Vox",
    contexts: ["selection"],
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "ler-com-vox" && tab?.id) {
    iniciarLeitura(tab.id);
  }
});

chrome.commands.onCommand.addListener((comando) => {
  if (comando !== "ler-selecao") return;
  chrome.tabs.query({ active: true, currentWindow: true }, ([aba]) => {
    if (aba?.id) iniciarLeitura(aba.id);
  });
});

chrome.action.onClicked.addListener((aba) => {
  if (aba?.id) iniciarLeitura(aba.id);
});

/* ------------------------------------------------------------------ leitura */

/* O content script é injetado sob demanda em vez de declarado no manifesto.
 * Declarado, ele entraria em toda página que o usuário abrisse, para ficar
 * parado esperando um clique que quase nunca vem. Sob demanda, ele só existe na
 * aba onde a leitura foi pedida. */
async function garantirContentScript(tabId) {
  try {
    await chrome.tabs.sendMessage(tabId, { tipo: "ping" });
    return true;
  } catch {
    await chrome.scripting.executeScript({ target: { tabId }, files: ["content.js"] });
    await chrome.scripting.insertCSS({ target: { tabId }, files: ["content.css"] });
    return true;
  }
}

async function iniciarLeitura(tabId) {
  try {
    await garantirContentScript(tabId);

    const selecao = await chrome.tabs.sendMessage(tabId, { tipo: "pegar-selecao" });
    if (!selecao?.texto?.trim()) {
      await avisar(tabId, "Nada selecionado.");
      return;
    }

    const resultado = await chamarVox("/read", {
      method: "POST",
      body: JSON.stringify({ text: selecao.texto }),
    });

    // Os trechos vêm do Vox, e não de uma divisão feita aqui: é ele quem decide
    // onde uma frase termina, e duas divisões independentes divergiriam na
    // primeira abreviação — o destaque passaria a apontar para a frase errada.
    await chrome.tabs.sendMessage(tabId, {
      tipo: "comecar",
      segmentos: resultado.segments || [],
    });

    acompanhar(tabId);
  } catch (erro) {
    await avisar(tabId, String(erro.message || erro));
  }
}

/* Enquanto a leitura anda, o worker pergunta a posição ao Vox e repassa.
 *
 * O Chrome descarrega o service worker depois de ~30s ocioso, mas um `fetch`
 * pendente conta como atividade — e há um a cada 120 ms. O laço termina sozinho
 * quando o Vox para de reportar progresso ou a aba some. */
async function acompanhar(tabId) {
  let paradas = 0;

  while (paradas < 25) {
    await new Promise((resolve) => setTimeout(resolve, 120));

    let posicao;
    try {
      posicao = await chamarVox("/progress");
    } catch {
      return;
    }

    if (!posicao || posicao.segments === 0) {
      paradas += 1;
      continue;
    }
    paradas = 0;

    try {
      await chrome.tabs.sendMessage(tabId, {
        tipo: "posicao",
        index: posicao.index,
        ratio: posicao.ratio,
      });
    } catch {
      // A aba fechou ou navegou: não há mais o que destacar, e insistir só
      // manteria o worker acordado à toa.
      return;
    }
  }

  chrome.tabs.sendMessage(tabId, { tipo: "terminar" }).catch(() => {});
}

async function avisar(tabId, mensagem) {
  try {
    await chrome.tabs.sendMessage(tabId, { tipo: "aviso", mensagem });
  } catch {
    // Sem content script não há onde mostrar. O console da extensão fica.
    console.warn("[vox]", mensagem);
  }
}

/* Acompanha uma leitura que a extensão **não** começou.
 *
 * O Vox é um aplicativo de desktop e não tem como avisar o navegador: a ponte é
 * um servidor, e quem fala é a extensão. Quando a leitura começa pelo atalho
 * global, a página só descobre porque viu a mesma tecla — e é ela que pede isto
 * aqui.
 *
 * A `generation` do lado do Vox é o que distingue "a leitura de antes continua"
 * de "outra começou". Sem ela, a extensão pegaria os trechos da leitura anterior
 * e tentaria destacá-los numa página que já mudou.
 */
async function acompanharLeituraExterna(tabId) {
  const inicio = await chamarVox("/progress").catch(() => null);
  const geracaoAnterior = inicio?.generation ?? 0;

  // O Vox precisa de tempo para preparar o texto e começar: limpeza, divisão e,
  // quando ligada, uma ida ao modelo. Doze segundos cobrem o caso com correção;
  // além disso é mais provável que a leitura nem tenha começado.
  for (let tentativa = 0; tentativa < 60; tentativa += 1) {
    await new Promise((resolve) => setTimeout(resolve, 200));

    let atual;
    try {
      atual = await chamarVox("/current");
    } catch {
      return;
    }

    if (!atual?.segments?.length) continue;
    if (atual.generation === geracaoAnterior) continue;

    await garantirContentScript(tabId);
    await chrome.tabs.sendMessage(tabId, {
      tipo: "comecar",
      segmentos: atual.segments,
    }).catch(() => {});

    acompanhar(tabId);
    return;
  }
}

/* O content script pede para parar quando o usuário aperta Esc. */
chrome.runtime.onMessage.addListener((mensagem, _remetente, responder) => {
  if (mensagem?.tipo === "atalho-de-leitura") {
    // A combinação vem do Vox. Uma cópia aqui divergiria no primeiro ajuste que
    // a pessoa fizesse no painel, e o sintoma seria o pior possível: a leitura
    // funcionando e o destaque não.
    chamarVox("/health")
      .then((saude) => responder({ atalho: saude?.read_shortcut ?? null }))
      .catch(() => responder({ atalho: null }));
    return true;
  }

  if (mensagem?.tipo === "acompanhar-leitura-externa") {
    const tabId = _remetente?.tab?.id;
    if (tabId) acompanharLeituraExterna(tabId);
    responder({ ok: true });
    return false;
  }

  if (mensagem?.tipo === "ler-elemento") {
    const tabId = _remetente?.tab?.id;
    if (!tabId) {
      responder({ ok: false });
      return false;
    }
    // Passa pelo mesmo caminho do menu de contexto: a seleção já foi feita pelo
    // botão flutuante, e daqui para a frente não há diferença nenhuma.
    iniciarLeitura(tabId)
      .then(() => responder({ ok: true }))
      .catch((erro) => responder({ ok: false, erro: String(erro) }));
    return true;
  }

  if (mensagem?.tipo === "parar") {
    chamarVox("/stop", { method: "POST" })
      .then(() => responder({ ok: true }))
      .catch((erro) => responder({ ok: false, erro: String(erro.message || erro) }));
    return true; // resposta assíncrona
  }
  return false;
});
