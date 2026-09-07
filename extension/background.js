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

/* O content script pede para parar quando o usuário aperta Esc. */
chrome.runtime.onMessage.addListener((mensagem, _remetente, responder) => {
  if (mensagem?.tipo === "parar") {
    chamarVox("/stop", { method: "POST" })
      .then(() => responder({ ok: true }))
      .catch((erro) => responder({ ok: false, erro: String(erro.message || erro) }));
    return true; // resposta assíncrona
  }
  return false;
});
